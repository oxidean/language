# Toolchain-in-oxl driver inventory (WI-0003 discovery)

> Scope per ADR-0004: T ports the *driver* only. Compiler passes
> (lexlib/parser/check/codegen) already self-compile and are imported
> as `.oxl` packages — out of scope as work, in scope as dependencies.

## Rust surface to port (~5.4k lines)

| File | Lines | Ports to |
|---|---|---|
| `oxl/src/main.rs` | 3405 | CLI dispatch (`build/run/test/fmt/doc/get/mod/lsp`), `pkg_args`/`pkg_src`, `load_pkg` + module discovery/`internal/`/cycle machinery, `compile_mode`, `emit_and_cc` (flag ordering: sources→rt.c→`-lsqlite3`/`-lm`/`-pthread`/`--gc-sections`, Windows set), `mk_scratch_dir`, `cmd_mod` subcommands (init/tidy/verify/list), LSP (`cmd_lsp` is inline — JSON-RPC `Content-Length` loop + `analyze` + publish-diagnostics scoping) |
| `oxl/src/fmt.rs` | 984 | formatter — must re-express over `parser.oxl`'s `Node` AST, not Rust's |
| `oxl/src/modcache.rs` | 481 | `~/.oxl/mod` cache layout, extract/verify/list, `OXLMODCACHE` env hook |
| `oxl/src/semver.rs` | 208 | semver parse + MVS build list |
| `oxl/src/fetch.rs` | 171 | git veneer: `clone --bare` + `git archive` extract, URL map (scheme/`git@`→ssh) |
| `oxl/src/diag.rs` | 119 | `file:line:col` + source-line + caret rendering |

Not ported (seed only): `check.rs`/`codegen.rs`/`parser.rs`/`lexer.rs`/`ast.rs`.

## Existing .oxl dependencies (import, don't rewrite)

- `bootstrap/lexlib` (282), `bootstrap/parser` (1244, `package parser`), `bootstrap/check` (6708, `package check`), `bootstrap/codegen` logic (5327 inside `bootstrap/cmd/codegen`, needs lib-split or import).
- std coverage is sufficient: os.files, io, bufio, bytes, strings, strconv, encoding/json (`jget`/`tojson`/`jadd` builders + parse), flag, path/filepath, sort/maps/collections, sync, time.

## Language gaps (ADR-0004: new work, spec explicitly)

1. **Process exec** — none in `std/os`/`rt.c`. Driver shells out to `cc` (status-only, inherited stdio), `git` (stdout capture via pipe), and the test binary (status). Needs POSIX `posix_spawn`/fork+execvp+`waitpid` + pipe, Windows `CreateProcess`+`WaitForSingleObject` via the E3 veneer. Two flavors: `run` (inherit stdio, return exit code) and `output` (capture stdout+stderr, return exit code).
2. **fs primitives** — `create_dir_all` (modcache, scratch), recursive remove (scratch cleanup), `rename` (modcache tmp→final), `tempdir`. Already externs in rt.c used by check.oxl but unwrapped in `std/os`: `fexists/fullpath/getenv/isdir/isfile/islnk/lsdir`.
3. **LSP `check::Index`** — `oxl/src/check.rs:119` builds a position→symbol index during checking; `check.oxl` has no equivalent. LSP port depends on porting the index machinery (largest single sub-piece; candidate for last stage).

## Bootstrap-of-the-bootstrap

New `oxl` = `package main` (e.g. `bootstrap/cmd/oxl/`) importing the
compiler packages + std. First build uses the Rust seed; exit criterion
(milestone map): self-built `oxl` passes `tests/run.sh` as the driver
with cargo out of the *product* build path (seed stays for bootstrap).

## Verification surface

`config.yaml` test_command: `cargo build --release --manifest-path
oxl/Cargo.toml && tests/run.sh oxl/target/release/oxl`. Dogfood gate:
run the same suite with the self-built driver (`tests/run.sh <new-oxl>`)
— byte-identical `oxl doc`/`fmt`/`check` output, plus mod/get coverage.
