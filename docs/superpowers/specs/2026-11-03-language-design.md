# Language Design — "language" (working name, TBD)

## Intent and the why

A new general-purpose programming language. Primary driver: keep Go's
developer experience — small spec, obvious code, instant builds — while
removing the two things that cost Go its footprint: the tracing GC and
the heavyweight runtime. First users are **internal projects and
services**; the language will be **open-sourced early**, so tooling,
docs, and error quality are first-class requirements, not afterthoughts.

## Goals

- Lean memory profile: no tracing GC, small binaries, small resident
  footprint.
- Fast builds in the Go/Zig class; fast runtime in the native class
  (AOT, no JIT).
- Go-shaped DX: explicit over clever, one obvious way, productive
  compiler errors, integrated tooling.
- Targets: native CLI and long-running services in v0.0.1; GUI and web
  (JS/HTML/CSS) in charter, deferred.
- C FFI from day one.
- Single toolchain binary: build, run, test, fmt, doc, LSP.

## Non-goals (v0.0.1)

- No GUI toolkit, no web backend (both chartered for later — see below).
- No macros, no free-form compile-time metaprogramming, no exceptions.
- No package ecosystem/registry beyond a minimal module story.

## Anti-goals (failure even if it "works")

- A borrow checker users fight (Rust-class DX friction).
- Runtime weight approaching Go's (GC + scheduler + big binaries).
- Build times approaching Rust/Swift's (rich-type-system compile cost).
- Function-colored async syntax as the only concurrency surface.

## Approaches — the decisions

Each entry lists what was considered, what it trades off, and what was
picked.

### 1. Type system — DECIDED

Options: fully inferred (slow tooling), rich/expressive (slow builds),
static with boundary annotations.

**Picked: static types, explicit at boundaries, inferred inside bodies.**
Signatures, fields, and exported API require annotations; inside function
bodies, `:=`/`let`-style local inference (Go/Rust flavor). Rationale:
known interfaces make per-module parallel and incremental compilation
cheap, LSP features become lookups instead of analysis, and static types
enable monomorphized unboxed values (which serves the memory goal).
The type system is deliberately *shallow* — strict but boring.

### 2. Memory management — DECIDED (direction)

Options: tracing GC (fails lean goal), static ownership/borrow checker
(fails DX goal), refcounting (Swift model), manual + arenas (Zig/Odin),
dynamic ownership enforcement.

**Picked: dynamic ownership + borrow checking.** Borrow/ownership rules
are enforced at runtime; violations trap deterministically (Go-style
failure model — same shape as nil-deref or out-of-bounds panics).
Precedents: Rust's `RefCell` (runtime borrow rules), Swift's dynamic
exclusivity enforcement, Vale's generational references. The compiler
elides checks it can prove statically — the borrow checker acts as
optimizer, not gatekeeper. Metadata cost: a few bytes per heap object +
a branch-predictable flag check per borrow/deref. Cross-thread shared
data requires an explicit `Shared<T>` wrapper with atomic borrow state.

Open details for the builder: exact ownership metadata layout,
`unsafe`/opt-out escape hatch design, what static checking is attempted
before falling back to dynamic, and whether checks are on in release
builds or profile-gated (recommendation: on by default, opt-out
explicit per build flag or per block — safety is the product's pitch).

**Reclamation: hybrid.** Scope arenas are the default allocation site —
bulk-freed when the scope closes, near-zero per-object cost — plus owned
heap objects for data that must escape or outlive its scope. Escaping
data is explicit and greppable; it matches the concurrency model
(scopes own their task frames).

**Violation coverage: both.** Mutable-aliasing checks (exclusivity) and
dangling use-after-free checks (generational references or equivalent).
Aliasing is the borrow-checker value the runtime cost buys; dangling-only
would be the cheap half of the product.

**Trap semantics: scope-contained.** A violation inside a scoped task
kills that task, cancels its siblings, unwinds the scope (arena frees in
bulk — no walk of half-valid objects), and propagates failure to the
parent scope, which handles it or dies itself. Go's panic is the wrong
model for services; this is supervision built into the concurrency
model. Process-kill (`abort`) remains for traps outside any scope or
corruption a scope cannot safely unwind. Requires unwind/teardown
machinery and a defined "poisoned" state for shared data held by the
dying task — runtime work, listed here so nobody is surprised.

A dedicated **language-semantics design phase** precedes compiler work:
the exact violation taxonomy and check sites are pinned there and written
up as a semantics annex to this doc.

### 3. Concurrency — DECIDED (direction)

Surface: Go-shaped — `go`/`spawn` inside a `scope` block (structured
concurrency; scope end joins/cancels children), first-class channels
that **transfer ownership** on send. Shared state only via `Shared<T>`
with dynamic checks. Fixes Go's goroutine-leak/cancellation gaps.

Implementation: **stackless coroutines + executor-as-library.**
Suspendable functions compile to state machines; task frames are
arena-sized allocations (tens–hundreds of bytes), not growable stacks.
A sequential CLI pays nothing — no mandatory runtime. Single-threaded
executor (a queue + a loop) is near-free and doubles as the future
GUI main-thread story; a work-stealing multithread pool is opt-in for
services. Task frames live in the spawning scope's arena and free in
one shot when the scope closes. Colorless-ish surface (Zig-style):
blocking-looking code, no mandatory `async`/`await` ceremony.

The suspend-capable set is determined by compiler reachability analysis
from `spawn`/`scope`/`go` sites — the surface stays uncolored; if this
proves infeasible at acceptable compile cost, the fallback (an opt-in
marker) is decided in the semantics phase, not silently. Because this
transform is the hardest feature in the spec, it gets a dedicated
**de-risking prototype milestone** before codegen architecture is
committed.

Known costs the builder must design for: the state-machine transform is
the hardest compiler feature in this spec; debugging through suspended
tasks needs tooling attention; blocking C FFI/syscalls can't suspend —
a blocking-thread escape hatch is required (Go solves the same problem).
`select`-style multi-channel wait: desired, design TBD.

### 4. Error handling — DECIDED

Options: Go multiple returns, Rust `Result`+`?`, Zig error unions + `try`,
exceptions (rejected — invisible control flow, unwind machinery).

**Picked: Go-style `[value, err]` multiple returns** with `if err != nil`
idiom. Errors are values carrying context (Go `error` semantics, not
Zig name-only sets). Two-tier model: expected failure = error values;
bugs/contract violations = traps (from §2).

One departure from Go, by design: **silently dropping an error is a
compile error** (`v, _ := f()` losing `err` is flagged). Keeps Go's
ergonomics, removes its most-patched footgun.

### 5. Generics & compile-time code — DECIDED

**Picked: Go-style type parameters with constraint interfaces.**
Implementation preference: shape-based instantiation sharing
(Go's gcshape stenciling — monomorphize per memory layout, not per
type) to bound compile time and binary size; full monomorphization is
an acceptable v1 simplification. Compile-time evaluation allowed for
constants only (array sizes, lookup tables). **Anti-goal: macros and
arbitrary compile-time code.**

### 6. Targets and version scope — DECIDED

- **v0.0.1**: native CLI + services *as a product scope* — it is the
  domain the first release serves, not a gate requiring every decided
  feature. Feature landing is staged at planning time (compiler + core
  semantics + errors + FFI first is the natural first slice; generics,
  coroutines, and full LSP need not all land in the first tag). Platform
  matrix TBD by builder (recommendation: x86_64 + aarch64, Linux/macOS
  first, Windows next).
- **Post-v0.0.1, in charter**: GUI — all three paths intentionally kept
  open: (a) FFI bindings to platform-native toolkits, (b) a portable UI
  layer written in the language itself (retained widget tree over a
  portable renderer; the design must keep "main-pinned task owns UI" as
  a first-class concurrency concept), (c) FFI bridge to Qt/GTK.
- **Post-v0.0.1, parked**: web compilation (JS/HTML/CSS). Nothing in
  this design should preclude it — a future widget tree maps to DOM the
  same way it would to a GPU surface. Not designed now.

### 7. Toolchain & bootstrap — DECIDED

- One binary: build, run, test, fmt, doc, LSP. Non-negotiable.
- C FFI from day one (ecosystem bridge).
- Compiler implementation language: Zig preferred (lean ethos, trivial
  C interop, LLVM or self-hosted codegen), Rust acceptable. Builder
  settles this; self-hosting is a much-later milestone.
- Developed in the open per "open-sourced almost instantly": license
  (my call: MIT), CI/release plumbing, and contribution docs land in
  early milestones rather than pre-release scramble.

**Resourcing:** milestone-gated, never date-gated. Execution model is a
single driver + effectively unbounded parallel agent worktrees. The
critical path stays sequential (semantics phase → de-risking prototype
→ compiler skeleton); afterward work fans out (stdlib, LSP, fmt, doc,
runtime hardening, CI, docs) as parallel tracks. The semantics phase
therefore owes the plan one extra artifact: **interface contracts**
(lexer/parser/AST → typechecker → IR → codegen → runtime ABI) crisp
enough that worktrees can build against them independently.

## What's left to the builder

- Language name and repo/org branding.
- Syntax surface (brace-family assumed for familiarity; not specified).
- Ownership metadata layout, `unsafe` boundary, release-check policy (§2).
- Coroutine frame allocation strategy, executor API surface (§3).
- Stdlib scope for v0.0.1 (suggest: fmt, io, os, net/http, sync, time,
  encoding/json, testing — the service-shaped subset).
- Module/package model minimal viable shape.
- Platform matrix (§6).
- Whether `select` ships in v0.0.1 or follows.

## Success and failure

**Pilot apps** (live under `examples/`, built when the language is ready):
- **v0.0.1 gate**: a simple CLI, and an HTTP CRUD server backed by
  SQLite via C FFI (libsqlite3). The CRUD server is the real acceptance
  test — it exercises the toolchain, HTTP, JSON, scopes + channels,
  the error model, and FFI in one small real program.
- **GUI-milestone gate**: a simple GUI app, exercising whichever GUI
  path ships first.
- v0.0.1 ships when the CLI and CRUD server run in production-shaped
  conditions — not before, not to a date.

**Success looks like:** the pilot apps running for real; public repo
with docs and a toolchain a stranger can install; builds fast enough
that users don't think about builds.

**Failure looks like:** users fighting ownership rules the way they
fight Rust's borrow checker; binaries with a runtime story like Go's;
a compile step anyone describes as slow.
