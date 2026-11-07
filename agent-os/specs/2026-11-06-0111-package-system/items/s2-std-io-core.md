# S2 — io core + the byte story

## Problem

A core library hangs off `io.Reader`/`Writer`-shaped interfaces and a
byte slice. oxl has
iface types (D3) — the interfaces are expressible — but no `byte`
type and an `err` that's an opaque int-like value. Three real gaps
stand between today and an io-shaped library: `byte`/`[]byte`,
message-carrying errors, and the interfaces themselves.

## Design sketch

- **`byte`/`u8` type + `[]byte`** — int width-8 value type; `byteat`
  already returns int, `substr` works on str. Minimal language add:
  `u8` keyword type, slice over it, `[]byte(s)`/`str(b)` conversions.
  Everything in S4 (encodings, hashes, csv) consumes it.
- **`std/errors` + richer `err`** — today `Ty::Err` is int-compatible
  and `nil`-able. To carry `errors.new("boom")` the runtime err value
  needs a message: err becomes a small struct {code, msg} at ABI
  level (or a builtin `err("msg")` constructor over a str payload —
  pick at impl time; the item is *that* err carries a message, not
  which representation). API: `errors.new(msg)`, `errors.wrap(e,
  msg)`? — keep to `new` + `msg(e)`/`code(e)` accessors for v1;
  `Is/As` need error trees + typed targets, deferred (see parked).
- **`std/io`** — `Reader`/`Writer`/`Closer`/`ReadWriter` iface decls
  (`read(mut b: []byte) -> [int, err]` etc.) + `io.copy`, `io.readall`,
  `io.limitreader`. Pure iface work — no language change.
- **`std/strconv`** — `atoi`, `itoa`, `parsefloat`, `formatfloat`,
  `parseint(s, base)` — pure-.oxl over the builtins.
- **`std/bytes`** — `Buffer` (write/read/grow over `[]byte`) +
  `bytes.equal/compare/contains/index` — slice helpers over `[]byte`.
- **`std/bufio`** — `bufio.Reader`/`Writer` over `io` interfaces —
  buffered read/write/peek/flush.
- **`std/fmt`, bounded** — no variadics and no `any` runtime type
  means no `sprintf("%v", x)` reflection. Honest bound:
  `fmt.sprint[T any](v T)`, `fmt.sprintf(f: str, args: []str)` for
  string substitution, typed `fmt.sprinti/sprintf` helpers. `%v`-
  anything goes on the parked list until reflection exists.

## Acceptance

- `std/io` interfaces are real: a `bufio.Reader` over an `os.File`
  reads lines; `io.copy(dst, src)` moves bytes
- `errors.new("x")` flows through `[v, err]` returns and `msg(e)`
  prints it; `nil` still means no error
- `[]byte` round-trips: `str(bytes.tostr(b)) == s`; byte literals
  in corpus
- strconv round-trips int/float both directions incl. error paths
- `bytes.Buffer` + `bufio.Writer` round-trip through `io.copy`
- Every new pkg covered by `stdmods_test`-style run.sh cases

## Depends on

S1 (the tree exists), M1. The `u8`/`err` language bits need their own
corpus entries — they're inside this item, not separate tasks.

## Risks

- `err` representation change is an ABI touch — every emitted
  program and rt.c agree on the new layout in one commit, or nothing
  links.
- `[]byte` vs `str` conversions must respect ownership (a `[]byte`
  into an arena freed early is the B3 dangling gap again — `own`
  buffers or copy-on-convert, and say which in the API docs).
