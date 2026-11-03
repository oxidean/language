# language

A lean, fast language for CLI tools and services — Go-shaped DX on a
runtime with no tracing GC. Memory safety comes from dynamic
ownership/borrow checking; concurrency is structured (`scope`, `spawn`,
channels); everything compiles ahead-of-time to native binaries through
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
headers (runtime links `-lsqlite3`).

```sh
cd lang && cargo build --release
# binary: lang/target/release/lang
```

## Using it

```sh
lang build file.lang [-o out]   # compile to a native binary
lang run file.lang [args...]    # compile + run
lang test file.lang             # run all fn test_*() in a scope each
lang fmt / doc / lsp            # scaffolded (phase 2)
```

## Language surface (v0)

```lang
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
structured concurrency (interim pool threads — the state-machine
transform lands behind the same semantics, see
`docs/semantics/concurrency.md`); builtin stdlib wave-1 (`print`, `itoa`,
string ops, `mkbuf`/`poke*`, `argc`/`argv`, sqlite3 shims, `assert`).

## Examples

| Path | What it proves |
|---|---|
| `examples/hello.lang` | hello world end-to-end |
| `examples/features.lang` | structs, `own`/`mut`, `[v,err]`, FFI, scope/spawn/channels |
| `examples/cli/main.lang` | CLI pilot: argv, string ops, error paths |
| `examples/http-crud-sqlite/main.lang` | the flagship pilot: HTTP server + SQLite + per-connection tasks |
| `examples/smoke_test.lang` | `lang test` scoped test harness |

Try it:

```sh
lang run examples/http-crud-sqlite/main.lang &
curl -d 'widget' localhost:8080/items
curl localhost:8080/items        # → 1|widget
```

## Repo layout

```
lang/          the compiler (Rust): lexer → parser → check → codegen → C → cc
runtime/       rt.c/rt.h — arenas, ObjHdr checks, scopes/traps, executor, channels
docs/          design doc, semantics annex, prototype report
examples/      pilot programs (see above)
prototype/     de-risking benchmarks
agent-os/      product docs, standards, and the milestone plan
.agents/skills project skills (agentcanon layout — see AGENTS.md)
```

Planning flows through Agent OS (`agent-os/`); design exploration
happened via the brainstorming skill → `agent-os-planning` handoff.
