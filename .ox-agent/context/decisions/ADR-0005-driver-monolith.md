# ADR-0005: the .oxl driver is a monolith over the compiler packages

Status: accepted (WI-0003) — 2026-11-07

## Decision
The toolchain-in-oxl driver is a single binary — `cmd/oxl` — that
imports `bootstrap/lexlib`, `bootstrap/parser`, `bootstrap/check`, and
a new `bootstrap/codegen` package directly. `bootstrap/cmd/codegen`
splits into a library package plus a thin main, mirroring the
`bootstrap/check` + `bootstrap/cmd/check` layout. A subprocess
component model (driver shells out to per-pass binaries) was rejected:
the AST has no stable serialized form and inventing one for IPC is
waste.

## Rationale
- The "one toolchain binary" language principle is verbatim: build,
  run, test, fmt, doc, LSP in one binary, no separate tools.
- Compile-mode checking runs in-process exactly as the Rust driver
  does today — no new IPC surface, no serialization format to version.
- The lib-split is a pure move: it keeps `cmd/codegen` working for the
  existing self-compile gate while making the emitter importable.

## Consequences
- `bootstrap/codegen/` becomes a `package codegen` importable by both
  `cmd/oxl` and `bootstrap/cmd/codegen`.
- Any future subprocess driver would need an AST serialization format
  designed on purpose — out of scope here.
