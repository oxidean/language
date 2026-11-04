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

### Task 11: Stdlib wave 1 [done]

Module system landed: `import "path"` / `import "std/x"` (merged
namespace, cycle-safe, `stdlib/` lookup from ancestors). Stdlib is
written in the language: `stdlib/strings.lang` (startswith/endswith/
contains/trim/split→own StrList/join/upper), `stdlib/testing.lang`
(expect/expect_eq/expect_streq), `stdlib/http.lang` (serve + user
route()). Builtins added: `readfile`, `writefile`, `exit`, `mkshared`,
`sget`, `sset`. Wave-1 `io`/`os` coverage comes via file builtins;
dedicated `os` module still open.

### Task 12: Stdlib wave 2 [partial]

`net/http` exists as `stdlib/http.lang` (socket externs, accept loop,
`spawnBlocking` per conn, user `route()` callback) — pilot rewritten on
it. `encoding/json`, `sync`, `time` still open.

### Task 13: Multithread executor + blocking pool [done]

Work pool: N=nproc workers on a shared MPMC queue; scopes join by
pending-task count (same protocol the SM executor will keep). Blocking
pool via `spawnBlocking` (thread-per-task, FFI-safe). `Shared<T>` atomic
word cell: `mkshared/sget/sset`. Fixed a lost-wakeup race in unbuffered
channels the pool exposed (broadcast wakeup discipline).

### Task 14: Generics [done]

`fn f[T constraint](...)` — monomorphized per call-site binding set
(`l_f_name__T` instantiations), constraints: `any`, `eq`, `ordered`,
`add` + named `interface` decls. Type params bind through own/ref/mut.
Ops inside generic bodies require the matching constraint (e.g. `<`
needs `ordered`).

### Task 15: LSP + fmt + doc [done — v0 quality]

`lang lsp`: real `publishDiagnostics` on didOpen/didChange (full-text
sync, imports resolved). `lang fmt`: indentation/whitespace normalizer
(comment-preserving, `-w` to rewrite). `lang doc`: `///` comments on
decls → markdown. Hover/goto remain open (needs position index).

### Task 16: OSS plumbing [done]

LICENSE (MIT), README, CONTRIBUTING, `.github/workflows/ci.yml`
(build + `tests/run.sh` end-to-end suite). Release workflow still open
(waits on the v0.0.1 tag).

## Phase 3 — Gate

### Task 17: Pilot apps [done — load tested]

`examples/cli`: works. `examples/http-crud-sqlite`: rewritten on
`std/http` + `spawnBlocking` per conn. Load test: 300 concurrent GETs
all 200s, RSS ~2.4 MB. Writes: 30/30 serial; ~78% under 20-way
concurrent POSTs — residual is sqlite multi-connection write contention
(per-request `sqlite_open`, no connection pooling in v0 surface);
retry loop + WAL pragma added to rt shims. Channel lost-wakeup race
found + fixed under load. `bootstrap/lexer.lang` — lexer in the
language — is a pilot too: lexes itself (955 tokens) + runs in CI.

### Task 18: v0.0.1 tag [not started]

Gate: both pilots run. Tag the release.
