# D3 — Methods + interface dispatch

## Problem

`x.f(y)` receiver-call syntax and interface-typed values are the
biggest missing type-system piece for real service code (handlers,
serializers, storage backends). Today's `interface` is only a generic
op-constraint vocabulary — not a type.

## Design sketch

Go-shaped, deliberately shallow:

- Methods: `fn (r Receiver) name(args)` — `recv` on the type; called
  `r.name(args)`. Sugar for `name(r, args)` — same C emit, just
  resolves the receiver's type to pick the fn.
- Interface types: `interface Writer { write(s: str) -> int }` (today's
  op-constraint `interface` gains *method* decls). A struct satisfies
  it structurally (has the methods) — no explicit `implements`.
- Interface values: `let w: Writer = my_impl` — fat pair `{data, vtable}`
  in codegen (itab, Go-style).
- Dispatch: `w.write(s)` → indirect call through vtable.
- Generics: `interface` constraints already exist — method-declaring
  interfaces become constraint-capable automatically (a `Writer`
  constraint lets a generic call `.write`).

Explicitly out: trait solving, associated types, generics on the
interface itself, inheritance. Shallow is the point.

## Acceptance

- `fn (p Point) norm() -> int` called as `p.norm()` and `norm(p)`
- `interface`-typed var holds two different impls; dispatch picks
  correctly
- Generic fn with a method-constraint compiles + calls through
- HTTP `route()` or a std `Writer` demo uses the real feature

## Depends on

- B1 (enum) helps internally but isn't required
- Nothing hard — biggest standalone type-system item in the milestone

## Risks

- Vtable layout + method set computation — the meat of the work.
- `own`/`mut` receivers (`fn (p mut Point)`) — borrow semantics must
  compose with method dispatch; keep the same rules as free fns.
