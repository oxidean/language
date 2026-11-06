# Parked — Go stdlib packages deliberately out of reach

"Full functional match as much as logically possible" — these are the
honest bounds. Each says why it doesn't map or doesn't pay yet. None
is permanently banned; several graduate when the language grows the
machinery they need.

## Needs reflection / `any` runtime values — the language lacks it

- `fmt` `%v`-anything, `reflect`, `encoding/gob`, `encoding/xml`
  (tagged-field driven), `database/sql` (driver-registry + `Scan(any)`)
  — oxl's type system is static + generic with no runtime type
  descriptors. `std/fmt` ships a bounded surface (S2). A `std/reflect`
  lite or compiler-emitted type descriptors would unlock these —
  its own milestone.
- `html/template`, `text/template` — string-keyed `map[string]any`
  pipelines; same bound.

## Needs machinery the language doesn't have

- `crypto/tls`, `x509`, `crypto/{rsa,ecdsa,ed25519}` — a TLS stack is
  a project, not a package. `std/crypto` v1 = hash+HMAC (S4).
- `plugin`, `go/*` (ast/parser/token), `runtime` internals,
  `runtime/pprof` — runtime/codecs introspection doesn't exist.
- `unsafe` (Go sense) — oxl `unsafe` is an ownership-check opt-out
  block, not `Pointer` arithmetic.

## Replaced by language semantics — porting them would be wrong

- `context` — `scope` blocks + structured cancellation already carry
  cancellation + deadline semantics; a package would double the
  mechanism. `sync.Cond`-style waits live in `std/sync`.
- `testing.T` beyond what `std/testing` has — benchmarks/fuzzing are
  possible later but not a parity blocker.
- `os/exec`, `os/signal` — process/signal surface exists in `rt.c`
  but is runtime-owned; a thin `std/os` growth covers what's needed
  without a Go `exec.Cmd` clone.

## Doesn't pay at this scale

- `image`, `image/color`, `image/draw` — needs a real use case first
  (the GUI pilot goes through GTK, not an image buffer).
- `compress/{gzip,zlib,flate}`, `archive/{tar,zip}` — C-FFI wrap is
  trivial when needed; pure-.oxl ports wait for `[]byte` to settle.
- `net/http/httputil`, `pprof`, `net/url` (small — folds into
  `std/net/http` when it needs it), `mime`, `net/mail`.
- `container/{heap,list,ring}` — `std/collections` covers the map/set
  core; heap/ring are addable on demand.
- `log/slog`, `slog` — `std/log` ships first; structured logging
  rides on it later.
- `crypto/rand`, `math/big`, `time/tzdata`, `syscall` — platform or
  precision machinery deferred until a real consumer exists.
