# M3 — Unused imports are a compile error

## Problem

Today an import that's never referenced is silent dead weight — and
under the new model it also means a `require` that shouldn't exist.
The answer: `imported and not used`, per file, no
warnings. This is the user's primary "no bloat by neglect" guarantee.

## Design sketch

- Per file, each import must be referenced ≥once: `pkg.name` counts
  for the default name or `as` alias; each `.{name}` in a selective
  import must itself be used.
- Unused whole import → `error [check] imported and not used:
  "std/strings"` at the import span. Unused selective name →
  `imported and not used: "std/strings".{split}`.
- `import "path" as _` is the explicit side-effect import — legal,
  exempt, currently a no-op (no `init()` in oxl; accepted for parity
  so generated/future code can express intent).
- Usage test = any reference in the file's decls (fn bodies, types,
  globals) — the unit of usage is the file, not the decl.
- Port to `check.oxl`; new `tests/diag/` fixtures: unused import,
  unused selective name, `as _` exempt, same-import-used (negative).
- Companion behavior lives in `oxl mod tidy` (V3): it fixes the
  manifest side — dropping requires no package imports, adding the
  requires a bare `cannot find package` needs.

## Acceptance

- Diag corpus covers all four cases; seed + self-hosted checker agree
- The v0.2 migration commit compiles clean — i.e. the repo itself has
  no unused imports after M1 lands (check before committing)
- `as _` survives the corpus without error

## Depends on

M1 (import sites must be package-aware first — usage is qualified
name use, which only exists post-M1).

## Risks

- Selective-import accounting has an edge: `.{name}` where `name` is
  used only as a *type* (`json.JObj`) not a call — count any qualified
  ident/typename use, not just calls.
