# Enums (tagged unions)

`enum` declares a tagged union with unit, positional, and named
payload variants. Implemented in v0.2 spec B1.

```oxl
enum ExprKind {
    Int(int),
    Str(str),
    Bin { op: str, l: own Expr, r: own Expr },
    Nil,
}
```

## Construction

```oxl
i := ExprKind.Int(7)                    // positional payload
b := ExprKind.Bin{op: "+", l: l, r: r}  // named payload
n := ExprKind.Nil                       // unit variant
```

- Positional payload fields are `f0`, `f1`, … in declaration order;
  named fields keep their names.
- Variant tags are declaration order, starting at 0.
- Duplicate variant names and payload arity/type mismatches are
  compile errors.
- Enums qualify like other decls: `m.E.V(...)` inside an importing
  module, `E.V` inside the declaring module.

## Reading a value

`match` (B2) is the primary reader — see `match.md`. The raw escape
hatches remain available:

```oxl
k := e.kind()            // int tag, works on `own E` too
if k == 2 {
    p := e as ExprKind.Bin  // payload struct; wrong variant traps
    println(p.op)
}
```

`e as E.V` extracts the payload struct `E.V`. The cast checks the tag
at runtime — extracting the wrong variant traps rather than reading
the union raw. Both forms dereference `own E` handles.

## Representation

The C backend emits `struct { int tag; union { ... } u; }` — no
boxing required for payloads; `own` fields heap-allocate as usual.
`new(E.V(...))` heap-boxes an enum value into `own E`; more generally
`new(expr)` now boxes any value (`own T`), while `new(T)` still
allocates a zero-initialized `own T` struct.

## Not yet

- Generic enums (`enum Opt[T] { Some(T), None }`) — spec allows a
  fast-follow; tracked separately.
