# Contributing

The language is at v0.0.1: a bootstrap-stage project where the compiler
(in `oxl/`, Rust) emits C99 through `runtime/` into a native binary.

## Build & test

```sh
cargo build --manifest-path oxl/Cargo.toml   # the `oxl` toolchain
bash tests/run.sh ./oxl/target/debug/oxl    # end-to-end suite
```

`tests/run.sh` exercises every example, `oxl test`, the stdlib modules,
and the HTTP/SQLite pilot (needs `sqlite3` + `libsqlite3-dev`, curl).
The parser and LSP checks also invoke `python3`, so have Python 3 on
`PATH` before running the suite.

## Layout

- `oxl/` — the compiler: `lexer → parser → check → codegen(C) → cc`
- `runtime/` — rt.h/rt.c: arenas, ownership checks, scopes, pool executor
- `stdlib/` — standard library, written in the language itself
- `bootstrap/` — self-host components (lexer, parser, checker, codegen — see `docs/self-hosting.md`)
- `examples/` — pilot apps (cli, http-crud-sqlite, gui-todo)
- `docs/semantics/` — the contracts everything is built against
- `agent-os/` — product + spec docs (Agent OS planning artifacts)

## Rules of the road

- The C backend is interim — keep the codegen seam clean for the
  state-machine transform (see `docs/semantics/interfaces.md`).
- Stdlib code goes in `stdlib/*.oxl`, written in the language.
- Commits use your configured git identity (see `AGENTS.md`).
- Bugs: file an issue with the smallest `.oxl` repro you can make.
