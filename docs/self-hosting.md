# The compiler bootstrap (self-hosted compiler)

The goal, achieved: the compiler, written in the language, compiled by
itself. This document covers the **compiler** bootstrap chain only.
Porting the *toolchain driver* (CLI, module resolution, fetch, fmt,
doc, LSP) is a separate Phase-2 milestone — **toolchain-in-oxl**
(`docs/specs/2026-11-07-phase-2-plan/`).

**Done so far — the entire frontend is self-hosted:**

- `bootstrap/lexlib/lexlib.oxl` — a working lexer for `.oxl`, written in
  `.oxl` (shared library: token kinds, `lex`, cursor helpers)
- `bootstrap/cmd/lex` — thin driver over lexlib (token dump)
- `bootstrap/parser/parser.oxl` — a recursive-descent parser for `.oxl`,
  written in `.oxl`, emitting the AST as JSON via `std/encoding/json`.
  Internals are a typed `Node` enum (B1) — `to_json` dispatches on
  variants with `match` (B2), including `match` itself in the
  grammar. **It parses itself with zero skipped tokens** — full
  round-trip.
- `bootstrap/check/check.oxl` — a port of the seed typechecker, written in
  `.oxl`: interned integer type handles, scope-slice environments,
  `std/collections` tables, registration + checking passes over the
  parser's AST (fns, structs, enums, globals, imports, bodies,
  patterns/match exhaustiveness, suspension analysis). **It checks
  itself clean** — `oxl run bootstrap/cmd/check -- file.oxl` prints
  diagnostics or `ok`. The parser produces the full v0.2 surface —
  generics (`fn f[T any]`), `interface` decls, and `fn (r T) m`
  receivers — and the checker consumes them: ty-param registration,
  interface tables, structural method coverage, and `own` move
  tracking (D2) are all ported. Remaining subset: float-typed AST
  distinctions beyond literals.
- `bootstrap/cmd/codegen` — a port of the seed C emitter, written in
  `.oxl`, consuming the checker's tables over the parser's AST:
  `oxl run bootstrap/cmd/codegen -- file.oxl > out.c`, then
  `cc -O2 -pthread -Iruntime out.c runtime/rt.c -o out`. When `out.c`
  references `rt_sqlite_*` (sqlite stdlib in use), add `-DOXL_SQLITE`
  before the sources and `-lsqlite3` after them — `emit_and_cc`
  detects the same condition and adds both only then, since `-l`
  must trail the objects that reference it under `--as-needed`.
  Suspension lowers to the same `o_frm_*`/`o_poll_*` state machines the
  seed emitter produces; `--test` mirrors `oxl test`'s scope-wrapped
  runner. **It compiles itself** — the emitted binary regenerates
  byte-identical C, closing the classic bootstrap chain.

All five bootstrap components are compiled by the seed compiler and
run in CI (`tests/run.sh`), so the self-host claim can't rot.

## What the language already has (sufficient for frontend)

- `readfile`/`writefile`/`exit` builtins (file io + process)
- `import "std/net/http"` — binds the module under a namespace qualifier (`http.serve(...)`); `as` renames it and `.{name, ...}` imports select names unqualified. `std/...` resolves against the toolchain `std/` tree; module mode (`oxl.mod` requires/replaces, `internal/`, MVS selection, and `oxl.sum` `h1:` verification — sha256 dirhash ported to pure oxl) is fully mirrored with byte-identical diagnostics
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
   `bootstrap/parser/parser.oxl` already uses it for its AST.
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
   `docs/semantics/methods.md`. The bootstrap codegen lowers method
   dispatch too — vtab pairs and generic instantiations emit through
   `codegen.oxl` the same as the seed pipeline.
6. ~~**Maps/sets in stdlib**~~ — **done (B6).** `std/collections`:
   FNV-1a `map`/`set` over `[]Bucket` power-of-2 tables, `int` payloads
   (word-sized, covers env ids and counts). Needed `&`/`|`/`^` infix
   ops — added int-only `BitAnd`/`BitOr`/`Xor` at C precedence; the
   bootstrap parser's infix chain learned them too.

## Path

```
bootstrap/lexlib/      (done — tokenizes .oxl, shared library)
bootstrap/cmd/lex      (done — token-dump driver)
bootstrap/parser/      (done — AST → JSON, self-parses clean)
bootstrap/check/       (done — typechecks .oxl, self-checks clean)
bootstrap/cmd/codegen  (done — emits C, self-compiles byte-identical)
```

The shipped `oxl` toolchain is the seed implementation — it
*first-compiles* the self-hosted compiler, closing the classic
bootstrap chain. `codegen.oxl` now lowers the full language surface,
including `RtIface`/vtable pairs for interface values and
monomorphized generic instantiations; emitted programs using generics,
interfaces, or methods compile through either pipeline. The interim C
backend stays the codegen target.

## Keeping the claim honest

`tests/run.sh` runs all bootstrap components in CI: the lexer over
`examples/hello.oxl`, the parser over `features.oxl` *and itself*
(asserting zero skipped tokens both ways), the checker over itself
and representative sources (asserting `ok`), and the codegen by
compiling emitted C and diffing program output against `oxl run` —
plus the chain check: `codegen.oxl` compiled by itself must emit
byte-identical C for `hello.oxl`.

Two more gates keep the claim honest:

- **Differential diagnostics** — the `tests/diag/` corpus (programs
  that parse in both pipelines but fail checking) asserts the seed
  checker's diagnostics equal `check.oxl`'s line-for-line and in
  order, after stripping the `file:line:col:` prefix.
- **All-four self-compile** — each bootstrap component compiled by
  `codegen.oxl` must produce identical output to `oxl run` (lexer and
  parser output compared in full, checker diagnostics on a corpus
  file); codegen's own self-compile is byte-identical.

"The language can parse, typecheck, and compile itself" is tested on
every push.
