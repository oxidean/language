# S5 — `std/regexp`

## Problem

The model is a linear-time NFA — no backreferences, no exponential
blowup. The oxl-scale
version is a Thompson NFA engine: same guarantee, honest scope.

## Design sketch

- Pure-.oxl implementation: parse → compile to NFA → simulate.
- Syntax subset — NFA-shaped, deliberately: literals, `.`, classes
  `[a-z]`, `^`/`$`, `*`/`+`/`?`, `{n,m}`, groups `(...)` + `|`,
  escapes. **No** backrefs, lookahead/lookbehind, or lazy quantifiers
  in v1 (lazy `*?` is cheap — include if trivial, defer if not).
- API: `regexp.compile(pat) -> [Re, err]`,
  `re.matchstr(s)`, `re.findall(s, n)`, `re.replaceall(s, repl)`,
  `re.split(s, n)`.
- Linear-time guarantee: NFA simulation is O(len(pat) × len(input)) —
  no exponential blowup — the property that matters.

## Acceptance

- `regexp` package passes a ported regexp test corpus
  (the subset that uses supported syntax — cherry-pick ~50 cases)
- Pathological pattern (`(a|a)*$` on `"aaaa...b"`) completes in
  linear time — that guarantee, asserted by timeout not timing
- Both codegens compile + run it — the engine exercises maps,
  slices, fn values, generics all at once

## Depends on

S1. Consumes `std/strings`, `std/collections` — nothing else.

## Risks

- Engine correctness is subtle — the acceptance corpus *is* the
  spec; don't ship without the pathological-case test.
