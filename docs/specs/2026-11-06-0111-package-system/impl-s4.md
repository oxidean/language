# S4 Implementation Plan — net & encodings

> Implements `items/s4-std-net-encoding.md`. Executed natively in-session.

**Goal:** `std/net` (Conn/Listener over the socket externs), an http
client beside `serve`, `encoding/{base64,hex,csv,binary}`, `std/hash`,
and `crypto/{sha256,md5,hmac}` — pure .oxl except one runtime helper.

## Representation decisions (vs the sketch)

- **`net.dial` needs one new extern.** rt.c ships listen/accept/bind
  but no connect — the spec's "no new externs" predates the client.
  `rt_sock_connect_ipv4(fd, ip_h32, port)` joins the OXL_NET block in
  the same portable style (E3 veneer: `connect` on `oxl_sock_t`,
  `_fullpath`-style normalization not needed — no paths). Host is a
  dotted IPv4 parsed in .oxl — getaddrinfo/DNS stays parked
  (documented; numeric hosts resolve — which covers loopback).
- **nil-slice emit fixed on the way.** `let r: [][]str = nil` checks
  fine (`nil` types as Err, `assignable` lets it through) but both
  emitters wrote `RtSlice rows = 0` — invalid C. `conv_v`/`gconv_v`
  gains a Nil→Slice arm → `(RtSlice){ 0, 0, 0, 0 }`. This unblocks
  `[][]T` declarations — `csv.read_all` returns `[][]str`; there is
  no `mkslice([]T)` element literal so nil-init + push is the only
  construction path. Both codegens get the arm (parity).
- **`hash` is an iface-shaped package.** `interface Hash` —
  `write(b) -> [int,err]` (io.Writer), `sum() -> int`, `reset()`,
  `blocksize() -> int` — plus `new_fnv1a/new_adler32/new_crc32 ->
  Hash` and package-level `fnv1a32/fnv1a64/adler32/crc32` one-shots.
  rt.c's FNV is interning-internal, not an extern — all three are
  pure .oxl (the sketch's "expose it" doesn't apply). crc32's IEEE
  table is generated lazily on first use (arith shifts — no `>>`).
- **crypto is fresh-from-RFC .oxl, u32-in-int64 arithmetic.** No shift
  ops exist; `x>>n` is `x/2**n` and `x<<n` is `(x*2**n)&0xffffffff` —
  masked non-negative so `/` stays exact. sha256 K-table and md5
  T-table are the spec-mandated constants (allowed by the item).
  Each pkg: `sum*(b []byte) -> []byte` + `new() -> hash.Hash`
  incremental impl + `hexdigest(b) -> str` convenience.
- **`hmac` is shaped over the Hash iface:** `new(hf: fn() ->
  hash.Hash, key: []byte) -> hash.Hash` — Hmac itself is a Hash
  (write/sum/reset chain to inner). Named-fn-type params landed in
  S3. Convenience `sum256(key,msg)`/`sum_md5(key,msg)` one-shots.
- **http serve migrates to net.listen only at the socket boundary.**
  The accept loop's scope/inflight machinery and fd-level helpers
  stay — `Listener.accept()` hands over a `Conn` whose `fd()` the
  existing code keeps using. Client side is new: `get(url)` /
  `post(url, ctype, body)` / `request(method, url, ctype, body)` →
  `[Resp, err]`. URL parse handles `http://` + host + `:port` +
  path; HTTP/1.0 request so every legal server close-delimits the
  body; `rt_set_rcvtimo(fd, 10)` so a hung peer can't block forever.
  Reads go through `net.Conn` + `bufio.Reader` (status line +
  headers via `readline`).

## Packages

- `std/net` — `Conn{fd}` impl `io.Reader/Writer/Closer` + `fd()`
  escape hatch + `set_rcvtimo/set_sndtimo`; `Listener{fd}` with
  `accept() -> [Conn,err]` + `close()`; `dial(ip4, port) ->
  [Conn,err]` (parses dotted quad, `rt_sock_connect_ipv4`, err on
  malformed host/connect fail); `listen(port) -> [Listener,err]`
  (open+reuseaddr+bind+listen, same retry shape serve had — backoff
  lives in net now, serve calls listen once? No: the backoff exists
  because rapid restarts hit EXCLUSIVEADDRUSE — `net.listen` keeps
  the retry loop, `serve` just calls it).
- `std/encoding/base64` — `encode`/`decode` (RFC 4648 std alphabet,
  padding), `encode_raw`/`decode_raw` (no padding),
  `url_encode`/`url_decode` (`-_` alphabet). Decode rejects
  non-alphabet bytes and bad padding.
- `std/encoding/hex` — `encode`/`decode`, lower-case; decode
  rejects odd length and non-hex.
- `std/encoding/binary` — `le_u16/le_u32/le_u64/be_u16/be_u32/be_u64`
  readers over `[]byte` at offset, `put_le_*/put_be_*` writers.
  Needed by md5 (le) and every real protocol.
- `std/encoding/csv` — `read_all(s) -> [[][]str, err]` RFC 4180:
  quoted fields, `""` escape, embedded commas/newlines/CRLF,
  bare-`\r` rejected inside quotes; `format(rows) -> str` with
  minimal quoting (comma/quote/newline/CR triggers).
- `std/hash` — iface + one-shots as above.
- `std/crypto/sha256` — `sum256`, `new()->hash.Hash`, `hexdigest`.
- `std/crypto/md5` — `sum`, `new()->hash.Hash`, `hexdigest`.
- `std/crypto/hmac` — `new(fn()->hash.Hash, key)->hash.Hash` +
  `sum256`/`sum_md5` one-shots.
- `std/net/http` — serve on net; `get/post/request` client.

## Call-site + test wiring

- `tests/net_test.oxl` — loopback echo: `spawnBlocking` a
  listen/accept/read/write server, main `net.dial("127.0.0.1")` →
  write → read → assert round-trip; dial-refused → err non-nil;
  listener close. Ends with `exit(0)` so the accept loop doesn't
  need a shutdown path.
- `tests/enc_test.oxl` — RFC 4648 base64 vectors (incl. url+raw),
  hex vectors + malformed, binary le/be known patterns, csv RFC
  4180 cases + format round-trip.
- `tests/hash_test.oxl` — fnv1a32/64, adler32 ("Wikipedia"),
  crc32 ("123456789") known answers; incremental write==one-shot.
- `tests/crypto_test.oxl` — sha256/md5 "" / "abc" / 55-56-64-byte
  boundary + quick-brown-fox vectors; hmac-md5 RFC 2202 case 1-2;
  hmac-sha256 RFC 4231 case 1-2.
- `tests/httpclient_test.oxl` — `spawnBlocking` serve, `get`/`post`
  against it, status/body/ctype asserts, `exit(0)`.
- run.sh `== oxl test: net & enc (s4) ==` section; enc/hash/crypto
  also through `bootstrap/cmd/codegen --test` + cc.
- README std table, plan.md marker.

## Acceptance (per item)

- net.echo + http.get through the new stack on all three CI legs
- sha256/md5/hmac published vectors pass
- base64/hex/csv/binary RFC vectors + round-trips
- only `rt_sock_connect_ipv4` added to rt.c; rest pure .oxl
- `[][]str` works end to end (nil-slice fix), both codegens
