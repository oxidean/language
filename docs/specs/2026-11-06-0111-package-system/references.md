# References for Package System + Stdlib

## Design doc (product decisions)

- `docs/specs/2026-11-03-language-design.md` — surface
  decisions trace here; the module-ecosystem carve-out it makes is
  revised by this spec (see shape.md §Decisions).
- `docs/product/roadmap.md` — Phase 2 lists this work.

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

- Versioned-module model (the upstream design M/V tracks implement):
  `go.dev/ref/mod` — manifest grammar, MVS algorithm, pseudo-version
  format, `h1:` dirhash, `/vN` suffix rule
  (M-track, V-track)
- `internal/` package-visibility rule — `go.dev/doc/go1.4`
  §internalpackages
- `goimports` unused-import diagnosis — M3's error wording mirrors
  the `imported and not used` compile error
- Linker dead-code GC prior art — G1/G2 (`-ffunction-sections` +
  `--gc-sections`/`-dead_strip`/`/OPT:REF` equivalents)
- The core-library API surface the S track maps — `pkg.go.dev/std`
- Thompson NFA construction — S5 (linear-time regex, no backrefs)
