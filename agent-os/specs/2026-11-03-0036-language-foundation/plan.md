# Language Foundation — Implementation Plan

Built from the approved design doc:
`docs/superpowers/specs/2026-11-03-language-design.md`

Execution model: sequential critical path through Phase 0–1, then
parallel worktree tracks in Phase 2. Every task lands behind the
`language-principles` standard.

## Task 1: Save spec documentation

Create `agent-os/specs/2026-11-03-0036-language-foundation/` with
plan.md, shape.md, standards.md, references.md. (Done — this commit.)

## Phase 0 — Semantics (sequential; blocks all compiler work)

### Task 2: Language semantics annex

Produce `docs/semantics/ownership.md`, `docs/semantics/concurrency.md`,
`docs/semantics/interfaces.md` pinning: violation taxonomy + check sites
(aliasing + dangling); hybrid reclamation rules (arena scope default,
owned-heap escape, explicit escape syntax); trap propagation and
poisoned-`Shared<T>` semantics; suspend-capable set (reachability from
spawn sites) and the opt-in-marker fallback; compiler interface contracts
(lexer/parser → AST → typechecker → IR → codegen → runtime ABI) crisp
enough for worktrees to build against.

### Task 3: De-risking prototype

Spike the two riskiest mechanics on a representative subset: the
state-machine transform (compile-cost) and ownership-check metadata
(runtime overhead). Deliverable: `docs/prototype/report.md` with numbers
and a go/no-go on the codegen architecture.

## Phase 1 — Compiler skeleton (sequential)

### Task 4: Toolchain skeleton

Zig project, single binary exposing `build run test fmt doc lsp`
(stubs are fine), CI-shaped build.

### Task 5: Frontend core

Lexer, parser, AST for the core subset: literals, functions, control
flow, structs, `:=` locals, boundary annotations, `[v, err]` returns.

### Task 6: Typechecker v1

Boundary annotations checked, local inference inside bodies, dropped
error = compile error.

### Task 7: Codegen v1

Settle LLVM vs self-hosted (recommendation: LLVM). Emit native binaries
for hello-world-class programs.

### Task 8: Runtime core

Arena allocator; ownership metadata + checks (aliasing, dangling);
scope-contained traps + abort fallback; `unsafe` opt-out.

### Task 9: Errors + C FFI

`[v, err]` in-language semantics; `extern` declare/call for C ABI.

## Phase 2 — Parallel tracks (fan out in separate worktrees)

### Task 10: Coroutine transform production [critical path]

Reachability-based suspend set, state-machine codegen, single-thread
executor, scope join/cancel, channels with ownership transfer on send.

### Task 11: Stdlib wave 1 [parallel]

`fmt`, `io`, `os`, `testing`.

### Task 12: Stdlib wave 2 [parallel; after Task 11 interfaces]

`net/http`, `encoding/json`, `sync`, `time`.

### Task 13: Multithread executor + blocking pool [parallel; after Task 10]

Work-stealing thread pool; blocking-pool escape hatch for FFI/syscalls;
atomic borrow state in `Shared<T>`.

### Task 14: Generics [parallel; after Task 6]

Type parameters + constraint interfaces; shape-based instantiation
preferred, full mono acceptable.

### Task 15: LSP + fmt + doc [parallel]

Language server (hover, goto-def, diagnostics) in the toolchain binary;
formatter; doc generator.

### Task 16: OSS plumbing [parallel]

MIT LICENSE, CI matrix (Linux x86_64/aarch64, macOS), release workflow,
README, CONTRIBUTING.

## Phase 3 — Gate

### Task 17: Pilot apps

`examples/cli` — simple CLI. `examples/http-crud-sqlite` — HTTP CRUD
server backed by libsqlite3 via C FFI. Run in production-shaped
conditions.

### Task 18: v0.0.1 tag

Gate: both pilots run. Tag the release.
