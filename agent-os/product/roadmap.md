# Product Roadmap

## Phase 1: MVP (v0.0.1)

- Language semantics annex: ownership rules, coroutine semantics,
  compiler interface contracts
- De-risking prototype: state-machine transform + ownership-check spike
- Compiler skeleton: lexer/parser → AST → typechecker → native codegen
- Minimal runtime: scope arenas, ownership metadata + checks,
  scope-contained traps
- Go-style `[v, err]` errors with must-use checking; C FFI
- Integrated toolchain binary: build, run, test, fmt, doc, LSP
- Concurrency runtime: single-thread executor, scopes, channels,
  multithread work-stealing pool
- Service-shaped stdlib subset; generics (Go-style type parameters)
- Pilot apps under `examples/`: CLI; HTTP CRUD server + SQLite
- Open-source plumbing: MIT license, CI, release builds, docs

## Phase 2: Post-Launch

- GUI: platform-toolkit FFI bindings, portable UI layer in the language,
  Qt/GTK bridge — plus the simple GUI pilot app
- Web compilation target (JS/HTML/CSS)
- `select`, richer stdlib, package registry
- Self-hosting (much later)
