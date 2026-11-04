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

- **Scope arena (default).** `new(T)` inside a scope allocates in the
  enclosing scope's arena. Arena memory is bulk-freed when the scope
  closes. Arena objects may NOT escape their scope — the typechecker
  rejects returning, storing into outer structures, or sending arena
  references up-scope.
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
   same object may activate. Violation → `TrapAliasing`.
2. Any access through a dead handle (moved `own`, outlived arena
   reference, freed generation) → `TrapDangling`.
3. Channel `send` transfers ownership: the sent handle dies at the
   sender and revives at the receiver.
4. `Shared<T>` performs the same checks atomically; a poisoned
   `Shared<T>` (owner trapped mid-mutation) traps on next access.

## Runtime metadata

Each checked heap object carries a header:

```c
typedef struct {
    uint32_t gen;        // generation tag — dangling detection
    uint8_t  borrow;     // 0 = free, 1 = shared-readers>0, 2 = mut-held
    uint8_t  arena;      // arena-owned flag
} ObjHdr;
```

Checks are emitted at every borrow activation and every dereference
through a handle, and are elided where the typechecker proves safety
(future optimization pass — v0 emits all checks).

## Trap behavior

- Trap inside a `scope`: kill task, cancel siblings, unwind scope, free
  scope arena, propagate `ErrTrap` to parent scope's wait point.
- Trap outside any scope, or metadata corruption detected during unwind:
  `abort()`.
- Opt-out: `unsafe { }` block suppresses check emission inside it;
  `--unsafe` build flag suppresses checks globally. Both are greppable
  and never implicit.

## Compile-time rules

- Dropping a non-`nil` second return value (`err`) silently is a compile
  error. `_` may discard a *value*, never an error.
- `ref`/`mut` borrows may not be stored in heap objects (v0 restriction;
  keeps the escape analysis trivial).
- Arena references may not cross a `spawn` boundary — only `own` and
  values may be sent/captured.
