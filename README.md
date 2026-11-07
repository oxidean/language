# oxl

[![CI](https://github.com/oxidean/oxl/actions/workflows/ci.yml/badge.svg)](https://github.com/oxidean/oxl/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/oxidean/oxl)](https://github.com/oxidean/oxl/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A lean, compiled language for CLI tools and services. Memory safety
comes from dynamic ownership/borrow checking — no tracing GC, no static
borrow checker; concurrency is structured (`scope`, `spawn`, channels);
everything compiles ahead-of-time to native binaries through one
toolchain binary.

The dynamic checking is not yet unconditional — two documented gaps
remain (`docs/semantics/ownership.md`): assigning a scope-backed slice
to an outer struct field is not checked (access after the scope closes
reads freed storage), and ordinary `own` dereferences compile to raw C
access without per-dereference dangling checks.

**Status: v0.2.0 released; the package/module system is on `main`.**
The compiler works end to end — programs in `examples/` compile to
native binaries today, including an HTTP CRUD + SQLite service and a
GTK4 todo app — and the toolchain's own lexer, parser, checker, and C
emitter are written in oxl and compile themselves (see
`docs/self-hosting.md`).

## Design

- `docs/specs/2026-11-03-language-design.md` — approved design
  (the spec everything traces to)
- `docs/semantics/` — ownership, concurrency, and compiler-interface
  contracts
- `docs/specs/` — milestone plans and per-task status
- `docs/prototype/report.md` — de-risking benchmarks (state-machine
  frames ~96 B/task; ownership checks ≈ free)

## Building the toolchain

Requires: a Rust toolchain (cargo) for the seed compiler, a C compiler
(`cc`), and pthreads. Programs that use the sqlite3 shims also need
sqlite3 dev headers (`-lsqlite3` is linked only when the emitted C calls
it). Supported platforms: Linux (x86_64), macOS (aarch64), and Windows
(x86_64, via MinGW gcc or LLVM clang — the runtime's `oxl_*` veneer in
`runtime/rt.h` maps threads/sync to SRWLOCK+CONDITION_VARIABLE and
sockets to WinSock2).

```sh
cd oxl && cargo build --release && cd ..
# binary: oxl/target/release/oxl — put it on PATH for the commands below
export PATH="$PWD/oxl/target/release:$PATH"
```

`oxl build`/`run` invoke a C compiler for the final link; set
`LANG_CC` to override (e.g. `LANG_CC=clang-18`), else the driver
probes `cc` → `gcc` → `clang` (MSVC's `cl` can't take the emitted C —
`_Thread_local`, `__sync` builtins). `LANG_LDFLAGS` appends extra link
inputs (FFI shim objects, libraries — see `examples/gui-todo/build.sh`).
Programs that touch SQLite link `-lsqlite3`; where no dev package
exists (stock Windows) `LANG_SQLITE_SRC` points at an amalgamation
`sqlite3.c` to compile instead. `tests/run.sh` honors the same
overrides.

## Using it

```sh
oxl build file.oxl [-o out]   # compile to a native binary
oxl run file.oxl [args...]    # compile + run
oxl test file.oxl             # run all fn test_*() in a scope each
oxl fmt [-w|--check] file.oxl # canonical AST pretty-print (comments kept)
oxl doc file.oxl              # /// comments → markdown
oxl get <path>@<query>        # fetch + pin a dependency (needs git)
oxl mod init <path>           # write a skeleton oxl.mod
oxl mod tidy                  # sync requires/oxl.sum with the imports
oxl mod verify                # re-hash the cache against oxl.sum
oxl mod list                  # the MVS-selected module build list
oxl list -m all               # same build list (alias)
oxl lsp                        # stdio LSP: diagnostics, hover, goto-definition
```

## Language surface

A complete program — a tiny HTTP service built on `std`, compiled by
`oxl run` to a native binary that answers `GET /hello/<who>`:

```oxl
package main

import "std/encoding/json"

import "std/net/http"

import "std/strings"

struct App { name: str }

fn greet(path: str) -> str {
    end := len(path)
    q := find(path, "?")
    if q >= 0 {
        end = q
    }
    who := substr(path, len("/hello/"), end)
    who = strings.trim(who)
    if who == "" {
        who = "world"
    }
    return "hello, " + who
}

fn (a App) route(method: str, path: str, body: str) -> http.Resp {
    if method == "GET" && strings.startswith(path, "/hello/") {
        r := json.jobj()
        json.jadd(r, "message", json.jstr(greet(path)))
        json.jadd(r, "from", json.jstr(a.name))
        return http.Resp{status: 200, body: json.tojson(r) + "\n", ctype: "application/json"}
    }
    return http.Resp{status: 404, body: "{\"error\":\"not found\"}\n", ctype: "application/json"}
}

fn main() {
    http.serve(8080, App{name: "oxl"})
}
```

Types `int float bool byte str cstr err chan` + user structs, `enum`
tagged unions with `match`, slices `[]T`, function values/closures, and
methods `fn (r T) m`; `own T` / `ref T` / `mut T` handles with runtime
borrow checks and compile-time move tracking; `[v, err]` returns with
dropped-error-as-compile-error; `defer`; `extern fn` C FFI;
`scope`/`spawn`/`go` + `spawnBlocking` structured concurrency on a
worker pool + blocking pool (suspend-capable functions compile to
stackless state machines — tasks are ~96 B frames, not threads; see
`docs/semantics/concurrency.md`); buffered channels `mkchan(n)` and
`select` multi-channel wait; `break`/`continue`; `interface` op
constraints; generics `fn f[T any]`/`fn g[T cmp.Ordered]` monomorphized
per call site; `import "std/strings"` modules with `as` aliases and
`.{name, ...}` selection; top-level `let` globals
(process-wide state, initialized before `main`); builtin surface: `print`,
`itoa`, string ops, `mkbuf`/`poke*`, `argc`/`argv`, `readfile`/
`writefile`, `exit`, `mkshared`/`sget`/`sset`/`scas`/`sadd`,
`now`/`now_ms`/`usleep`, sqlite3 shims, `assert`.

## Modules

Packages are directories of `.oxl` files that share a `package` clause;
`package main` marks an executable. A project opts into module mode by
placing an `oxl.mod` manifest at its root:

```
module github.com/me/app

oxl 0.3

require github.com/oxidean/widget v1.4.2
replace github.com/oxidean/widget => ../widget
```

Directives: `module` (this module's path), `oxl` (minimum toolchain
version), `require` (a dependency at a version, also in a parenthesized
block), and `replace` (pin a dependency to a local directory). The
toolchain walks up from the entry file to the nearest `oxl.mod`;
without one, only `std/` imports resolve.

Import resolution order:

1. `std/...` — the toolchain's shipped standard library
2. this module's path — resolved under the manifest's directory
3. required modules — longest path wins; `replace` directories first,
   then the module cache (`$OXLMODCACHE` or `~/.oxl/mod`)
4. anything else — `cannot find package`

Packages under `internal/` may only be imported by code beneath the
`internal` directory's parent. Import cycles are a compile error and
print the cycle chain. And an import the file never references is a
compile error too — unused imports do not build.

### Fetching dependencies

`oxl get <path>@<query>` fetches a module through `git` (a bare clone
plus `git archive` of the resolved commit — canonical tree bytes, never
a checkout) into the module cache, upserts the `require` line in
`oxl.mod`, and pins both `h1:` hashes in `oxl.sum`. Queries: `@latest`
(highest semver tag), `@vX.Y.Z` (a tag), or a branch/SHA (recorded as a
pseudo-version, `v0.0.0-<timestamp>-<sha>`). The cache is
content-addressed (`<cache>/<escaped-path>@<version>/`, uppercase
letters `!`-escaped) and sealed read-only; every build verifies a cached
module against `oxl.sum` — a tampered or missing entry fails with
`checksum mismatch` / `missing oxl.sum entry` diagnostics. `replace`
directories bypass cache and sums by design. Private repos work through
git's own auth (ssh agent, credential helpers); `git` is a toolchain
dependency for `get` only.

### Maintaining manifests

`oxl mod` keeps `oxl.mod`/`oxl.sum` in sync so neither is edited by
hand: `oxl mod init <path>` writes a skeleton manifest (refusing to
overwrite), and `oxl mod tidy` walks the import graph — an import no
require provides is resolved to its containing module and fetched
(longest prefix wins, then progressively shorter ones; a required
module lacking the package is re-probed at `@latest`), modules only
reached through other modules land as `// indirect` requires at the
MVS-selected version, and requires nothing imports are dropped — the
companion to unused imports being compile errors. Rewrites are
surgical: comments, replaces, and untouched lines survive verbatim.
`oxl mod verify` re-hashes every required module's cache slot against
`oxl.sum`; `oxl mod list` prints the build list like `oxl list -m
all`. `git` is needed only when tidy actually fetches — a manifest
already in sync tidies offline.

## Examples

| Path | What it proves |
|---|---|
| `examples/hello.oxl` | hello world end-to-end |
| `examples/features.oxl` | structs, `own`/`mut`, `[v,err]`, FFI, scope/spawn/channels |
| `examples/cli/main.oxl` | CLI pilot: argv, string ops, error paths |
| `examples/http-crud-sqlite/main.oxl` | the flagship pilot: HTTP server + SQLite + per-connection tasks |
| `examples/http-hello/main.oxl` | the minimal `std/net/http` service shown above |
| `examples/smoke_test.oxl` | `oxl test` scoped test harness |
| `tests/stdlib_test.oxl` | stdlib + break/continue coverage |
| `tests/stdmods_test.oxl` | `std/encoding/json`, `std/sync`, `std/time` end-to-end |
| `tests/io_test.oxl` | `std/io`, `std/bytes`, `std/bufio`, `std/strconv`, `std/fmt` end-to-end |
| `tests/util_test.oxl` | `std/path`+`filepath`, `cmp`/`sort`/`slices`/`maps`, `math`+`rand`, `unicode/utf8` |
| `tests/flaglog_test.oxl` | `std/flag` registration/usage/parse + `std/log` level/line shape |
| `tests/enc_test.oxl` | `std/encoding/{base64,hex,binary,csv}` RFC vectors + round-trips |
| `tests/hash_test.oxl`, `tests/crypto_test.oxl` | `std/hash` one-shots + `crypto/{sha256,md5,hmac}` published vectors |
| `tests/net_test.oxl`, `tests/httpclient_test.oxl` | `std/net` loopback dial/listen + `net/http` client get/post |
| `examples/gui-todo/` | GTK todo app over `extern fn` FFI (see `docs/gui-pilot.md`) |
| `bootstrap/cmd/lex` | **self-host**: the language's lexer written in the language |
| `bootstrap/parser/` | **self-host**: the language's parser written in the language — parses itself, zero skips |
| `bootstrap/check/` | **self-host**: the typechecker written in the language — checks itself clean |
| `bootstrap/cmd/codegen` | **self-host**: the C emitter written in the language — compiles itself to byte-identical C |

Try it:

```sh
oxl run examples/http-crud-sqlite/main.oxl &
curl -d 'widget' localhost:8080/items          # → {"created":"widget"}
curl localhost:8080/items                      # → {"items":[{"id":1,"name":"widget"}]}
```

## Repo layout

```
oxl/          the seed compiler (Rust): lexer → parser → check → codegen → C → cc
runtime/       rt.c/rt.h — arenas, ObjHdr checks, scopes/traps, executor, channels
docs/          design doc, semantics annex, prototype report
std/           the standard library, written in the language (strings, testing, http/net, json, csv, base64, hex, binary, sync, time, os, errors, io, bytes, bufio, fmt, strconv, path, filepath, sort, slices, maps, cmp, math, rand, unicode/utf8, flag, log, net, net/http, hash, crypto/{sha256,md5,hmac}, collections, regexp)
bootstrap/     self-host components — lexlib, parser, check libraries + cmd/ drivers (docs/self-hosting.md)
oxl.mod        this repo's module manifest (`module oxl`)
examples/      pilot programs (see above)
tests/         tests/run.sh end-to-end suite + tests/diag/ checker-diagnostic corpus (runs in CI)
scripts/       commit-check.sh — commit grammar + no-AI-attribution gate
prototype/     de-risking benchmarks
docs/          product docs, standards, semantics, and the spec archive
.ox-agent/     ox-agent runtime — work items, skills, workflows, pipelines
.agents/skills materialized agent skills (agentcanon layout — see AGENTS.md)
```

Planning flows through ox-agent work items (`ox-agent start`); design
exploration runs through the staged skills (brainstorm → specify →
multi-stage-plan → implementation → verify → ship).
