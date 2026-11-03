# Language Foundation — Implementation Plan

Built from the approved design doc:
`docs/superpowers/specs/2026-11-03-language-design.md`

Execution model: sequential critical path through Phase 0–1, then
parallel worktree tracks in Phase 2. Every task lands behind the
`language-principles` standard.

**Status (2026-11-03): Phase 0 + Phase 1 complete; Phase 2 underway.**
The compiler compiles all `examples/` to native binaries and they run —
including the HTTP CRUD + SQLite pilot. Implementation language is Rust
(approved alternative — Zig absent on the dev machine); codegen backend
is C emit → cc (documented interim, swappable for LLVM behind the
`interfaces.md` seam); tasks run on interim pool threads pending the
state-machine transform.

## Task 1: Save spec documentation [done]

Create `agent-os/specs/2026-11-03-0036-language-foundation/` with
plan.md, shape.md, standards.md, references.md. (Done — this commit.)

## Phase 0 — Semantics (sequential; blocks all compiler work)

### Task 2: Language semantics annex [done]

`docs/semantics/ownership.md`, `docs/semantics/concurrency.md`,
`docs/semantics/interfaces.md` — violation taxonomy + check sites;
hybrid reclamation; trap propagation + poisoned-`Shared<T>`; suspend set
(reachability); stage contracts lexer → AST → typecheck → codegen →
runtime ABI.

### Task 3: De-risking prototype [done]

`prototype/` + `docs/prototype/report.md`. Measured: state-machine
frames ~96 B/task vs ~8.5 kB/task pooled threads (88×); ownership checks
at measurement noise (~0–4%). Verdict: GO.

## Phase 1 — Compiler skeleton (sequential)

### Task 4: Toolchain skeleton [done]

`lang/` (Rust): `build run test fmt doc lsp`. build/run/test work;
fmt/doc are honest stubs; lsp is a stdio JSON-RPC scaffold (initialize/
shutdown/exit).

### Task 5: Frontend core [done]

`lang/src/{lexer,ast,parser}.rs` — full v0 subset: literals, fns,
structs, extern decls, `let`/`:=`, `[v, err]` binding, if/else, for,
scope/spawn/go, unsafe, struct literals, comments.

### Task 6: Typechecker v1 [done]

`lang/src/check.rs` — boundary annotations enforced, `:=` local
inference, dropped-error compile error, own→ref/mut borrow conversion,
FFI arg restrictions, suspend-set computation.

### Task 7: Codegen v1 [done]

`lang/src/codegen.rs` — C emit → cc → native binary. Interim backend
(LLVM remains the production target behind the same seam). Spawn
trampolines, `[v, err]` structs, borrow-check emission, builtin
expansion, test driver.

### Task 8: Runtime core [done]

`runtime/rt.{c,h}` — scope arenas (bulk-free on leave), ObjHdr
gen/borrow/poison checks, scope-contained traps with sibling cancel +
parent propagation + abort fallback, executor (interim thread-per-spawn),
unbuffered channels, `unsafe` opt-out hooks.

### Task 9: Errors + C FFI [done]

`[v, err]` lowered to `Ret2` structs; `nil`; `extern fn` direct C calls;
libc + sqlite3 exercised by pilots.

## Phase 2 — Parallel tracks (fan out in separate worktrees)

### Task 10: Coroutine transform production [partial]

Surface semantics final and working (scope join/cancel, channels,
suspend set computed in check.rs). Execution is interim pool threads —
the state-machine transform itself is open (critical path item).

### Task 11: Stdlib wave 1 [partial]

Builtins in place: `print/println`, `itoa`, string ops
(`len/substr/find/byteat`), `argc/argv`, `mkbuf/bufstr/poke*`, `assert`,
`mkchan/send/recv/close`, `sleep`, sqlite3 shims, `cstr`. Missing:
`fmt`, `io`, `os`, `testing` as modules (no module system yet).

### Task 12: Stdlib wave 2 [not started]

`net/http`, `encoding/json`, `sync`, `time` — pilot uses raw socket FFI
instead of `net/http` today.

### Task 13: Multithread executor + blocking pool [partial]

Thread-per-spawn works under scopes; work-stealing pool, blocking pool
for FFI, and `Shared<T>` atomics are open.

### Task 14: Generics [not started]

Type parameters + constraint interfaces; shape-based instantiation
preferred.

### Task 15: LSP + fmt + doc [scaffold]

`lang lsp` speaks JSON-RPC initialize/shutdown/exit; hover/goto/
diagnostics, formatter, doc generator open.

### Task 16: OSS plumbing [partial]

README done; MIT LICENSE, CI matrix, release workflow, CONTRIBUTING open.

## Phase 3 — Gate

### Task 17: Pilot apps [partial — running]

`examples/cli` — CLI with argv + string ops + error paths: **works**.
`examples/http-crud-sqlite` — HTTP server, socket FFI, per-connection
`spawn`, sqlite3 insert/select: **works** (verified: POST inserts, GET
returns rows). "Production-shaped conditions" (sustained load, memory
profile, deployment shape) still to run.

### Task 18: v0.0.1 tag [not started]

Gate: both pilots run. Tag the release.
