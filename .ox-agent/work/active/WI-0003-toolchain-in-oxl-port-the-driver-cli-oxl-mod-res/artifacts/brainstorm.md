# WI-0003 brainstorm: toolchain-in-oxl (milestone T)

Discovery inventory: `context/references/toolchain-driver-inventory.md`
— ~5.4k Rust driver lines to port, compiler passes already importable
`.oxl` packages, three ADR-0004 language gaps (process exec, fs
primitives, LSP `check::Index`).

## Fork 1 — binary architecture

How does the `.oxl` driver reach the compiler passes?

| option | shape | tradeoff |
|---|---|---|
| **A. Monolith** (recommended) | `cmd/oxl` imports `bootstrap/lexlib`, `bootstrap/parser`, `bootstrap/check`, codegen-as-package → one binary | matches the "one toolchain binary" language principle verbatim; zero IPC; compile-mode checks run in-process (same as Rust today). Cost: requires the codegen lib-split (Fork 6) |
| B. Driver + subprocess | `oxl` shells out to `cmd/check`/`cmd/codegen` binaries | needs exec (have it anyway) but forces AST serialization across process boundary — the AST has no stable serialized form; inventing one for this is waste |
| C. Hybrid | in-process check, subprocess codegen | worst of both — still needs serialization for the check→codegen handoff |

Recommendation: **A**. The passes are already packages or a lib-split
away; composition is strictly less machinery than IPC.

## Fork 2 — where the new primitives live

| option | shape | tradeoff |
|---|---|---|
| **A. `std/os` + `std/os/exec`** (recommended) | `os.mkdir_all/remove_all/rename/temp_dir/getenv/exists/ls/fullpath` + `exec.cmd(name,args).run()/.output()` | Go-faithful (the established std pattern); fs funcs belong next to `os.open`; process control gets its own package because it's the heavy surface (spawn/wait/pipe/kill) |
| B. `std/sys` or `std/process` | new top-level packages | diverges from the std layout convention for no gain |
| C. runtime builtins | `exec()`/`mkdir()` globals | spreads the driver-port surface into the language core; wrong layer |

The runtime additions are small rt.c primitives (+ E3 Windows veneer
twins): `posix_spawn` or fork/execvp+waitpid+pipe on POSIX,
`CreateProcess`/`WaitForSingleObject` on Windows; `mkdir`(parents),
`rename`, `rmdir`/recursive-remove helper. FFI-widened signatures may
be needed (argv as `[]str`, env handling).

## Fork 3 — staging order (critical path)

Dependency order is mostly forced:

1. **T0 — exec + fs primitives** (rt.c + `std/os`/`std/os/exec`) —
   *everything* below needs them.
2. **T1 — codegen lib split** — `bootstrap/cmd/codegen/main.oxl` (5327)
   → `bootstrap/codegen/` package + thin main; pure move, unblocks the
   monolith.
3. **T2 — core loop: build/run/test** — manifest/module resolution
   (port of the M2 machinery + MVS + modcache read paths), compile_mode
   orchestration over imported passes, `emit_and_cc` via exec, test
   binary via exec. The dogfood gate can start here: self-built `oxl`
   compiles the suite.
4. **T3 — fmt + doc** — AST consumers; fmt is the biggest single port
   (984 lines over a *different* AST — `parser.Node` vs Rust AST).
5. **T4 — get/mod** — fetch veneer + modcache write paths + `oxl.sum`.
6. **T5 — LSP** — needs `check::Index` ported into check.oxl. Largest
   and least coupled; last on purpose.

## Fork 4 — LSP in or out of T (preserve disagreement)

ADR-0004 names LSP in scope. But its true cost is the `check::Index`
port — machinery *inside* the checker (position→symbol map built
during checking), which brushes the "driver only" boundary the ADR
drew.

- **Keep in T** (recommended): it's listed in the milestone scope and
  the index is driver-facing data, not a pass change — check.oxl gains
  a read-only index product alongside diagnostics. Sequenced last so a
  slip doesn't gate the core loop.
- **Defer to T-follow-up**: if the index port balloons, it becomes its
  own WI without renegotiating T's exit criteria — *if* the exit
  criterion is phrased as driver-commands parity, not command-list
  completeness.

Decision needed at specify: phrase the exit criterion so T5 can split
cleanly if the Index port outgrows the milestone.

## Fork 5 — the bootstrap chain & what happens to the Rust driver

The exit criterion says "cargo out of the build path." Options:

| option | shape | tradeoff |
|---|---|---|
| **A. Seed stays, product is .oxl** (recommended) | `oxl/` crate remains as the documented bootstrap seed; CI builds product `oxl` via seed → .oxl path; suite runs under the self-built driver | honest bootstrap semantics (Go did exactly this with the C→Go handoff); zero artifact-committing; seed bit-rot guarded by CI still building it |
| B. Seed deleted, pinned binary committed | ship a frozen `oxl` per platform | binary-in-repo is fragile + unreviewable; new-contributor path breaks |
| C. Dual-driver CI forever | run suite under both drivers | doubles suite time; value is parity-checking the *seed*, which we no longer care about once product is .oxl — except during the transition it IS the parity net |

Transition detail: the fixpoint proof is C-text equality — the
self-built driver's codegen emits the same C for the driver sources as
the seed's (same gate the compiler bootstrap already passes). Binary
hashes won't match (build-ids/paths); C text will.

## Fork 6 — codegen lib split mechanics

`bootstrap/cmd/codegen/main.oxl` embeds the emitter (5327 lines).
Options: (a) move logic to `bootstrap/codegen/codegen.oxl` (`package
codegen`) + thin main — mirrors the `check/` layout, mechanical, keeps
the cmd binary working for the self-compile gate; (b) driver absorbs
the emitter source directly — duplicates 5k lines. **(a)**.

## Open questions for specify

- New driver main location: `bootstrap/cmd/oxl/` (keeps bootstrap
  self-contained) vs `cmd/oxl/` at root (signals "this is the product,
  not a bootstrap artifact")? — lean `cmd/oxl/`: the bootstrap tree is
  for compiler passes; the driver is the product.
- Does `oxl test` need `-- `-forwarded argv for test binaries (the S3
  flag-parse gap surfaced this) or keep current behavior?
- exec API: Go's `exec.Cmd` struct or simpler `run()/output()` fns?
  Lean minimal (two fns) — oxl's std convention is honest-thin.
- `oxl get` git exec — allow `git` on PATH only (current Rust behavior)
  or vendored fallback? — PATH only, matches documented design.
