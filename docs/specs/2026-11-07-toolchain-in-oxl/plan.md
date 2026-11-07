# WI-0003 plan: toolchain-in-oxl (milestone T)

Spec: `spec.md` (sibling). ADRs: 0005 (monolith), 0006 (seed +
C-text fixpoint), 0007 (std/os + std/os/exec). Every stage is its own
PR through the loop (branch → suite green → CodeRabbit approved →
squash-merge, `type(scope):` subjects, no attribution).

Leverage found at discovery: `bootstrap/check` already exports the
module machinery (`mod_ctx_load`, `mod_build_list` MVS, `mod_h1` sum
hashing, `resolve_imp`, `load_pkg`) — `cmd/check` imports it as a
package today. The driver reuses it rather than re-porting; what
remains driver-side is CLI, compile orchestration, exec/scratch, the
AST consumers, and the fetch/cache write paths.

## Stage 0 — planning docs PR (this PR)

- Copy spec to `docs/specs/<date>-toolchain-in-oxl/` (archive
  convention); WI artifacts live under `.ox-agent/work/` (tracked —
  `runtime/` only is ignored).
- Files: spec.md, plan.md, ADR-0005/0006/0007, brainstorm.md,
  `context/references/toolchain-driver-inventory.md`; link from
  `docs/specs/2026-11-07-phase-2-plan/plan.md` T section.
- Verify: markdown links resolve; commit-check clean; docs-only diff
  → CI + CodeRabbit → merge.

## Stage T0 — exec + fs primitives (done)

- `runtime/rt.c`/`rt.h`: `rt_mkdir`, `rt_rename`, `rt_proc_run`
  (spawn+wait, inherited stdio), `rt_proc_output` (spawn+pipe
  capture+wait) — POSIX `posix_spawn`/`waitpid`/`pipe`; Windows
  `CreateProcess`/`WaitForSingleObject`/`CreatePipe` behind the E3
  veneer. Quoting rules: argv array passthrough, never a shell string.
- `std/os`: `mkdir_all`, `remove_all`, `rename`, `temp_dir`, `getenv`,
  `exists`, `isdir`, `ls`, `fullpath` (wrapping existing externs +
  new ones).
- `std/os/exec`: `cmd(name, args)` / `cmd_env(name, args, env
  []str)` — env `K=V` pairs override the inherited environment
  (`TZ=UTC`, `GIT_TERMINAL_PROMPT=0` for the fetch veneer). `run() ->
  [int, err]` returns child exit status; `output() -> [Out, err]`
  returns `Out{stdout, stderr, status:int}`; `err` covers
  spawn/capture/wait failures while a nonzero child exit stays a
  status, never an `err` — `oxl run` propagates exit 42 losslessly
  and `git archive` bytes can't silently truncate.
- Tests: `tests/exec_test.oxl`, `tests/fsops_test.oxl` + run.sh
  section; Windows leg exercises the veneer.
- Deps: none. Gate: suite green incl. windows.

## Stage T1 — codegen lib split

- Move emitter from `bootstrap/cmd/codegen/main.oxl` (5327) to
  `bootstrap/codegen/codegen.oxl` (`package codegen`); cmd becomes a
  thin main importing it — mirrors `check/` + `cmd/check`.
- Update run.sh self-compile paths if they reference the cmd layout.
- Gate: byte-identical self-compile chain unchanged — pure move.
- Deps: none (parallel-safe vs T0, order them anyway — T0 keeps the
  diff narrative clean).

## Stage T2 — cmd/oxl core: build/run/test

- `cmd/oxl/` (root, `package main`): argv dispatch; `pkg_args`/
  `pkg_src`; manifest discover/parse for the *driver* surface
  (`module`/`oxl`/`require`/`replace` — reuse `check`'s `mod_ctx_load`
  for resolution; driver needs root/build-list for the file set);
  `compile_mode` orchestration over imported check+codegen;
  `emit_and_cc` via `exec.run` with proven flag order (sources → rt.c
  → libs last: `-lsqlite3`/`-lm`/`-pthread`/`--gc-sections`, Windows
  set); `mk_scratch_dir` via `temp_dir`; `test` = build `--test` +
  `exec.run` + exit passthrough; diag renderer port (`diag.rs`, 119).
- Parity corpus: extend the `tests/diag/` harness pattern to driver
  outputs — seed vs self-driver stdout/stderr/exit-code identical on
  mod_* fixtures, file/module modes, error paths.
- Gate: self-built `oxl` builds/runs/tests the suite's core sections;
  parity corpus green.
- Deps: T0, T1.

## Stage T3 — fmt + doc

- `fmt [--check] [-w]`: port `fmt.rs` (984) over `parser.Node` AST —
  the one place ASTs differ; preserve comment attachment + canonical
  line breaking exactly (round-trip, not reformat opinion).
- `doc`: line-scan `///` extraction port (~60 lines).
- Tests: `fmt -d` clean on the repo corpus + idempotent second pass;
  `doc` output byte-identical vs seed on std corpus.
- Deps: T2 (driver skeleton exists).

## Stage T4 — get/mod subcommands

- `get`: git veneer via `exec.output` — `clone --bare` tmp + `git
  archive` extract (port `fetch.rs`, 171); `OXLMODCACHE` honored.
- `mod init/tidy/verify/list`: manifest rewrite (tidy diffs requires
  vs imports), `~/.oxl/mod` layout, `oxl.sum` `h1:` verify — `mod_h1`
  exists in `check` pkg for reuse.
- Tests: suite gains a local-git-remote fixture (`git init --bare`
  in scratch → `oxl get path@ref` → build → `mod verify`) — no network.
- Deps: T0 (exec), T2.

## Stage T5 — LSP

- `check` pkg: port `check::Index` — `uses` use-site records +
  `decls` scope-parallel binding spans (Rust `check.rs:119-160`,
  ~150 lines) as a read-only checking product.
- `cmd/oxl lsp`: JSON-RPC `Content-Length` stdio loop; `initialize`,
  `didOpen`/`didChange`, `hover`, `definition`, `publishDiagnostics`
  with per-scope clear. Columns are **UTF-16 code units** — port the
  seed's col↔byte-offset conversions; scripted fixture includes
  non-ASCII text.
- Tests: scripted stdio fixture — framed requests in, expected
  responses out (`tests/lsp_session.oxl` or run.sh heredoc).
- **Split gate**: if the Index port outgrows the stage, record the
  split decision here + file a WI; milestone exit becomes
  all-commands-but-`lsp` parity, and the suite's lsp section becomes
  capability-gated (probe `oxl lsp`, skip when absent) so the dogfood
  run stays green per the spec's contingency.
- Deps: T2 (uses module machinery + analyze path).

## Stage T6 — dogfood CI + docs

- `run.sh` self-build section: seed builds `cmd/oxl` → suite runs
  under the product driver end-to-end (the milestone exit gate).
- Fixpoint assert: seed-emitted vs self-emitted C for `cmd/oxl`
  sources byte-identical (not binary hash — ADR-0006).
- Docs: `docs/self-hosting.md` + CONTRIBUTING updated to describe the
  seed/product split; `oxl/` crate docstring marks it as the seed.
- Gate: full suite green under self-built driver on all 3 platforms;
  CodeRabbit approved; milestone T exit criterion met.

## Risks

- **fmt over `parser.Node`** is the riskiest port — different AST
  shape than fmt.rs assumed; mitigate with the round-trip gate and a
  corpus-diff harness before wiring `-w`.
- **Windows exec veneer**: `CreateProcess` takes a command *line*, not
  argv — needs correct quoting (embedded spaces/quotes); test fixture
  must include an arg with spaces.
- **Parity drift in diags**: driver-level errors must match the seed
  byte-for-byte (learned at M2: no canonicalization in message-visible
  paths; `\\?\` prefixes break parity on Windows).
- **`check` pkg reuse boundary**: driver's manifest *rewrite* (tidy)
  is new code — the checker only reads manifests.
- **Suite time** grows with the self-build section — keep it to one
  build + the parity corpus, not a second full pass.
