# D1 — `float` type

## Problem

JSON floats, durations, metrics, any real service numerics need a
floating-point type. `std/json` documents ints-only as the v0
limitation; this unblocks it.

## Design sketch

- `float` (f64) primitive — C `double` in codegen
- Arithmetic: same `+ - * /` operators; comparisons `== != < > <= >=`
- Mixing `int`+`float`: explicit conversion via `ftoi(f)`/`itof(i)`
  builtins — implicit widening is a deliberate anti-choice (keeps the
  type system shallow and costs visible)
- `atof`/`ftoa` builtins (str conversions)
- `std/json`: `j.i` for ints, `j.f` for floats, `kind` tag picks —
  `jparse` reads `.`/`e`/`E` into float

## Acceptance

- `float` arithmetic + comparisons; `ftoi`/`itof` round-trips
- `jparse("3.14")` → float node; `tojson` emits `3.14`
- `json_test.oxl` asserts float round-trip
- `int`/`float` mixing is a compile error, not silent widening

## Depends on

Nothing — self-contained, small. Good first worktree task.

## Risks

- Low. Only real choice is implicit-mixing (spec says no — keep it
  explicit and shallow).
