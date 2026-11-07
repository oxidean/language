# Package System + Stdlib — Shaping Notes

## Scope

Versioned package management and a batteries-included core library,
spec'd to
execution depth:

- `package` clauses and directory packages (import path → directory,
  not file) — the one breaking surface change
- `oxl.mod` / `oxl.sum` manifests, module-aware import resolution,
  `internal/` visibility, the `std` toolchain pseudo-module
- Full versioned-module model: semver tags, pseudo-versions, MVS build
  list,
  git-based fetch into a module cache, `oxl get` / `oxl mod`
- Unused imports as a hard compile error (per-file — an import nothing
  references fails the build)
- Dead-code elimination: call-graph reachability in both codegens +
  linker section GC for `rt.c` — unused packages must not bloat output
- `std/` restructured into a real package tree and grown toward a
  full core library
  standard library, bounded by what the type system can express

## Decisions

Product decisions are governed by
`docs/specs/2026-11-03-language-design.md`, with one
explicit revision: the design doc's "no package ecosystem/registry
beyond a minimal module story" (§1 line 29, §6 line 207) is superseded
here — the roadmap (`docs/product/roadmap.md`, Phase 2) already
lists "Package registry / module versioning", and this spec executes
it at full fidelity where it makes sense for oxl.

Locked in shaping discussion (2026-11-06):

1. **Directory packages + `package` clause** — every
   `.oxl` declares `package <name>`; an import path names a directory;
   the bound local name is the declared package name, not a file stem.
   `package main` marks executables.
2. **Full versioned-module model** — semver `vX.Y.Z`, pseudo-versions
   for
   commit pins, MVS (minimal version selection, no SAT solver),
   `/vN` path suffix for majors ≥ 2, `replace` for local dev.
3. **Loose files keep working** — `oxl run x.oxl` still works
   outside any module when the file is `package main` and only imports
   `std`; `oxl build` on a directory requires `oxl.mod`.
4. **Unused imports are a compile error** — per-file usage, matching
   build. Plus reachability-based dead-code elimination so unused
   machinery never reaches the binary.
5. **Stdlib targets a full core library** —
   bounded honestly: packages whose API needs reflection (`fmt`-style
   `%v` formatting, `reflect`, `encoding/gob`), `unsafe.Pointer`
   tricks, or a whole TLS/X.509 stack are documented bounds or parked,
   not silently dropped. See `items/parked-stdlib.md`.

Deliberate divergences from the model:

- **No decl-level visibility in v1** — capitalization can't carry it
  (oxl types are `StrList`-style capitalized already). Package
  boundaries + `internal/` dirs are the encapsulation mechanism.
  A `private` keyword can land later without breaking anything.
- **No package `init()`** — no top-level side effects in the language;
  `as _` imports are accepted for parity but are no-ops.
- **`context` package is a non-goal** — `scope` blocks + structured
  cancellation already fill that role better.
- **`oxl.mod` + `oxl.sum`** — the manifest/lockfile pair, same grammar
  and semantics as the established versioned-module model and
  hash-line shape, renamed.

## Context

- v0.2.0 released; E3 Windows + C2 iface codegen merged; suite green
  on `main` (`dcebc6b`).
- Today's machinery: `resolve_import` / `load_mod` in `oxl/src/main.rs`
  (file-stem modules, alias/selective maps from D5), 8 flat files in
  `stdlib/`, 86 `.oxl` files in-tree, no manifest/cache/versions.
- Both checkers and both codegens must stay in lockstep — every item
  lists its self-host port where one exists.
- Migration scale: every `.oxl` in repo + stdlib call sites. Bundle the
  clause/relayout into one commit like D5 did.

## Standards Applied

- `global/language-principles` — applies to every item
