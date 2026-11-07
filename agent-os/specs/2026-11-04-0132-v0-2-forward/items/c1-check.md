# C1 — `bootstrap/check.oxl` (typechecker in the language)

## Problem

The third self-host component: typecheck the AST that
`bootstrap/parser.oxl` produces (moved to a typed `Expr`/`Item`/`Ty`
enum tree once B1 lands). The checker's rules are all specified in
`docs/semantics/` — the port is a translation, not a design.

## Design sketch

Port `oxl/src/check.rs` rule-for-rule:

- `Env` scope stack over `std/collections` maps (B6)
- fn/struct/interface registration pass, then per-item checks
- `[v, err]` must-bind (dropped-error = compile error)
- `own`→`ref`/`mut` borrow checks + param restrictions on FFI
- generic call-site binding inference + constraint checking
- suspend-set transitive computation (drives A1's input too — this
  port must produce the same set, a good cross-check)
- diagnostics: `{file,line,col,kind,msg}` list — same shape the LSP
  already consumes

## Acceptance

- `oxl run bootstrap/check.oxl -- file.oxl` → diagnostics or
  "ok" — matching `oxl build`'s errors on the example corpus
- Self-application: `check.oxl` checks `parser.oxl` (the real AST
  corpus, ~450 lines) — error-free
- Differential test in `tests/run.sh`: same `.oxl` file, seed
  checker's diagnostics == self-hosted checker's diagnostics

## Depends on

- B1 (`enum`) — the AST as tagged union
- B2 (`match`) — the dispatch mechanism the port is written in
- B6 (maps) — env + fn tables

## Risks

- Rule drift: two checkers is a fork — mitigate with the differential
  test; the seed one stays the reference until C2 completes the chain.
- Volume: ~900 lines of checker to port — the largest single .oxl
  file written so far; will surface real DX gaps (that's the point —
  log them as they appear).
