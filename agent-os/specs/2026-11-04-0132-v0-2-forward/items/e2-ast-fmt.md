# E2 — AST-printer formatter

## Problem

`lang fmt` is a whitespace/indentation normalizer — honest but blind:
it can't canonicalize expression layout (extra parens, operator
spacing), can't wrap long lines, and doesn't reorder anything.
`gofmt`-class formatting is table stakes for adoption ("no arguments
about style" is a Go lesson worth copying).

## Design sketch

- Pretty-print from the AST (comment attachment is the only hard part —
  comments are currently dropped by the lexer; they must be retained
  as trivia attached to the following token)
- Emit canonically: 4-space indent, single blank line between items,
  `op` spacing normalized, `{`-on-same-line (the repo's existing
  style), trailing commas dropped
- `-w` rewrite, `--check` exits nonzero on diff (CI-friendly)
- The self-hosted parser's AST is a candidate emitter source —
  writing `bootstrap/fmt.lang` in `.lang` once C1 lands is the
  self-host-friendly path; a Rust-side printer over `ast.rs` is the
  faster path. **Decide at implementation** — spec recommends Rust-
  side first (the self-hosted AST is JSON-shaped until B1/B2 land).

## Acceptance

- `lang fmt examples/features.lang` — idempotent (fmt(fmt) == fmt)
- Comments survive in place; string literals byte-exact
- `--check` in CI asserts the whole repo is formatted

## Depends on

- Nothing hard; easier with the merged-crate AST the checker already
  produces. If deferred past B1/B2, doing it in `.lang` becomes the
  better path (self-host dogfooding).

## Risks

- Comment-trivia attachment is ~half the work (every formatter's
  known cost); the other half is pure pretty-printing.
