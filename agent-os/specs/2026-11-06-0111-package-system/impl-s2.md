# S2 Implementation Plan — io core + the byte story

> Implements `items/s2-std-io-core.md`. Executed natively in-session.

**Goal:** the io substrate — `byte`/`[]byte`, message-carrying
`err`, `std/io` interfaces — plus `errors`, `strconv`, `bytes`, `bufio`,
and a bounded `fmt`. Everything S4 consumes lands here.

## Representation decisions (vs the sketch)

- **`err` stays `int64` + interned-message registry — no ABI touch.**
  The sketch allowed either; the struct option forces a same-commit
  retarget of every `struct o_ret_*`/`o_vret_*`, `rt_scope_propagate`,
  and `RtScope.err` — the exact "nothing links" risk the item flags.
  Registry: `rt_err_new(msg)` interns by message → same code every call
  (sentinel-friendly: `io.eof()` is stable), `rt_err_msg(code)` looks up.
  Codes allocate from `1<<20` up — runtime trap codes stay tiny, so a
  messaged err never aliases a trap code. `nil`/`0` stays no-error;
  `msg(nil)` = `""`. Leak: one table entry per distinct message,
  mutex-guarded — documented in `std/errors` docs.

- **`byte` is `Ty::Byte` (u8), byte↔int assignable both ways** — the
  `err`↔`int` precedent. `byteat` keeps returning `int` (zero churn);
  `b[i] = byteat(s, i)` just compiles. Arithmetic wraps mod 256 —
  codegen emits `(uint8_t)(l op r)` when result ty is byte (parity:
  `byte + byte` stays byte, doesn't promote to int64).

- **`'a'` is lexer sugar for the int 97** — no new AST/checker/codegen
  node. Both lexers produce the Int token with the scalar value; escapes
  mirror strings (`'\n' '\t' '\\' '\'' '\0'`). `''` and multi-char
  literals are lex errors. Byte literals in corpus = char lits + `byte`
  decls.

- **Conversions ride `as`** — it already parses `e as TyRef` (enum
  extraction today). Add `(str→[]byte)`, `([]byte→str)`, `(int→byte)`,
  `(byte→int)`, `(byte↔byte)` to the same arm in both checkers/codegens.
  `s as []byte` = `rt_bytes_str` (scope-arena copy — mutation can't
  write into string data; `own` via `new(b)` to escape). `b as str` =
  `rt_str_bytes` (zero-copy view; backing dies with the slice — docs
  say so). `x as []T` / `x as y` for other pairs stays an error.

- **FFI allowlist gains `err` + `[]T` args** — `err`↔`int64_t`,
  `[]byte`↔`RtSlice` are already the exact ABI; the ban was
  over-broad. Ret positions were never restricted. Both checkers.

## Runtime additions (rt.c/rt.h — shared by both backends)

```c
int64_t rt_err_new(OxlStr msg);        /* interned registry → code */
OxlStr   rt_err_msg(int64_t code);     /* "" for unknown/0         */
RtSlice  rt_bytes_str(OxlStr s);       /* arena copy → []byte       */
OxlStr   rt_str_bytes(RtSlice b);      /* view, no copy             */
int64_t  rt_open_read(OxlStr path);    /* fd or -1                  */
int64_t  rt_open_write(OxlStr path);   /* create|trunc, fd or -1    */
int64_t  rt_fd_read(int64_t fd, RtSlice b);   /* n or -1            */
int64_t  rt_fd_write(int64_t fd, RtSlice b);  /* n or -1            */
void     rt_fd_close(int64_t fd);
```

fd fns get the E3 veneer: POSIX `open/read/write/close` ⇄ Win32
`_open/_read/_write/_close` + `O_BINARY`. `io.Reader` over a real file
is the acceptance path, so Windows can't be a stub.

## Language changes (×2 toolchains, byte-parity diags)

| Change | Seed | Self-hosted |
|---|---|---|
| `Ty::Byte` + `name()` + assignable | check.rs | check.oxl Ty |
| `"byte"` in type resolution | check.rs `ty_named` | check.oxl |
| `'c'` literal → Int token | lexer.rs | lexlib.oxl |
| `as` conv arms | check.rs + codegen.rs | check.oxl + codegen.oxl |
| FFI allowlist `err`,`[]T` | check.rs | check.oxl |
| `cty(Byte)`=`uint8_t`, wrap emit | codegen.rs | codegen.oxl |

## Std packages (pure .oxl over the builtins/externs)

- `std/errors`: `new(msg) -> err`, `msg(e) -> str`, `code(e) -> int`.
- `std/io`: `Reader`/`Writer`/`Closer`/`ReadWriter` ifaces;
  `eof() -> err` sentinel (interned msg `"EOF"`, stable across calls);
  `copy(dst, src) -> [int, err]` (32KB buf, eof→nil, partial-write loop);
  `readall(r) -> [str, err]`; `limitreader(r, n) -> Reader`.
- `std/os`: `struct File{fd: int}` + `open/create` → `[File, err]`,
  `f.read/f.write/f.close` implementing io ifaces, `stdin/stdout/stderr`.
- `std/strconv`: `atoi/itoa` (wrap builtins; atoi validates →
  `[int, err]`), `parsefloat/formatfloat`, `parseint(s, base)`.
- `std/bytes`: `Buffer{buf: []byte}` w/ `write/read/str/len/reset`;
  `equal/compare/contains/index`.
- `std/bufio`: `Reader{r: io.Reader, buf, ri, wi}` + `Writer` — read/
  peek/write/flush; `newreader`/`newwriter` ctors.
- `std/fmt` (bounded): `sprintf(f: str, args: []str) -> str` (`%s`
  substitution + `%%`), `sprint/sprinti` — `%v` parked (no reflection).

## Tasks

- **S2.1** rt.c/rt.h externs + Windows veneer.
- **S2.2** seed: `byte` ty, char lit, `as` convs, FFI allowlist, wrap emit.
- **S2.3** oxl: same four in lexlib/parser/check/codegen + corpus parity.
- **S2.4** `std/errors`, `std/io`, `std/os` File, `std/strconv`.
- **S2.5** `std/bytes`, `std/bufio`, `std/fmt`.
- **S2.6** run.sh stdmods-style coverage + diag corpus rows + suite +
  self-compile chain + spec markers.

## Diagnostics (normative — corpus parity)

| Condition | Message |
|---|---|
| `as` non-conv pair | `` `as` casts enum values to variant payloads — `X as Y` invalid `` (existing; byte/slice pairs added to allowed set) |
| `''` / multi-char | lex error matching existing string-escape diag shape |
| err in FFI *arg* | now allowed — prior diag retires (corpus updated) |

## Risks

- `'c'` in the self-hosted lexlib must produce identical Int tokens —
  lexlib.oxl diffs its token stream against the seed lexer's in the bootstrap
  chain; divergence = chain break, caught immediately.
- `rt_bytes_str` arena-copy vs `own` — an escaping `[]byte` needs
  `new(b)`; documented in `std/bytes` docs (B3 dangling class).
- `e == io.eof()` depends on interning — `rt_err_new` must intern by
  message, not allocate per call.
