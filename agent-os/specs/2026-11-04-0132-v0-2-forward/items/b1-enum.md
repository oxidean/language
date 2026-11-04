# B1 — `enum` tagged unions [highest leverage]

## Problem

The AST is a tagged-union problem. `bootstrap/parser.oxl` models it
with `std/json` nodes and kind ints — works, but verbose and
type-unsafe. `bootstrap/check.oxl` (the next self-host component) is
genuinely blocked: it needs real sum types for `ExprKind`, `Ty`, `Item`.

## Design sketch

```
enum ExprKind {
    Int(int),
    Str(str),
    Ident(str),
    Call { f: str, args: own ExprList },
    Bin { op: str, l: own Expr, r: own Expr },
    Nil,
}
```

- Declaration: `enum Name { Variant(payload), Variant { named: ty }, UnitVariant }` —
  positional payloads, named payloads, and unit variants all legal.
- Construction: `ExprKind.Int(7)`, `ExprKind.Bin{op:"+", l:l, r:r}`.
- Consumption: `match` (B2) reads payloads; until B2 lands, a `kind()`
  tag accessor + `as` casts are the escape hatch (keeps B1 shippable
  alone).
- Representation: C tagged union `struct { int tag; union {...} u; }` —
  the C backend emits it directly; no boxing required for payloads ≤
  word size, `own` heap for larger.
- Exhaustiveness: `match` without covering all variants is a compile
  error (B2's job, but the enum must carry enough info).

## Acceptance

- `bootstrap/parser.oxl`'s `Json`-tree replaced by a typed `Expr`
  enum — same output, typed internals
- Round-trip: `ExprKind.Bin{...}` constructed, matched, read back
- Payloads: int, str, `own` struct, nested `own` enum — all work
- `enum` in generics position: `enum Opt[T] { Some(T), None }` —
  monomorphized like other generics (may defer to a fast-follow if it
  doubles scope; flag in review)

## Depends on

Nothing. Everything in Track C wants it — land first.

## Risks

- Recursive payloads (`own Expr` inside `ExprKind`) — self-referential
  structs already work; the union layout must not regress that.
- Generic enums (`Opt[T]`) multiply the work; spec allows staging.
