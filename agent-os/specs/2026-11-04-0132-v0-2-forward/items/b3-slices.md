# B3 — Slices `[]T`

## Problem

Token streams, arg lists, AST children all want contiguous arrays with
O(1) indexing. Today: `own` linked lists (O(n) index, allocator noise)
or `mkbuf`/`poke8` (byte-level, untyped).

## Design sketch

- `[]T` type: `{ ptr, len, cap, heap }` fat header, 32 bytes on the
  documented 64-bit targets — passed by value (the header), elements
  by reference into the backing store. `heap` marks own backings that
  outlive scopes; views inherit it and growth preserves it.
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
  outlive the scope — same rule as `ref`. Ordinary slice escapes are
  not generally runtime-checked and are unsafe (see
  `docs/semantics/slices.md`); the checker rejects `[]T` at `return`
  and `spawn` boundaries. The general check — an arena-backed slice
  stored in an outer struct and read after its scope closes reads
  freed storage — remains open work beyond the D2 own-move tracking.
- Generics interplay: `[]T` inside generic fns — shape-substitution
  already handles pointer-sized types; verify.
