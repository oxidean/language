# Compiler Interface Contracts (v0)

Stage boundaries so parallel worktrees can build independently. Each
stage consumes the previous stage's output; shapes below are the
contract. Implementation language: **Rust** (design doc: "Zig preferred,
Rust acceptable" — Zig absent on the dev machine, Rust 1.100 present).

## Pipeline

```
source ──▶ lexer ──▶ parser ──▶ AST ──▶ typecheck ──▶ TAST ──▶ codegen ──▶ C ──▶ cc ──▶ native
                            diagnostics ◀────────── all stages emit
```

## Diagnostics (all stages)

```rust
struct Diag { span: Span, sev: Sev, code: &'static str, msg: String }
enum Sev { Error, Warning }
struct Span { file: u16, lo: u32, hi: u32 }
```

No stage panics on bad input; it emits `Diag`s and the pipeline stops
before codegen if any `Error` exists.

## Lexer → Token

```rust
enum Tok { Ident, Int, Str, Kw(Kw), Punct(P), Eof }
struct Token { tok: Tok, span: Span, text: Range<u32> } // text = byte range into src
```

Keywords (v0): `fn struct extern return if else for let scope spawn go
send recv chan own ref mut unsafe nil true false import`

## AST (parser output)

`Crate { items: Vec<Item> }` where `Item = Fn | Struct | Extern | Import`.
Expressions/statements per `oxl/src/ast.rs` — the AST is deliberately
syntactic (no desugaring in parser except `x := e` → `Let{infer:true}`).

## TAST (typechecker output)

- Every `Expr` node annotated with `Ty`.
- `Ty = Int | Bool | Str | CStr | Void | Err | Struct(Id) | Own(Box<Ty>) |
  Ref(Box<Ty>) | Mut(Box<Ty>) | Chan(Box<Ty>) | Shared(Box<Ty>)`
- Function signatures fully resolved (boundary annotations required —
  inference is locals-only).
- Multi-return `[T, err]` lowered to `Ret2 { val: Ty, err: Err }`.
- Side table `suspend: Set<FnId>` — suspend-capable functions
  (concurrency.md).
- Side table `moves: Set<NodeId>` — points where an `own` handle dies.

## Codegen (interim backend)

Target: **C99 → cc → native binary**. Chosen as the bootstrap backend:
LLVM bindings would gate the skeleton on an external dependency; C emit
exercises the full pipeline today and keeps the backend swappable
(`codegen/` is the only cc-aware module; an LLVM backend drops in behind
the same `TAST → object` seam).

Emitted program links `runtime/rt.c` (committed under `runtime/`) which
provides: arena alloc, ObjHdr checks, scope/trap machinery, executor
pool, channels, and the `extern` FFI shim table.

## Runtime ABI (codegen → rt.c)

| Symbol | Contract |
|---|---|
| `rt_arena_push/pop` | scope enter/exit; frees arena on pop |
| `rt_alloc(size, scope)` → `void*` | arena or heap per flag |
| `rt_borrow_mut/read(obj)` / `rt_unborrow*` | aliasing checks |
| `rt_deref(obj, gen)` | dangling check before access |
| `rt_scope_enter/leave` | trap containment + sibling cancel |
| `rt_spawn(fn, args, scope)` / `rt_send/recv/close` | executor + channels |
| `rt_trap(code)` | raise trap at current scope boundary |
| `oxl_main()` | emitted entry; `main()` lives in rt.c |

## Error semantics

`[v, err] = f()` lowers to `Ret2{ptr, errno}` — `err` is a small integer
(0 = nil). `rt_trap` raises a scope-contained trap (unwind to scope
boundary), NOT an errno.
