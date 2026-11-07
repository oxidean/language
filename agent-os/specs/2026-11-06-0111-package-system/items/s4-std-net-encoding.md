# S4 — net & encodings

## Problem

Sockets exist in `rt.c` (the http pilot uses them) but there's no
`std/net` — http reaches raw externs. Encodings/hashes are missing
entirely. This is the "core packages" bulk of the ask.

## Design sketch

- `std/net` — `net.dial(host, port)`, `net.listen(port)` → `Conn`/`Listener`
  iface-typed (Conn implements `io.Reader`/`Writer`/`Closer` —
  S2's interfaces pay off). Backed by existing `rt.c` socket externs;
  the pilot's raw calls migrate behind it.
- `std/net/http` — moved from `std/http` (S1), rebuilt on `std/net` +
  `std/io` + `std/bufio`: `serve(port, handler)` keeps today's shape;
  `Client`/`get`/`post` is the growth (one-shot client calls —
  client-side HTTP on `net.dial`).
- `std/encoding/json` — moved (S1); add `encode`/`decode` over
  `io.Reader/Writer` signatures where sensible.
- `std/encoding/base64`, `hex`, `csv`, `binary` — pure-.oxl over
  `[]byte` (S2). `binary` = `big/little-endian` uint read/write —
  needed for real protocols.
- `std/hash` — `fnv`/`adler32`/`crc32` (rt.c already carries an FNV —
  expose it; adler/crc pure-.oxl).
- `std/crypto/sha256`, `md5`, `hmac` — written **fresh in oxl** from
  the public specs (RFC 6234 / RFC 1321 / RFC 2104) — no C port, no
  copied constants beyond the spec-mandated round tables. Self-
  contained bit-twiddling; the language is sufficient.
- `crypto/tls`, `x509`: non-goal — documented in parked-stdlib.
- `database/sql`, `compress/*`, `image`, `html`, `text/template`:
  parked — documented there.

## Acceptance

- `net.echo` test: dial → write → read back over loopback in run.sh
- http pilot serves through the new stack; `http.get` fetches it in
  the same test
- sha256/md5 match published test vectors (`"abc"` → known digests);
  hmac RFC 2202 vectors
- base64/hex/csv/binary round-trips + RFC vectors where they exist
- All pure-.oxl — no new externs beyond what `rt.c` already has

## Depends on

S1 (tree), S2 (`[]byte` + `io` + `bufio` — everything here consumes
them). `std/net` itself only needs S2's interfaces.

## Risks

- Crypto correctness — spec vectors are the acceptance bar, exactly
  because these are hand-written.
- `net` on Windows: E3's WinSock veneer is in place; the `std/net`
  API must not leak POSIX types (`int fd` → platform handle behind
  the iface).
