# WI-0003 spec: toolchain-in-oxl (milestone T)

Port the Rust driver under `oxl/src/` (~5.4k lines: `main.rs`,
`fmt.rs`, `modcache.rs`, `semver.rs`, `fetch.rs`, `diag.rs`) to `.oxl`,
producing a single `oxl` binary built from `.oxl` sources that passes
the full suite as the driver. Compiler passes are dependencies, not
work (ADR-0004, ADR-0005).

## Scope

Deliverable is the milestone itself, staged as sequential PRs (stage
boundaries in `plan.md`). This WI's *planning* increment — spec +
staged plan + ADRs — ships as its own docs PR first, matching the
WI-0002 precedent.

### In scope

1. **`std/os` + `std/os/exec`** (ADR-0007): `mkdir_all`, `remove_all`
   (recursive), `rename`, `temp_dir`, `getenv`, `exists`, `isdir`,
   `ls`, `fullpath`; `exec.cmd(name, args)` → `run() -> [int, err]`
   (child exit status + spawn error — status recoverable, spawn
   failure distinguishable) and `output() -> [Out, err]` with
   `Out{stdout, stderr, status}` (both streams captured; spawn
   failure is the only `err`).
   Backing rt.c primitives: POSIX `posix_spawn`/fork+execvp+`waitpid`
   +`pipe`, Windows `CreateProcess`/`WaitForSingleObject`/`CreatePipe`
   via the E3 veneer; `mkdir`, `rename`, recursive-remove.
2. **Codegen lib split** (ADR-0005): `bootstrap/cmd/codegen/main.oxl`
   → `bootstrap/codegen/codegen.oxl` (`package codegen`) + thin main.
3. **`cmd/oxl/`** — the driver main (`package main`), at repo root
   (product, not a bootstrap artifact). Imports lexlib/parser/check/
   codegen + std. Command parity with the Rust CLI:
   - `build [dir|file] [-o out]`, `run`, `test` — module+file modes,
     `compile_mode` orchestration over imported passes, `emit_and_cc`
     via `os/exec` with the proven flag ordering (sources → rt.c →
     `-lsqlite3`/`-lm`/`-pthread`/`--gc-sections` platform set), scratch
     dirs via `temp_dir`, test binary via `exec.run`, exit-code passthrough.
   - `fmt [--check] [-w]` — AST printer over `parser.Node` (port of
     `fmt.rs`, 984 lines — the one surface where the ASTs differ).
   - `doc` — `///` extraction (line-scan port, ~60 lines).
   - `get`, `mod init/tidy/verify/list` — manifest parse, MVS build
     list, `~/.oxl/mod` cache (`OXLMODCACHE` override honored), git
     fetch via `exec.output` (`clone --bare` + `git archive`), `oxl.sum`
     `h1:` verify.
   - `lsp` — JSON-RPC `Content-Length` stdio loop; `initialize`,
     `didOpen`/`didChange`, `hover`, `definition`, `publishDiagnostics`
     with per-scope clearing parity. Position columns are **UTF-16
     code units** (the LSP default `initialize` doesn't negotiate) —
     ported conversions between protocol cols and internal byte
     offsets, exercised by a non-ASCII session fixture. Requires
     porting `check::Index` (`uses` use-site records + `decls`
     binding-site spans) into check.oxl — read-only index product,
     not a pass change.
4. **Module resolution parity** — the M2 machinery in `.oxl`:
   manifest discover/parse (`module`/`oxl`/`require`/`replace`), std
   anchor (exe-walk), internal/ rule, cycle diagnostics, file-mode
   module-off — byte-identical diagnostics vs the seed (the existing
   diag corpus is the parity net).
5. **Dogfood CI wiring** — suite gains a self-build section: seed
   builds `cmd/oxl`, the suite runs under it; fixpoint asserted as
   C-text equality on the driver sources (ADR-0006).

### Out of scope (explicit)

- Compiler passes, `rt.c` redesign beyond the T0 primitives, stdlib
  growth beyond the listed packages.
- New features/flags — command surface is parity, not extension
  (e.g. no `oxl test -- `-argv forwarding; that stays a follow-up).
- Registry (milestone R), vendored-git fallback, alternate exec APIs
  beyond `run`/`output`.

## Decisions resolved at brainstorm (now contracts)

- **Monolith** (ADR-0005): one binary, passes imported as packages.
- **Seed stays** (ADR-0006): `oxl/` crate = documented bootstrap seed;
  "cargo out of the build path" = out of the *product* path.
- **Primitives placement** (ADR-0007): `std/os` + `std/os/exec` only.
- **Driver location**: `cmd/oxl/` at root.
- **`git` on PATH only** for fetch — matches documented V2 design.
- **LSP stays in T** but sequenced last; exit criterion below is
  phrased so the Index port can split to a follow-up WI without
  reopening this spec if it outgrows the milestone.

## Acceptance criteria

Milestone exit (phase-2 map) + per-stage gates (in `plan.md`):

- `cmd/oxl` builds via the seed into a working `oxl` binary; every
  CLI command behaves identically to the Rust driver's on the shared
  corpus (same stdout/stderr/exit codes — extending the existing
  diag-parity harness pattern to driver outputs).
- `tests/run.sh <self-built-oxl>` fully green — including the
  bootstrap chain (the new driver must compile check.oxl byte-
  identically), the diag corpus, module fixtures, and fmt checks.
- C-text fixpoint: seed-emitted and self-emitted C for the `cmd/oxl`
  sources are byte-identical.
- `oxl get`/`mod` round-trip verified against a local git remote in
  the suite (clone/archive/oxl.sum-verify path exercised).
- `oxl lsp` answers `initialize`/`hover`/`definition` and publishes
  diagnostics in a scripted stdio session (fixture in `tests/`).
- CI green on ubuntu/macos/windows; commit-lint clean; CodeRabbit
  approved on the final PR.
- If T5 (LSP) splits: milestone exit becomes "all commands except
  `lsp` at parity" **and the suite gate is capability-scoped** — the
  lsp section probes driver support and skips under split mode, so
  `tests/run.sh <self-built-oxl>` still passes while the Index port
  moves to its own WI with the same acceptance bar. The split
  decision is recorded in `plan.md` at T5 start, not silently.

## Non-goals

- Performance work on the driver (parity first; measure after).
- Windows-first authoring — but every primitive ships its E3 veneer
  twin; no POSIX-only shortcuts.
- Rewriting `tests/run.sh` beyond the self-build/dogfood sections.
