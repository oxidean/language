# Slices — `[]T`

A slice is a 32-byte fat header `{ ptr, len, cap, heap }` passed **by
value**. The elements live in a shared backing store; copies of the
header — including views created by `s[a:b]` — point at the same
backing. `heap` records the backing's domain: `0` = scope arena,
`1` = heap (`malloc`).

## Construction

```oxl
s := mkslice(int, 8)   // len == cap == 8, all elements zeroed
e := mkslice(str, 0)   // empty slice, ptr == nil
```

`mkslice(T, n)` takes a type name and an element count. The backing
store is allocated from the enclosing scope's arena (`rt_alloc`), so it
is bulk-freed when the scope ends — the same hybrid reclamation model
as other allocations.

## Indexing — `s[i]`

`s[i]` is an lvalue: it reads or writes element `i` of the backing
store. Both the slice header and the index are evaluated once. The
index must be an integer; `0 <= i < len` is enforced at run time and an
out-of-range access traps (`rt_trap(10)`).

```oxl
s[0] = 42
v := s[len(s) - 1]
```

## Length — `len(s)`

`len` returns the header's `len` field for slices (strings keep their
existing meaning). There is no `cap` accessor in the surface language.

## Growth — `s = push(s, v)`

`push` appends `v` to the backing store, growing capacity by doubling
(first growth allocates 4 elements) and returns the possibly-updated
header. Because the header is a value, **the result must be
reassigned**: `s = push(s, v)`. Elements are copied on growth; existing
views into the old backing keep referencing it — they do not follow the
reallocation (same rule as Go append).

## Views — `s[a:b]`

`s[a:b]` creates a view sharing the backing store — no copy. The view's
`ptr` is `s.ptr + a`, `len` is `b - a`, and `cap` is `s.cap - a`, so
`push` on a view can write into spare capacity the parent sees. Writes
through a view are visible in the parent and vice versa.

Bounds may be omitted: `s[:b]`, `s[a:]`, `s[:]`. The invariant
`0 <= a <= b <= len` is enforced at run time; violations trap
(`rt_trap(10)`).

## Ownership and escape

- `own []T` (written `new(slice)`) deep-copies header **and** backing to
  the heap via `rt_slice_own`: the `own` value may outlive the scope
  that produced the source slice. The copy is one level deep — for
  reference-like elements (`[]str` copies the `OxlStr` headers, not
  their byte buffers) the referenced storage must still outlive the
  owned slice. Indexing, `len`, and view slicing
  auto-deref the handle, and views/growth of a heap slice keep the heap
  domain (`push` reallocs with `malloc`, never into the arena).
- Ordinary (non-`own`) slices keep arena backing: escaping one past its
  scope is unsafe for the same reason escaping any arena allocation is.
  The checker rejects `[]T` at the `return` and `spawn` boundaries
  (pass `own []T` instead); a general intra-scope escape check
  remains open work beyond the D2 `own`-move tracking.
- `nil` assigns to `[]T` and produces the empty header
  `{ nil, 0, 0, 0 }`.

## Generics

`[]T` participates in shape substitution like any other type:
`fn first[T any](s: []T) -> T` binds `T` from the argument's element
type and monomorphizes as usual.

## Trap codes

| code | meaning |
|------|---------|
| `rt_trap(10)` | slice index out of range, or invalid `s[a:b]` range |

## Bootstrap note

`bootstrap/parser.oxl` keeps its linked `Json` child lists rather than
migrating to `[]Node`: the parser builds nodes incrementally and the
linked spine avoids re-teaching `jadd` a growth discipline for no
measurable win at self-parse scale. The slice surface is exercised by
`tests/slices.oxl` (10k pushes, views, generics, own headers, and a
suspendable worker).
