# Match

`match` dispatches on enum tags or literal values. Implemented in
v0.2 spec B2.

```oxl
match e {
    ExprKind.Int(v) => handle_int(v)
    ExprKind.Bin{op, l, r} => handle_bin(op, l, r)
    ExprKind.Nil => default_case()
    _ => fallback()
}
```

## Patterns

- `E.V(x, …)` — positional payload variant; binds each payload field
  in declaration order (`x` gets `f0`, etc.). `_` skips a position.
- `E.V{a, …}` — named payload variant; binds the named fields (any
  subset, by field name).
- `E.V` — unit variant. Writing `E.V` for a variant that has a
  payload is a compile error — bind the payload instead.
- `_` — wildcard; covers everything not matched above.
- `x` — bare identifier; binds the whole scrutinee and covers the
  rest (acts as a catch-all).
- Literal — `7`, `1.5`, `"hi"`, `true`, `false` — compares the
  scrutinee for equality (strings via `rt_streq`).

Patterns qualify like enum constructors: inside the declaring module
`E.V`, from an importer `m.E.V`.

## Arms

Two shapes — do not mix them in one match:

```oxl
// expression arms — the match is a value
v := match e {
    E.A(x) => x + 1
    _ => 0
}

// block arms — the match is a statement
match e {
    E.A(x) => {
        println(itoa(x))
    }
    _ => {}
}
```

Expression arms unify on the first arm's type — later arms must be
assignable to it.

## Exhaustiveness

- Enum scrutinee: all variants must be covered by name or by a
  wildcard/bind arm. The error names missing variants:
  ``non-exhaustive match on `E` — missing B``.
- `bool`: `true`+`false` covers without a wildcard.
- `int`/`float`/`str`: literal patterns never exhaust — a `_` (or
  bare bind) arm is required.

## Interaction with concurrency

Arm bodies run conditionally, so a suspending call (`recv`, `send`,
`sleep`, or any suspend-capable fn) may appear only where the frame
transform can emit it as a statement:

- A bare statement-level `match … { … }` (expr arms included —
  `=> susp_call()` alone works there) — emitted as an if-chain inside
  the coroutine frame.
- A match in value position (`let`/`assign`/args) may NOT suspend in
  arm bodies — the checker reports an error; hoist the call or use a
  statement match. The scrutinee may suspend freely — it hoists out.

## Lowering

Matches lower to `if`/`else if` chains on `.tag` (enums) or `==` /
`rt_streq` (literals) in C — no decision tree or jump table yet.
Payload binds read the union fields directly
(`_ms.u.v{i}.f_{field}`); `own E` scrutinees are dereferenced.
