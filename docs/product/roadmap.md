# Product Roadmap

## Phase 1: MVP (v0.0.1)

- Language semantics annex: ownership rules, coroutine semantics,
  compiler interface contracts
- De-risking prototype: state-machine transform + ownership-check spike
- Compiler skeleton: lexer/parser → AST → typechecker → native codegen
- Minimal runtime: scope arenas, ownership metadata + checks,
  scope-contained traps
- `[v, err]` error returns with must-use checking; C FFI
- Integrated toolchain binary: build, run, test, fmt, doc, LSP
- Concurrency runtime: single-thread executor, scopes, channels,
  multithread work-stealing pool
- Service-shaped stdlib subset; generics (type parameters with
  constraint interfaces)
- Pilot apps under `examples/`: CLI; HTTP CRUD server + SQLite
- Open-source plumbing: MIT license, CI, release builds, docs

## Phase 1.5: v0.2 Forward Work (spec'd — `docs/specs/2026-11-04-0132-v0-2-forward/`)

- **Track A (critical path):** state-machine coroutine transform,
  `select`, buffered channels
- **Track B (self-host features):** `enum`, `match`, slices `[]T`,
  function values, `defer`, `std/collections` maps/sets
- **Track C (self-host compiler):** `bootstrap/check.oxl`,
  `bootstrap/codegen.oxl` — the full bootstrap chain
- **Track D (type depth):** `float`, `own` move tracking, methods +
  interface dispatch, shape-based generics, module qualification
- **Track E (tooling):** LSP hover/goto, AST-printer fmt, platform
  matrix CI (aarch64/macOS/Windows)
- **Track F (pilot):** GUI app — GTK4 FFI bridge first, portable layer
  remains the charter

## Phase 2: Post-v0.2

Milestone map: [docs/specs/2026-11-07-phase-2-plan/](docs/specs/2026-11-07-phase-2-plan/shape.md)

- **T — toolchain-in-oxl**: port the seed driver (CLI, module
  resolution, git fetch, fmt, doc, LSP, test runner) to `.oxl`;
  cargo leaves the build path
- **G — portable GUI layer** in the language (charter path), plus
  optional platform-toolkit bindings
- **R — package registry** / `oxl publish` against a central index
- **P — polish**: small tracked debt as GitHub issues
- Parked on triggers: LLVM backend (behind the `interfaces.md`
  codegen seam), web compilation target (JS/HTML/CSS — guardrail in
  `docs/specs/2026-11-04-0132-v0-2-forward/items/parked-web.md`), D4 shape-generics, parked-stdlib list

(The compiler already compiles itself — see `docs/self-hosting.md`;
"T" retires the Rust seed driver, not the compiler.)
