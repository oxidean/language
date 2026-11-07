# Language Foundation — Implementation Plan

Built from the approved design doc:
`docs/specs/2026-11-03-language-design.md`

Execution model: sequential critical path through Phase 0–1, then
parallel worktree tracks in Phase 2. Every task lands behind the
`language-principles` standard.

**Status (2026-11-05): complete — v0.0.1 tagged.** Phase 0–2 all done.
The compiler compiles all `examples/` to native binaries and they run —
including the HTTP CRUD + SQLite and GTK pilots. Implementation language
is Rust (the settled seed language); codegen
backend is C emit → cc (documented interim, swappable for LLVM behind
the `interfaces.md` seam). Deferred items — including the state-machine
transform opened under Task 10 — were spec'd and landed under
`../2026-11-04-0132-v0-2-forward/plan.md`.

## Task 1: Save spec documentation [done]

Create `docs/specs/2026-11-03-0036-language-foundation/` with
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

`oxl/` (seed toolchain): `build run test fmt doc lsp`. build/run/test work;
fmt/doc are honest stubs; lsp is a stdio JSON-RPC scaffold (initialize/
shutdown/exit).

### Task 5: Frontend core [done]

`oxl/src/{lexer,ast,parser}.rs` — full v0 subset: literals, fns,
structs, extern decls, `let`/`:=`, `[v, err]` binding, if/else, for,
scope/spawn/go, unsafe, struct literals, comments.

### Task 6: Typechecker v1 [done]

`oxl/src/check.rs` — boundary annotations enforced, `:=` local
inference, dropped-error compile error, own→ref/mut borrow conversion,
FFI arg restrictions, suspend-set computation.

### Task 7: Codegen v1 [done]

`oxl/src/codegen.rs` — C emit → cc → native binary. Interim backend
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

### Task 10: Coroutine transform production [done — via v0.2 A1]

Surface semantics final and working (scope join/cancel, channels,
suspend set computed in check.rs). The state-machine transform opened
here landed under v0.2 Task A1: suspend-capable fns compile to
`o_frm_*`/`o_poll_*` stackless frames (~96 B) driven by the executor —
see `docs/semantics/concurrency.md`.

### Task 11: Stdlib wave 1 [done]

Module system landed: `import "path"` / `import "std/x"` (merged
namespace, cycle-safe, `stdlib/` lookup from ancestors). Stdlib is
written in the language: `stdlib/strings.oxl` (startswith/endswith/
contains/trim/split→own StrList/join/upper), `stdlib/testing.oxl`
(expect/expect_eq/expect_streq), `stdlib/http.oxl` (serve + user
route()). Builtins added: `readfile`, `writefile`, `exit`, `mkshared`,
`sget`, `sset`. Wave-1 `io`/`os` coverage comes via file builtins;
dedicated `os` module still open.

### Task 12: Stdlib wave 2 [done — v0 surface]

`net/http` exists as `stdlib/http.oxl` (socket externs, accept loop,
`spawnBlocking` per conn, user `route()` callback) — pilot rewritten on
it. `stdlib/json.oxl` — Json value tree, `tojson` serializer,
`jparse` recursive-descent parser, `jget` member lookup (ints only,
`\" \\ \n \t` escapes). `stdlib/sync.oxl` — spin Mutex on `scas`,
atomic `counter`/`cinc`/`cval` on `sadd`. `stdlib/time.oxl` —
`unix`/`unix_ms`/`sleep_ms`. Top-level `let` globals (process-wide
state, initialized in `oxl_main` before user main) — the pilot uses
one shared sqlite conn across all request tasks. New builtins:
`scas`, `sadd`, `now`, `now_ms`, `usleep`. Dedicated `os` module and
float JSON numbers remain open.

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

`oxl lsp`: real `publishDiagnostics` on didOpen/didChange (full-text
sync, imports resolved). `oxl fmt`: indentation/whitespace normalizer
(comment-preserving, `-w` to rewrite). `oxl doc`: `///` comments on
decls → markdown. Hover/goto remain open (needs position index).

### Task 16: OSS plumbing [done]

LICENSE (MIT), README, CONTRIBUTING, `.github/workflows/ci.yml`
(build + `tests/run.sh` end-to-end suite), `.github/workflows/release.yml`
(tag-triggered toolchain tarball). The `v0.0.1` release run failed on the
runner's missing sqlite link flags — fixed post-tag via conditional link
flags; publishing awaits the next tag.

## Phase 3 — Gate

### Task 17: Pilot apps [done — load tested]

`examples/cli`: works. `examples/http-crud-sqlite`: rewritten on
`std/net/http` + `std/encoding/json` + `std/strings`, serves real JSON
(`{"items":[...]}`), one shared sqlite conn via a `let` global —
sqlite's serialized mode does the locking. Load test: 300 concurrent
GETs all 200s, RSS ~2.4 MB; writes: 30/30 serial **and 100/100 at
20-way concurrency** (the earlier ~78% was multi-connection contention,
now gone). Bugs found and fixed under this hardening: channel
lost-wakeup race, task-trap never decrementing scope pending (join
hang), borrows never released (`rt_unborrow` now emitted post-call),
reborrow-vs-fresh-borrow distinction, main-thread traps silently
exiting 0 (now loud abort), parser spinning on unrecoverable items
(guaranteed-progress skip). `bootstrap/lexer.oxl` — lexer in the
language — is a pilot too: lexes itself (955 tokens) + runs in CI.

### Task 18: v0.0.1 tag [done]

Gate: both pilots run. Tag the release. (`v0.0.1` tagged.)
