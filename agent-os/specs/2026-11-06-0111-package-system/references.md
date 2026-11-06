# References for Package System + Stdlib

## Design doc (product decisions)

- `docs/superpowers/specs/2026-11-03-language-design.md` — surface
  decisions trace here; the module-ecosystem carve-out it makes is
  revised by this spec (see shape.md §Decisions).
- `agent-os/product/roadmap.md` — Phase 2 lists this work.

## Code being replaced / extended

- `oxl/src/main.rs` — `resolve_import` (39-70), `load_mod` +
  qualification (331-435), cc/link invocation incl. library ordering
  (609-700)
- `oxl/src/parser.rs`, `oxl/src/ast.rs` — `Import { path, alias,
  selective }` node; `package` clause lands alongside
- `oxl/src/lexer.rs` — keyword table (`package` joins it)
- `bootstrap/lexlib.oxl`, `parser.oxl`, `check.oxl`, `codegen.oxl` —
  self-host ports for every semantic item
- `tests/diag/` — differential corpus new diagnostics must extend

## Prior art per item

- Go modules: `go.dev/ref/mod` — `go.mod` grammar, MVS algorithm,
  pseudo-version format, `go.sum` `h1:` dirhash, `/vN` suffix rule
  (M-track, V-track)
- Go `internal/` package rule — `go.dev/doc/go1.4` §internalpackages
- `goimports` unused-import diagnosis — M3's error wording mirrors
  `go build`'s `imported and not used`
- Go linker dead-code GC — G1/G2 (`-ffunction-sections` +
  `--gc-sections`/`-dead_strip`/`/OPT:REF` equivalents)
- Go stdlib API surface — `pkg.go.dev/std` (S-track bounds)
- RE2/Thompson NFA construction — S5 (linear-time regex, no backrefs)
