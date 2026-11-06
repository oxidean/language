# oxl

A lean, fast language for CLI tools and services — Go-shaped DX on a
runtime with no tracing GC. Memory safety comes from dynamic
ownership/borrow checking — with two v0 gaps: arena-backed slices stored
in outer fields may be read after their scope closes, and ordinary `own`
dereferences don't yet receive dangling checks; concurrency is
structured (`scope`, `spawn`, channels); everything compiles ahead-of-time to native binaries through
one toolchain binary.

**Status: v0.0.1 bootstrap.** The compiler works end to end: programs in
`examples/` compile to native binaries and run today — including the
HTTP CRUD + SQLite pilot. See the roadmap below for what's v0 vs later.

## Design

- `docs/superpowers/specs/2026-11-03-language-design.md` — approved
  design (the spec everything traces to)
- `docs/semantics/` — ownership, concurrency, and compiler-interface
  contracts
- `agent-os/specs/2026-11-03-0036-language-foundation/plan.md` — the
  milestone plan and per-task status
- `docs/prototype/report.md` — de-risking benchmarks (state-machine
  frames ~96 B/task; ownership checks ≈ free)

## Building the toolchain

Requires: Rust (cargo), a C compiler (`cc`), and pthreads. Programs
that use the sqlite3 shims also need sqlite3 dev headers (`-lsqlite3`
is linked only when the emitted C calls them). Supported platforms: Linux
(x86_64), macOS (aarch64), and Windows (x86_64, via MinGW gcc or
LLVM clang — the runtime's `oxl_*` veneer in `runtime/rt.h` maps
threads/sync to SRWLOCK+CONDITION_VARIABLE and sockets to WinSock2).

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

## Language surface (v0)

```oxl
struct Point { x: int, y: int }

fn fail(b: bool) -> [int, err] {
    if b { return 0, 1 }
    return 42, nil
}

fn main() {
    [v, err] = fail(false)
    if err != nil { return }
    println(itoa(v))

    ch := mkchan()
    scope {
        spawn send(ch, 7)
        println(itoa(recv(ch)))
    }
}
```

Types `int float bool str cstr err chan` + user structs, `enum` tagged
unions with `match`, slices `[]T`, function values/closures, and methods
`fn (r T) m`; `own T` / `ref T` / `mut T` handles with runtime borrow
checks and compile-time move tracking; `[v, err]` returns with
dropped-error-as-compile-error; `defer`; `extern fn` C FFI;
`scope`/`spawn`/`go` + `spawnBlocking` structured concurrency on a
worker pool + blocking pool (suspend-capable functions compile to
stackless state machines — tasks are ~96 B frames, not threads; see
`docs/semantics/concurrency.md`); buffered channels `mkchan(n)` and
`select` multi-channel wait; `break`/`continue`; `interface` op
constraints; generics `fn f[T ordered](a: T, b: T)` monomorphized per
call site; `import "std/strings"` modules with `as` aliases and
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
print the cycle chain.

### Fetching dependencies

`oxl get <path>@<query>` fetches a module through `git` (a bare clone
plus `git archive` of the resolved commit — canonical tree bytes, never
a checkout) into the module cache, upserts the `require` line in
`oxl.mod`, and pins both `h1:` hashes in `oxl.sum`. Queries: `@latest`
(highest semver tag), `@vX.Y.Z` (a tag), or a branch/SHA (recorded as a
Go-style pseudo-version). The cache is content-addressed
(`<cache>/<escaped-path>@<version>/`, uppercase `!`-escaped like Go) and
sealed read-only; every build verifies a cached module against
`oxl.sum` — a tampered or missing entry fails with Go's
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
| `examples/smoke_test.oxl` | `oxl test` scoped test harness |
| `tests/stdlib_test.oxl` | stdlib + break/continue coverage |
| `tests/stdmods_test.oxl` | `std/encoding/json`, `std/sync`, `std/time` end-to-end |
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
oxl/          the compiler (Rust): lexer → parser → check → codegen → C → cc
runtime/       rt.c/rt.h — arenas, ObjHdr checks, scopes/traps, executor, channels
docs/          design doc, semantics annex, prototype report
std/           the standard library, written in the language (strings, testing, http, json, sync, time, os, collections)
bootstrap/     self-host components — lexlib, parser, check libraries + cmd/ drivers (docs/self-hosting.md)
oxl.mod        this repo's module manifest (`module oxl`)
examples/      pilot programs (see above)
tests/         tests/run.sh end-to-end suite + tests/diag/ checker-diagnostic corpus (runs in CI)
scripts/       commit-check.sh — commit grammar + no-AI-attribution gate
prototype/     de-risking benchmarks
agent-os/      product docs, standards, and the milestone plan
.agents/skills project skills (agentcanon layout — see AGENTS.md)
```

Planning flows through Agent OS (`agent-os/`); design exploration
happened via the brainstorming skill → `agent-os-planning` handoff.
