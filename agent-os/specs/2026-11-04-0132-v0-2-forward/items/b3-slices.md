# B3 — Slices `[]T`

## Problem

Token streams, arg lists, AST children all want contiguous arrays with
O(1) indexing. Today: `own` linked lists (O(n) index, allocator noise)
or `mkbuf`/`poke8` (byte-level, untyped).

## Design sketch

- `[]T` type: `{ ptr, len, cap }` fat pointer, 24 bytes — passed by
  value (the header), elements by reference into the backing store.
- `mkslice(T, n)` builtin → zeroed `[]T` of len n (capacity n).
- `s[i]` read/write index expr; `len(s)` works on slices; `push(s, v)`
  grows (realloc, cap doubling — arena-friendly: bulk-free still works).
- Slicing `s[a:b]` → a view (shared backing, no copy).
- Backing store: scope arena by default (slices die with their scope —
  matches hybrid reclamation); `own`-marked backing for escape.

## Acceptance

- `mkbuf`-free resizable buffer: push 10k ints, index, slice
- `len`, bounds-checked index (trap on OOB — same trap machinery)
- `s[a:b]` view semantics (writes visible in parent)
- `bootstrap/parser.oxl`'s token list optionally moved to `[]Tok` —
  measurable allocation drop (document if kept linked for simplicity)

## Depends on

Nothing — pure frontend+runtime addition. Track C benefits.

## Risks

- Escape analysis pressure: a `[]T` view of an arena slice must not
  outlive the scope — same rule as `ref`, enforce statically where
  trivially possible, runtime check otherwise.
- Generics interplay: `[]T` inside generic fns — shape-substitution
  already handles pointer-sized types; verify.
