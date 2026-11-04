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

Both components are compiled by the Rust bootstrap compiler and run in
CI (`tests/run.sh`), so the self-host claim can't rot.

## What the language already has (sufficient for frontend)

- `readfile`/`writefile`/`exit` builtins (file io + process)
- `import` — multi-file crates with a merged namespace + `std/` lookup
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
5. **Methods / `interface` dispatch** — today's `interface` is only an
   op-constraint vocabulary for generics, not a type.
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
bootstrap/check.oxl    (next — needs maps or long if-chains)
bootstrap/codegen.oxl  (emits C, links against runtime/)
```

At that point `oxl` (Rust) is only needed to *first-compile* the
self-hosted compiler — the classic bootstrap chain. The interim C
backend stays the codegen target through this; the state-machine
transform lands independently of self-hosting.

## Keeping the claim honest

`tests/run.sh` runs both bootstrap components in CI: the lexer over
`examples/hello.oxl` and the parser over `features.oxl` *and itself*
(asserting zero skipped tokens both ways). "The language can parse
itself" is tested on every push.
