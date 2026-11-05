# D4 — Shape-based generics (gcshape stenciling)

## Problem

Today every concrete call site gets a full monomorphized copy —
`Max[int]` and `Max[int64]` are separate functions. Works, but:
binary bloat + compile time grow linearly with instantiations. The
design doc's preferred implementation is Go's gcshape stenciling:
share code per *memory shape*, not per type.

## Design sketch

- Classify each concrete type argument by gcshape (Go's term):
  pointer-shaped, word-sized int, f64, or aggregate-specific
- Instantiate one function per distinct *shape signature*, not per
  type signature — `Max[int]`/`Max[int64]`/`Max[own X]` can share the
  same emitted body when their shapes coincide
- Runtime type identity: generic fns get a hidden shape descriptor
  (Go passes itab/dictionary params; C backend can pass a `void*`-sized
  descriptor or specialize only the ops that differ)
- Fallback: full monomorphization stays for types whose ops differ
  semantically (the constraint ops are the shape-breaking part)

## Acceptance

- `Max[int]` and `Max[int64]` emit ONE function; `Max[str]` a separate
  one (different ops)
- Binary size of a generic-heavy file measurably smaller
- All generic tests still pass (semantics unchanged — pure codegen
  optimization)

## Depends on

- Nothing — pure backend optimization. Low priority; can slip a
  milestone without blocking anything.

## Risks

- The dictionary-passing machinery is the fiddly part (Go's itab);
  the win is real but the complexity-per-benefit is low at v0 scale —
  spec honestly says: do it when generic usage is heavy enough to
  measure, not before.

## Deferred (measured 2026-11-05)

Per the risk note's own bar, generic usage was measured before
building: the entire corpus defines **3** generic fns
(`first[T any]` in tests/slices.oxl, `use[T Writer]` in
tests/methods.oxl, `id[T any]` in a run.sh fixture) and emitted C
contains **zero** shared-shape instantiation groups worth stenciling —
`first` isn't even instantiated with a concrete arg. Monomorphization
cost at this volume is nil; dictionaries + shape descriptors would be
pure complexity. Revisit when generic definitions or emitted
instantiations reach an order of magnitude more (rough threshold:
hundreds of mono'd bodies sharing a gcshape in one compile).
