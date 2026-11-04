# B2 — `match`

## Problem

The parser's if-chains work but the checker's kind-dispatch
(`ExprKind`, `Ty`, `Item` — dozens of variants) needs exhaustive
matching to stay readable and safe.

## Design sketch

```
match e.kind {
    ExprKind::Int(v) => handle_int(v)
    ExprKind::Bin{op, l, r} => handle_bin(op, l, r)
    _ => default_case()
}
```

- `match` on enum values: arms bind payload fields (positional `Int(v)`
  or named `{op, l, r}`); `_` wildcard.
- Exhaustive: no `_` and missing variants = compile error (the killer
  feature — new variants surface every site).
- Value-returning `match` as expression: `x := match e { ... }` — both
  arms must produce the same type.
- Non-enum `match` on int/bool/str constants: same syntax, constant
  patterns (v0.2 scope: literals only, no ranges/guards — ranges are a
  fast-follow).

## Acceptance

- All variants covered: compiles; missing one: error naming the variant
- Payload binding (positional + named) yields correctly-typed locals
- Expression-position `match` unifies arm types
- `bootstrap/parser.lang` rewritten to `match` where if-chains
  currently dispatch (proves the ergonomic gain, keeps CI honest)

## Depends on

- B1 (enum) — matching is meaningless without variants. B1 ships alone
  via `kind()`/`as` escape hatches; B2 completes the pair.

## Risks

- Low–moderate: pattern lowering to nested if-chains + bindings in the
  C backend is mechanical; exhaustiveness checking is a set-cover walk.
- Temptation to add guards/ranges — explicitly deferred to keep the
  feature honest-sized.
