# Function values and closures (B4)

`fn(A, B) -> R` is a first-class type. Named functions, closures, and
any expression of a `fn` type can be stored, passed, returned, kept in
slices — and called indirectly.

```oxl
fn add(a: int, b: int) -> int { return a + b }

f := add                 // named fn as a value
assert(f(2, 3) == 5)     // indirect call
ops := mkslice(fn(int, int) -> int, 2)
ops[0] = add
assert(ops[0](1, 1) == 2)
```

A function type spells `fn(param, ...) -> ret`; `-> ret` may be omitted
for `void`. Named parameters in the type are documentation only:
`fn(a: int, b: int) -> int` is the same type as `fn(int, int) -> int`.

## Closures

`{ |a: int, b: int| stmts }` builds a closure. Parameters need
annotations; the return type is inferred from the body's `return`s
(`void` when there are none).

```oxl
n := 100
addn := { |x: int| return x + n }
n = 200
assert(addn(5) == 105)   // captures are by value — env snapshot
```

Captures are **copied** into a heap-allocated environment
(`rt_alloc_own` — escaping storage, so a closure may outlive its
creating scope) at closure creation; later writes to the outer variable don't reach the closure
(and vice versa). A closure may not re-bind a captured name — the
checker reports the shadow instead of silently aliasing.

## Representation

Every function value is an `RtFnv` fat pair `{ void *env, void *fp }`
called as `fp(env, args...)`. Named functions get a zero-env wrapper so
named and closure callees share one calling convention; closures point
`env` at their capture struct. This keeps `spawn f()` and `[]fn(...)`
storage uniform.

## v0.2 restrictions

The checker rejects, with dedicated diagnostics:

- **suspendable functions as values** — the frame executor needs to
  know the callee is a poll machine; an `RtFnv` can't express that.
- **suspension inside closures** — direct (`sleep`, `recv`, `send`,
  `spawn`, `scope`, `select`) or transitive (calling a suspendable fn).
- **generic functions as values** — `f := id[T]` isn't callable
  without instantiation; call it first, wrap the result.
- **`[T, err]` pair-returning functions as values** — the pair ABI is a
  two-slot return, not a plain `fn` signature.
- **`==`/`!=` between function values** — fat pairs aren't comparable.

These are all statically diagnosable; none are load-bearing for the
bootstrap compiler (visitor passes want non-suspending closures).

## Notes

- Indirect calls evaluate the callee once: `pick(i)(1, 2)` binds the
  `RtFnv` in a temp before invoking.
- `spawn f()` on a fn value packs the fat pair into the arg block and
  trampolines through `fp` on the worker — non-suspending callees only.
