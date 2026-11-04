# Self-hosting readiness

The goal: the compiler, written in the language, compiled by itself.
`bootstrap/lexer.lang` is the first shipped component — a working lexer
for `.lang` source, written in `.lang`, compiled by the Rust bootstrap
compiler. It proves the language already expresses compiler-shaped code:
file io, byte-level classification, owned linked structures, recursion.

## What the language already has (sufficient for the lexer)

- `readfile`/`writefile`/`exit` builtins (file io + process)
- `import` — multi-file crates with a merged namespace + `std/` lookup
- `own` linked structures + `nil` — trees and lists without generics
- Generics: `fn f[T constraint]` monomorphized per instantiation
- `break`/`continue`, `for`, `[v, err]` returns, `if/else`
- `interface` decls as named op constraints (`eq`, `ordered`, `add`)

## Gaps before a full self-hosted compiler lands

Ordered by what the parser/codegen port actually needs:

1. **Sum types / enums** — the AST is a tagged-union problem; modeling it
   with struct-per-node + kind ints works (the lexer does) but is
   laborious. A `enum` decl is the highest-leverage next feature.
2. **`match`** — the parser/checker dispatch on kinds constantly;
   if-chains work, match is ergonomic pressure relief.
3. **Slices `[]T`** — token streams want contiguous arrays, not linked
   lists. `mkbuf`/`poke8` is the escape hatch today.
4. **Function values** — the parser's recursive descent doesn't need
   them, but visitor-style AST passes and stdlib `sort`/`map` will.
5. **Methods / `interface` dispatch** — today's `interface` is only an
   op-constraint vocabulary for generics, not a type. Real dynamic
   dispatch arrives with the deeper type-system work.
6. **`defer`** — scoped cleanup in a compiler pass that allocates
   per-node; arenas already make this tolerable.

## Path

```
bootstrap/lexer.lang    (done — tokenizes .lang)
bootstrap/parser.lang   (next — needs no new features, just volume)
bootstrap/check.lang
bootstrap/codegen.lang  (emits C, links against runtime/)
```

At that point `lang` (Rust) is only needed to *first-compile* the
self-hosted compiler — the classic bootstrap chain. The interim C
backend stays the codegen target through this; the state-machine
transform lands independently of self-hosting.

## Keeping the claim honest

`tests/run.sh` runs `bootstrap/lexer.lang` over `examples/hello.lang`
in CI — the self-host component is tested on every push, so "the
language can compile itself" never rots.
