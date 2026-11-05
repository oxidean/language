# Ownership & Memory Semantics (v0)

Implements the approved design doc's memory model. This file is the
contract the typechecker, codegen, and runtime are built against.

## Object kinds

| Kind | Written | Lifetime | Check |
|---|---|---|---|
| Value | `x := v`, params | lexical (stack/inline) | none |
| Borrowed ref | `ref T` param/field | tied to lender's scope | dangling |
| Mutable ref | `mut T` param | exclusive while live | aliasing + dangling |
| Owned heap | `own T` | explicit / scope arena | moved-on-use + dangling |
| Shared | `Shared<T>` | atomic state | aliasing + dangling (atomic) |

## Allocation sites

- **Scope arena (default).** Plain allocations inside a scope —
  params, locals, temporaries — come from the enclosing scope's arena.
  Arena memory is bulk-freed when the scope closes. Arena objects may
  NOT escape their scope — the typechecker rejects returning arena
  references or sending them up-scope. Assigning a scope-backed slice
  to an outer struct field is NOT currently checked — storing one and
  accessing it after the scope closes reads freed storage.
  `new(T)` is the exception: it allocates via `rt_alloc_own` (owned
  heap), so the result is NOT scope-tracked and is NOT bulk-freed when
  the scope closes — it lives until an explicit `rt_free` (extern C
  callers, or `rt_free` through the FFI surface) or process exit; the
  code generator does not emit handle-death reclamation today.
- **Owned heap.** `own new(T)` (or the `own` keyword on an allocation)
  produces an `own T` handle valid until its owner dies. Ownership moves
  on assignment/argument/return; after a move the old handle is dead —
  reads through it are compile errors where statically visible, runtime
  traps otherwise.

`new(T)` returns zero-initialized memory — `== nil` field checks are
reliable. `new(expr)` heap-boxes a *value*: `new(E.V(...))` yields
`own E`, `new(42)` yields `own int`.

## Borrow rules (dynamic)

1. While a `mut T` borrow is live: no other `mut` or `ref` borrow of the
   same object may activate. Violation → `TrapAliasing`. A call's `own`
   argument borrow is live for the whole call — `f(x, g(x))` where `x`
   is `own` and both params take `mut`/`ref` traps; hoist the inner
   call to a `let`/`:=` so the borrows sequence.
   (Passing a `mut`/`ref` *reference* on is a reborrow — no runtime op —
   only `own` arguments start a fresh borrow.)
2. Any access through a dead handle (moved `own`, outlived arena
   reference, freed generation) → `TrapDangling`.
3. Channel `send` transfers ownership: the sent handle dies at the
   sender and revives at the receiver.
4. `Shared<T>` performs the same checks atomically; a poisoned
   `Shared<T>` (owner trapped mid-mutation) traps on next access.

## Runtime metadata

Each checked heap object carries a header:

```c
typedef struct ObjHdr {
    uint32_t gen;            // generation tag — dangling detection
    uint8_t  borrow;         // 0 = free, 1 = shared-readers>0, 2 = mut-held
    uint8_t  arena;          // arena-owned flag
    uint8_t  poisoned;       // owner trapped mid-mutation — next access traps
    uint8_t  _pad;
    RtScope *hscope;         // owning scope for heap-fallback blocks
    struct ObjHdr *hnext;    // scope heap-list link
} ObjHdr;                    // 24 bytes on x86_64
```

Checks are emitted at borrow activation and on `ref`/`mut` call
arguments (`rt_borrow_read`/`rt_borrow_mut`). Ordinary `own`-handle
dereferences compile to raw C access today — per-dereference dangling
checks are the target contract, not yet emitted (and elidable wherever
the typechecker proves safety).

## Trap behavior

- Trap inside a `scope`: kill task, cancel siblings, unwind scope, free
  scope arena, propagate `ErrTrap` to parent scope's wait point.
- Trap outside any scope, or metadata corruption detected during unwind:
  `abort()`.
- Suppression is not implemented: `unsafe { }` parses as a plain
  scoping block today and silences nothing; there is no global build
  flag that disables runtime checks.

## Compile-time rules

- `own` handles are move-tracked statically (D2). Move sites: an `own`
  argument to an `own` parameter, `return` of an `own` result, a
  struct/enum field store (`s.f = v`, `E.V{ f: v }`), and `send(ch, v)`.
  Using a moved binding is a compile error naming it; re-binding
  (`x = new(T)`, `x := f()`) clears the mark. Deliberately *not* move
  sites: `y := x` and `y = x` between variables (the aliasing walk
  idiom `cur := head`), field reads, and `ref`/`mut` borrows — those
  stay runtime-checked. At branch joins a binding is dead when
  moved on *any* reachable fall-through path (`if`/`else`, `match`,
  `select` arms) — a union merge, since the join can't prove which
  branch ran; a move inside a branch that always diverges (returns,
  breaks) still doesn't count. A net
  move inside a `for` body is a compile error since the next iteration
  would read a moved value — re-initializing it in the same iteration
  is the fix. `mkshared` returns `shared` — a copyable atomic-cell type
  that is never move-tracked, so the same cell may be handed to many
  workers (`shared` params replace `own` for that API).
- Dropping a non-`nil` second return value (`err`) silently is a compile
  error. `_` may discard a *value*, never an error.
- `ref`/`mut` borrows may not be stored in heap objects (v0 restriction;
  keeps the escape analysis trivial).
- Arena references may not cross a `spawn` boundary — only `own` and
  values may be sent/captured.
