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
enum Tok { Ident(String), Int(i64), Float(f64), Str(String), Kw(Kw), P(P), Eof }
struct Token { tok: Tok, span: Span }   // Span { lo, hi } — byte offsets (diag.rs)
```

Keywords: `fn struct extern return if else for let scope spawn go own
ref mut unsafe nil true false import chan break continue defer select
enum match interface spawnBlocking` — `send`/`recv` are builtins, not
keywords.

## AST (parser output)

`Crate { items: Vec<Item> }` where `Item = Fn | Struct | Enum | Extern |
Interface | Import | Global`. Expressions/statements per
`oxl/src/ast.rs` — the AST is deliberately syntactic (no desugaring in
parser; `x := e` parses to `Stmt::Let { ty: None, .. }`).

## TAST (typechecker output — target contract)

`check::Checked` today carries only `suspend: Set<FnId>` and the LSP
`index`; codegen consumes the original `Crate` plus `checked.suspend`.
The rest of this section is the boundary the backend design expects —
not yet emitted by the checker:

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
| `rt_scope_enter/leave(s)` / `rt_scope_join_poll` / `rt_scope_finish` | scope lifecycle; join frees the arena; trapped codes propagate to the joiner |
| `rt_alloc(size)` / `rt_alloc_own(size)` → `void*` | scope arena (heap fallback, scope-tracked) / always-heap for escaping data |
| `rt_borrow_read/mut(obj)` / `rt_unborrow(obj)` / `rt_free(obj)` | aliasing checks + explicit release of own blocks |
| `rt_spawn(fn, arg)` / `rt_spawnb(fn, arg)` | thread tasks (non-susp fns) / blocking pool |
| `rt_spawn_task(fr)` / `rt_spawnb_task(fr)` / `rt_run_root(fr)` | frame tasks: work pool / blocking pool / drive the root frame to completion |
| `rt_send/recv/close(ch, v)` + `rt_send/recv_poll` | channels: blocking surface for thread tasks, poll surface for frames |
| `rt_select_poll(chs, n, tmo_ms, has_else, task)` / `rt_select_arm(task)` | select: poll all arms for readiness; clean up a parked wait, returning the winning arm |
| `rt_trap(code)` | raise trap; the chain up to the task's spawn scope cancels siblings and propagates `err` to the joiner |
| `oxl_main()` | emitted entry; `main()` lives in rt.c |

## Error semantics

`[v, err] = f()` lowers to a per-function `struct o_ret_<fn> { <T>
val; int64_t err }` — `err` is a small integer (0 = nil). `rt_trap`
raises a scope-contained trap (unwind to scope boundary), NOT an errno.
