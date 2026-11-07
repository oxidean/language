# Contributing

oxl is a young language — the toolchain works end to end and the
compiler's own components are written in oxl, but expect rough edges.
The seed compiler (in `oxl/`, Rust) emits C11 through `runtime/` into a
native binary; the self-hosted components under `bootstrap/` must stay
byte-for-byte in step with it.

## Build & test

```sh
cargo build --release --manifest-path oxl/Cargo.toml   # the `oxl` toolchain
bash tests/run.sh ./oxl/target/release/oxl            # end-to-end suite
```

`tests/run.sh` exercises every example, `oxl test`, the stdlib packages,
the self-hosted checker/codegen chain, the diagnostic-parity corpus,
module-resolution fixtures, and the HTTP/SQLite pilot (needs `sqlite3` +
`libsqlite3-dev`, curl). The parser and LSP checks also invoke `python3`,
so have Python 3 on `PATH` before running the suite.

## Layout

- `oxl/` — the seed compiler: `lexer → parser → check → codegen(C) → cc`
- `runtime/` — rt.h/rt.c: arenas, ownership checks, scopes, pool executor
- `std/` — standard library, written in the language (`std/strings`,
  `std/io`, `std/net/http`, …)
- `bootstrap/` — self-host components (`lexlib`, `parser`, `check`
  libraries + `cmd/` drivers — see `docs/self-hosting.md`)
- `examples/` — pilot apps (cli, http-crud-sqlite, gui-todo)
- `docs/semantics/` — the contracts everything is built against
- `tests/diag/` — diagnostic fixtures; both checkers must emit identical
  diagnostics for every one
- `agent-os/` — product + spec docs (Agent OS planning artifacts)

## Keeping the two implementations in parity

Changes to language semantics must land in **both** the seed toolchain
(`oxl/src/*.rs`) and the self-hosted components (`bootstrap/**/*.oxl`),
and their observable behavior must match: same diagnostics byte-for-byte
(`tests/diag/`), same emitted C where the harness compares it. When you
touch the checker, run the diag corpus; when you touch codegen, run the
self-compile comparison. `tests/run.sh` covers both.

## Commits

- Subjects are SemVer-prefixed: `type(scope): subject` — types
  `feat`, `fix`, `perf`, `revert`, `docs`, `chore`, `test`, `style`,
  `refactor`, `build`, `ci`. Exempt: `Merge …`, `Revert "…"`,
  `chore(release): …`.
- No AI/tool attribution: no `Co-authored-by` / `Generated with` /
  `Made-with` trailers or bot signatures for any AI or tool, and no
  invisible markers. Human co-authors and `Signed-off-by` are fine.
  Policy: `.cursor/rules/no-ai-attribution.mdc`.
- Enforcement is `scripts/commit-check.sh`: CI `commit-lint` checks the
  PR range, title, and body on every pull request (the CodeRabbit-owned
  release-notes block in the body is exempt). To get the same check
  locally on each commit:

  ```sh
  git config core.hooksPath .githooks   # installs .githooks/commit-msg
  ```

  `git commit --no-verify` bypasses the hook; CI still checks.
- Commits use your configured git identity (see `AGENTS.md`).

## Rules of the road

- The C backend is interim — keep the codegen seam clean behind the
  emitted state machines for suspend-capable functions (see
  `docs/semantics/concurrency.md`).
- Stdlib code goes in `std/`, written in the language.
- Bugs: file an issue with the smallest `.oxl` repro you can make.
