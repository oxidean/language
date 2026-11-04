# B6 — `std/collections` maps/sets

## Problem

The checker's env scopes, fn tables, and generic-instantiation maps are
`map[str]T`-shaped; `bootstrap/check.oxl` needs real associative
containers — linked lists at O(n) is fine for v0 but the *API shape*
should be real now so the port doesn't invent its own.

## Design sketch

Written in `.oxl` (stdlib convention):

```
m := map_new()
map_set(m, "key", 42)        // int values (word payloads)
v := map_get(m, "key")       // -> [int, err]: nil err on miss
map_del(m, "key")
s := set_new(); set_add(s, "x"); set_has(s, "x")
```

- Hash map: FNV-1a over the string bytes, power-of-2 buckets, separate
  chaining on `own` node lists (matches everything the language
  already does).
- v0 scope: `str` keys → `int` values only. `own`/`str` values need
  either generics-over-payload or boxed words — flag as fast-follow
  (generics exist but map's payload-generic shape needs D-generics
  review; word payloads cover the checker).
- Arena-aware: nodes allocate in the enclosing scope's arena — a map
  dies with its scope, bulk-free.

## Acceptance

- 10k set/get, no collisions bug, `map_get` miss returns `err != nil`
- delete removes correctly
- `bootstrap/check.oxl` env uses `std/collections` (acceptance is the
  port itself using it)

## Depends on

Nothing technically — pure stdlib. Track C consumes it.

## Risks

- Low. Only decision is payload polymorphism — spec keeps v0 to int
  payloads honestly rather than fake-generics.
