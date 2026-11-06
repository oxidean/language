# S3 — util & data packages

## Problem

The Go-parity surface everyone touches: paths, sorting, math, UTF-8,
flags, logging. None need new language machinery — generics and
interfaces already landed.

## Design sketch

- `std/path` — slash-separated path ops (join/dir/base/ext/clean/match
  glob) — pure .oxl.
- `std/path/filepath` — OS-aware paths via the E3 veneer (`\\` vs `/`,
  volume names, abs/rel) — the Windows work makes this honest.
- `std/cmp` — `Ordered` constraint iface + `cmp.compare`/`min`/`max`.
- `std/slices` — generic fns over `[]T`: `contains`, `index`,
  `reverse`, `clone`, `sort`-helpers (needs generics — have them).
- `std/maps` — generic fns over `collections.Map`/`Set`: `keys`,
  `values`, `merge`, `equal`.
- `std/sort` — `sort[T cmp.Ordered](mut s: []T)`, `sortby[T](mut s,
  less: fn(T,T) -> bool)` (fn values — have them).
- `std/math` — externs to libm (already linked for float) + constants.
- `std/math/rand` — small pure-.oxl PRNG (PCG/xorshift — write fresh,
  don't port Go's), `seed`/`intn`/`floatn`/`shuffle`.
- `std/unicode/utf8` — `utf8.len`, `decode`, `valid` — self-contained.
- `std/flag` — `-x=v`, `-x v`, `--x`, positional remainder — the
  `cli` example's hand parser becomes a flag consumer.
- `std/log` — leveled logger: `log.info/warn/err` to stderr with
  timestamps via `std/time`; `log.setlevel`.

## Acceptance

- `sort`/`slices`/`maps`/`cmp` generic fns instantiate + run through
  both codegens
- `filepath` passes on Windows CI — join/clean/abs with `\\` semantics
- `flag` drives the `cli` example end-to-end: `-n=3`, `-n 3`, `--n`,
  bad flag → usage error
- `utf8` agrees with Go behavior on the usual edge cases (surrogates,
  truncation, 4-byte)
- `log.info` output shape is fixed (asserted in run.sh)

## Depends on

S1. All packages parallel-safe with each other — one item, several
independent dirs.

## Risks

- `filepath` needs real OS knowledge — port Go's behavior table
  (drive letters, UNC, `/` acceptance) rather than inventing.
