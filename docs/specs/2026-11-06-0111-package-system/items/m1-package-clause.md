# M1 — `package` clause + directory packages

## Problem

A module is a *file*. `load_mod` qualifies decls by file stem, so a
"package" can never span files, `bootstrap/` can never hold 4 related
tools, and import paths leak filesystem detail. The unit is the
directory; ours has to match for `std/` to restructure (S1) and for
`oxl.mod` (M2) to have something to point at.

## Design sketch

- Grammar: `package` joins the keyword table; the clause
  `package <ident>` is the mandatory first item of every `.oxl`
  (before `import`s). Parser: missing clause → `expected package
  clause`; clause after other items → error.
- All `.oxl` in one directory = one package; every clause must match,
  and must equal the directory name (except `package main` anywhere a
  command lives). Mismatch → `found packages a and b in <dir>`.
- Import path → directory: load every `.oxl` in the resolved dir as
  one package. Bound local name = **declared package name**, not file
  stem — `load_mod`'s `mod_stem` is replaced by clause-name from the
  first file; `as` alias and `.{selective}` unchanged in behavior.
- Same-package files share one namespace: no `pkg.` qualification
  between them; redeclarations across files = the existing
  duplicate-decl error.
- `package main` + `fn main` = command. `oxl run x.oxl` on a loose
  file requires `package main`; multiple file args = all same dir +
  same package (the directory is the unit).
- No decl-level visibility — every decl is visible to importers
  (shape.md explains why capitalization can't carry it; `internal/`
  in M2 is the encapsulation tool).
- CLI surface: `oxl build`/`run`/`test`/`fmt`/`doc`/`lsp` all switch
  from file paths to package paths — arg that resolves to a `.oxl`
  file = file mode (package main only); arg resolving to a dir =
  package mode; bare `oxl build` = cwd package.
- Self-host ports: `parser.oxl` parses the clause, `check.oxl`
  enforces dir-name/mismatch rules, both must resolve imports through
  the new loader — the differential corpus keeps them honest.

## Migration (one commit, D5-style)

- Every in-tree `.oxl` gets a clause; `stdlib/*.oxl` move to
  `std/<name>/` dirs (S1 owns that exact layout — either land
  together or sequence M1 then S1 in the same PR).
- `bootstrap/`: `lexlib.oxl` → `bootstrap/lexlib/` (package lexlib);
  each main → `bootstrap/<tool>/main.oxl` (package main); internal
  imports become module-path imports.
- `tests/`, `examples/`, repo `oxl.mod` (`module
  github.com/oxidean/oxl` — dogfoods real paths for M2).

## Acceptance

- `tests/run.sh` green post-migration, including the bootstrap
  self-compile chain and the `tests/diag/` corpus
- Fixture: two-file package callable from a third package; names
  shared without qualification inside the package
- Diagnostics in corpus: missing clause, clause-not-first, mixed
  package names in one dir, dir-name mismatch
- `oxl run hello.oxl` (single file, `package main`, std imports only)
  works with **no** `oxl.mod` present

## Depends on

Nothing — it IS the foundation. Do it first and do the migration in
one commit.

## Risks

- Wide diff (every file) — mechanical, but conflicts with any open
  branch; land it while the tree is quiet.
- `load_mod` currently keys everything on file stem — the rename is
  invasive in `main.rs` and `check.oxl`'s mirror logic; budget for
  both sides in the same task.
