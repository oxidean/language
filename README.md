# language

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

Requires: Rust (cargo), a C compiler (`cc`), pthreads, sqlite3 dev
headers (runtime links `-lsqlite3`). Supported platforms: Linux
(x86_64) and macOS (aarch64); Windows is not yet supported (the
runtime is POSIX/pthread — a portability layer is future work).

```sh
cd oxl && cargo build --release && cd ..
# binary: oxl/target/release/oxl — put it on PATH for the commands below
export PATH="$PWD/oxl/target/release:$PATH"
```

`oxl build`/`run` invoke the system `cc` for the final link; set
`LANG_CC` to override (e.g. `LANG_CC=clang-18`) and `LANG_LDFLAGS`
to append extra link inputs (FFI shim objects, libraries — see
`examples/gui-todo/build.sh`). `tests/run.sh` honors the same
overrides.

## Using it

```sh
oxl build file.oxl [-o out]   # compile to a native binary
oxl run file.oxl [args...]    # compile + run
oxl test file.oxl             # run all fn test_*() in a scope each
oxl fmt file.oxl [-w]         # normalize indentation/whitespace
oxl doc file.oxl              # /// comments → markdown
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

Types `int bool str cstr err chan` + user structs; `own T` / `ref T` /
`mut T` handles with runtime borrow checks; `[v, err]` returns with
dropped-error-as-compile-error; `extern fn` C FFI; `scope`/`spawn`/`go`
+ `spawnBlocking` structured concurrency on a worker pool + blocking
pool (the state-machine transform lands behind the same semantics, see
`docs/semantics/concurrency.md`); `break`/`continue`; `interface` op
constraints; generics `fn f[T ordered](a: T, b: T)` monomorphized per
call site; `import "std/strings"` modules; top-level `let` globals
(process-wide state, initialized before `main`); builtin surface: `print`,
`itoa`, string ops, `mkbuf`/`poke*`, `argc`/`argv`, `readfile`/
`writefile`, `exit`, `mkshared`/`sget`/`sset`/`scas`/`sadd`,
`now`/`now_ms`/`usleep`, sqlite3 shims, `assert`.

## Examples

| Path | What it proves |
|---|---|
| `examples/hello.oxl` | hello world end-to-end |
| `examples/features.oxl` | structs, `own`/`mut`, `[v,err]`, FFI, scope/spawn/channels |
| `examples/cli/main.oxl` | CLI pilot: argv, string ops, error paths |
| `examples/http-crud-sqlite/main.oxl` | the flagship pilot: HTTP server + SQLite + per-connection tasks |
| `examples/smoke_test.oxl` | `oxl test` scoped test harness |
| `tests/stdlib_test.oxl` | stdlib + break/continue coverage |
| `tests/stdmods_test.oxl` | `std/json`, `std/sync`, `std/time` end-to-end |
| `bootstrap/lexer.oxl` | **self-host**: the language's lexer written in the language |
| `bootstrap/parser.oxl` | **self-host**: the language's parser written in the language — parses itself, zero skips |

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
stdlib/        the standard library, written in the language (strings, testing, http, json, sync, time, os)
bootstrap/     self-host components — lexer + parser done, self-parses clean (docs/self-hosting.md)
examples/      pilot programs (see above)
tests/         tests/run.sh end-to-end suite (runs in CI)
prototype/     de-risking benchmarks
agent-os/      product docs, standards, and the milestone plan
.agents/skills project skills (agentcanon layout — see AGENTS.md)
```

Planning flows through Agent OS (`agent-os/`); design exploration
happened via the brainstorming skill → `agent-os-planning` handoff.
