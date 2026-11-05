# Self-hosting readiness

The goal: the compiler, written in the language, compiled by itself.

**Done so far — the entire frontend is self-hosted:**

- `bootstrap/lexlib.oxl` — a working lexer for `.oxl`, written in
  `.oxl` (shared library: token kinds, `lex`, cursor helpers)
- `bootstrap/lexer.oxl` — thin driver over lexlib (token dump)
- `bootstrap/parser.oxl` — a recursive-descent parser for `.oxl`,
  written in `.oxl`, emitting the AST as JSON via `std/json`.
  Internals are a typed `Node` enum (B1) — `to_json` dispatches on
  variants with `match` (B2), including `match` itself in the
  grammar. **It parses itself with zero skipped tokens** — full
  round-trip.
- `bootstrap/check.oxl` — a port of the Rust typechecker, written in
  `.oxl`: interned integer type handles, scope-slice environments,
  `std/collections` tables, registration + checking passes over the
  parser's AST (fns, structs, enums, globals, imports, bodies,
  patterns/match exhaustiveness, suspension analysis). **It checks
  itself clean** — `oxl run bootstrap/check.oxl -- file.oxl` prints
  diagnostics or `ok`. The parser produces the full v0.2 surface —
  generics (`fn f[T any]`), `interface` decls, and `fn (r T) m`
  receivers — and the checker consumes them: ty-param registration,
  interface tables, structural method coverage, and `own` move
  tracking (D2) are all ported. Remaining subset: float-typed AST
  distinctions beyond literals, and `codegen.oxl` does not yet lower
  generics/interfaces/methods — those still emit through the Rust
  pipeline.
- `bootstrap/codegen.oxl` — a port of the Rust C emitter, written in
  `.oxl`, consuming the checker's tables over the parser's AST:
  `oxl run bootstrap/codegen.oxl -- file.oxl > out.c`, then
  `cc -O2 -pthread -Iruntime out.c runtime/rt.c -o out`. When `out.c`
  references `rt_sqlite_*` (sqlite stdlib in use), add `-DOXL_SQLITE`
  before the sources and `-lsqlite3` after them — `emit_and_cc`
  detects the same condition and adds both only then, since `-l`
  must trail the objects that reference it under `--as-needed`.
  Suspension lowers to the same `o_frm_*`/`o_poll_*` state machines the
  Rust emitter produces; `--test` mirrors `oxl test`'s scope-wrapped
  runner. **It compiles itself** — the emitted binary regenerates
  byte-identical C, closing the classic bootstrap chain.

All five bootstrap components are compiled by the Rust bootstrap
compiler and run in CI (`tests/run.sh`), so the self-host claim can't
rot.

## What the language already has (sufficient for frontend)

- `readfile`/`writefile`/`exit` builtins (file io + process)
- `import "std/http"` — binds the module under a namespace qualifier (`http.serve(...)`); `as` renames it and `.{name, ...}` imports select names unqualified. `std/` resolves against the toolchain stdlib
- `own` linked structures + `nil` — token lists and AST trees
- `mut`/`ref` borrows — parser cursors mutate through borrow regions
- Generics: `fn f[T constraint]` monomorphized per instantiation
- `break`/`continue`, `for`, `[v, err]` returns, `if/else`
- `interface` decls as named op constraints (`eq`, `ordered`, `add`)

## Gaps before a full self-hosted compiler lands

Ordered by what the checker/codegen port actually needs:

1. ~~**Sum types / enums**~~ — **done (B1).** `enum` decls with
   unit/positional/named payloads, `E.V(...)`/`E.V{...}`/`E.V`
   construction, `.kind()` tag access, tag-checked `as` payload casts.
   `bootstrap/parser.oxl` already uses it for its AST.
2. ~~**`match`**~~ — **done (B2).** `to_json` now pattern-dispatches
   with `match`; the grammar parses `match` too (self-parse round-trips).
3. ~~**Slices `[]T`**~~ — **done (B3).** `[]T` fat-header slices with
   `mkslice`, bounds-checked `s[i]`, doubling `push`, and shared-backing
   `s[a:b]` views — see `docs/semantics/slices.md`. The bootstrap parser
   keeps its linked `Json` spine (incremental child lists beat
   re-teaching `jadd` growth at self-parse scale); token storage can
   migrate when the checker port wants it.
4. ~~**Function values**~~ — **done (B4).** `fn(A) -> R` types,
   named-fn values via zero-env wrappers, copy-capture closures
   `{ |a: int| ... }` on `RtFnv {env, fp}` pairs, indirect calls and
   `spawn f()`. v0.2 rejects susp/generic/pair-returning fns as values
   and `==` on them — see `docs/semantics/fnvals.md`.
5. ~~**Methods / `interface` dispatch**~~ — **done (D3).** `fn (r T) m`
   receivers, structural `interface` types as `{data, vtab}` pairs,
   `w.m()` vtab dispatch, method-constraint generics — see
   `docs/semantics/methods.md`. The bootstrap codegen remains a
   documented subset: it doesn't lower method dispatch (same as
   generics) — parser and checker handle the syntax.
6. ~~**Maps/sets in stdlib**~~ — **done (B6).** `std/collections`:
   FNV-1a `map`/`set` over `[]Bucket` power-of-2 tables, `int` payloads
   (word-sized, covers env ids and counts). Needed `&`/`|`/`^` infix
   ops — added int-only `BitAnd`/`BitOr`/`Xor` at C precedence; the
   bootstrap parser's infix chain learned them too.

## Path

```
bootstrap/lexlib.oxl   (done — tokenizes .oxl, shared library)
bootstrap/lexer.oxl    (done — token-dump driver)
bootstrap/parser.oxl   (done — AST → JSON, self-parses clean)
bootstrap/check.oxl    (done — typechecks .oxl, self-checks clean)
bootstrap/codegen.oxl  (done — emits C, self-compiles byte-identical)
```

`oxl` (Rust) is now only needed to *first-compile* the self-hosted
compiler — the classic bootstrap chain is closed. The interim C
backend stays the codegen target; the state-machine transform lands
independently of self-hosting.

## Keeping the claim honest

`tests/run.sh` runs all bootstrap components in CI: the lexer over
`examples/hello.oxl`, the parser over `features.oxl` *and itself*
(asserting zero skipped tokens both ways), the checker over itself
and representative sources (asserting `ok`), and the codegen by
compiling emitted C and diffing program output against `oxl run` —
plus the chain check: `codegen.oxl` compiled by itself must emit
byte-identical C for `hello.oxl`. "The language can parse, typecheck,
and compile itself" is tested on every push.
