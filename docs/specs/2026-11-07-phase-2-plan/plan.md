# Phase 2 — Milestone Map

Four active milestones, four parked. Each active entry gives a scope
sketch, entry criteria (what must be true to start), and exit criteria
(what "done" means). Implementation specs are written when a milestone
starts; this map is the schedule, not the design.

## T — toolchain-in-oxl

*Renamed from "Full self-hosting" — see `shape.md` and ADR-0003.*

Port the seed driver — today the Rust code under `oxl/src/` — to
`.oxl`.

**In scope:**
- `oxl` CLI: arg parsing, subcommand dispatch (`build`/`run`/`test`/
  `fmt`/`doc`/`get`/`mod`/`list`/`lsp`)
- Module machinery in the driver: `oxl.mod` manifest parse, MVS
  build-list, `~/.oxl/mod` cache layout, git fetch, `oxl.sum`
  `h1:` verify (sha256 dirhash already ported for the checker)
- `fmt` (AST printer exists in the seed; port or wrap)
- `doc` extraction
- `lsp` server (hover/goto — LSP plumbing is driver-side)
- `oxl test` runner orchestration
- A driver-side pipeline: invoke the self-hosted lexlib/parser/check/
  codegen components as libraries (they already self-compile; the
  driver becomes a `cmd/` over them)

**Out of scope (explicit):** compiler passes, runtime (`rt.c` stays C
— it's the platform runtime, not the toolchain), stdlib growth.

**Entry:** none — can start now.

**Exit:** `oxl` binary is built from `.oxl` sources; `cargo` is gone
from the build path (retained only as historical seed, if at all);
full `tests/run.sh` green under the self-built driver including the
bootstrap/diag gates.

**Likely tracks:** T1 driver skeleton + cmd dispatch, T2 module
machinery, T3 test runner, T4 fmt/doc, T5 LSP, T6 cargo removal +
dogfood CI.

## G — portable GUI layer

Widget layer in the language on top of the GTK4 FFI pilot
(`examples/gui-todo`).

**Scope sketch:** widget-tree abstraction with retained state,
layout model, event dispatch into `scope`/`select` concurrency,
platform backend interface with GTK4 as the first backend. The
widget tree must stay DOM-mappable — it is the agreed guardrail that
keeps the web target open (`docs/specs/2026-11-04-0132-v0-2-forward/items/parked-web.md` in the v0.2 spec).

**Entry:** after T (dogfoods the self-built driver; GUI work also
stresses LSP). Can run parallel if T is mid-flight.

**Exit:** a reference app with real widgets (beyond the pilot's
raw FFI calls); a second backend sketched or an honest written case
for why the abstraction doesn't foreclose one.

## R — package registry

Central index + `oxl publish`/`oxl get` round-trip.

**Scope sketch:** index protocol (git-backed index or minimal HTTP
service), namespace rules, publish flow (auth, semver enforcement,
`oxl.sum` integration), `oxl get` preferring the registry with git
fallback. `oxl.mod`/`oxl.sum`/MVS already exist — this is
infrastructure, not a language change.

**Entry:** module system stable (done). Realistically waits until
there are packages worth publishing — likely post-G.

**Exit:** publish → fetch round-trip on a hosted instance; lockfile
verify path unchanged.

## P — polish

Ongoing bucket for small tracked debt. Each item is a GitHub issue,
pulled into whichever release fits:

- `std/encoding/json`: `\uXXXX` surrogate-pair decode edge cases —
  [#26](https://github.com/oxidean/oxl/issues/26)
- `std/errors`: `Is`/`As` (needs error-tree + typed-target design) —
  [#27](https://github.com/oxidean/oxl/issues/27)
- `std/fmt`: `%v` (needs reflection — may park, decide in issue) —
  [#28](https://github.com/oxidean/oxl/issues/28)
- `std/net`: DNS/`getaddrinfo` (needs a resolution API design) —
  [#29](https://github.com/oxidean/oxl/issues/29)
- `std/time`: revisit blocking-worker API stopgap —
  [#30](https://github.com/oxidean/oxl/issues/30)

## Parked

See `parked.md` — LLVM backend, web target, D4 shape-generics,
parked-stdlib list. Each has an explicit reopen trigger; none is
scheduled.

## Sequence

```
T ──► G ──► R        P (continuous, alongside)
└── LLVM, web, D4, stdlib-list: parked on triggers
```
