# v0.2 Forward Work — Implementation Plan

Every item deliberately deferred from v0.0.1, spec'd and ordered.
Per-item design sketches live in `items/`; this file is the order of
battle. Nothing here is scheduled — milestone-gated, not date-gated.

**Status (2026-11-04): spec only — no implementation started.**

Execution model: Track A (coroutine) is the critical path and gets its
own focused session. Tracks B→F are parallel-safe worktree items once
their dependencies land. Each task must pass `tests/run.sh` plus its
own acceptance criteria before its status flips.

## Task 1: Save spec documentation [done]

This folder: plan.md, shape.md, standards.md, references.md, items/.

## Track A — Coroutine runtime (critical path, sequential)

### Task A1: State-machine coroutine transform [done]

`items/a1-coroutine-transform.md`. The performance pitch: suspendable
fns become state machines; tasks are frames (~96 B measured), not
threads (~8.5 kB). Sequential because everything in Track A + the
self-hosted codegen builds on its suspend-set and frame layout.

### Task A2: `select` multi-channel wait [not started]

`items/a2-select.md`. Design doc: desired, TBD — this spec settles it.
Depends on A1 (channel ops become suspend points inside a state
machine) — doing it on pool threads would bake in the wrong
implementation twice.

### Task A3: Buffered channels [not started]

`items/a3-buffered-channels.md`. `mkchan(n)` capacity. Independent of
A1's transform mechanics but rides the same RtChan struct — schedule
after A1 lands to avoid churning the channel internals twice.

## Track B — Self-host language features (parallel-safe, ordered)

### Task B1: `enum` tagged unions [not started]

`items/b1-enum.md`. Highest leverage: the AST is a tagged-union
problem and `bootstrap/check.oxl` is blocked without it.

### Task B2: `match` [not started]

`items/b2-match.md`. Lands right behind B1 — checker/codegen dispatch
is the consumer.

### Task B3: slices `[]T` [not started]

`items/b3-slices.md`. Token/AST arrays. Parallel-safe with B1/B2.

### Task B4: function values [not started]

`items/b4-fn-values.md`. Visitor passes + stdlib sort/map. Parallel-safe.

### Task B5: `defer` [done]

`items/b5-defer.md`. Scoped cleanup. Parallel-safe, small.

### Task B6: maps/sets stdlib [not started]

`items/b6-maps.md`. `std/collections` — hash map + set over the runtime's
existing primitives. The checker's env/fn tables consume it.

## Track C — Self-host compiler (sequential after B)

### Task C1: `bootstrap/check.oxl` [not started]

`items/c1-check.md`. Typechecker in the language. Needs B1, B2, B6.

### Task C2: `bootstrap/codegen.oxl` [not started]

`items/c2-codegen.md`. Emits C against `runtime/` — the last piece
before the classic bootstrap chain (self-compiled `oxl` compiles
itself). Needs C1 + B3.

## Track D — Type-system depth (parallel-safe)

### Task D1: `float` type [done]

`items/d1-float.md`. Blocks JSON floats and any numeric service work.
Small, self-contained.

### Task D2: `own` move tracking [not started]

`items/d2-move-tracking.md`. Compile-time double-move detection —
today the checker allows passing the same `own` twice. Soundness gap,
moderate size.

### Task D3: methods + interface dispatch [not started]

`items/d3-methods-interfaces.md`. `impl`-style methods and interface-
typed values. The biggest type-system item; keep shallow (Go-style
structural interfaces, no trait solving).

### Task D4: shape-based generics [not started]

`items/d4-shape-generics.md`. Design doc's preferred instantiation
(gcshape stenciling — share code per memory layout, not per type).
Full monomorphization works today; this is a compile-time/binary-size
optimization. Can slip.

### Task D5: module qualification [done]

`items/d5-module-qualification.md`. `strings.split` namespacing instead
of merged namespace. Breaking surface change — spec'd, decide before
stdlib grows further.

## Track E — Tooling (parallel-safe)

### Task E1: LSP hover/goto + position index [not started]

`items/e1-lsp-hover.md`. The remaining DX gap.

### Task E2: AST-printer formatter [not started]

`items/e2-ast-fmt.md`. Replaces the lexical normalizer with a real
pretty-printer over the AST. Cheap once B-landed (could even be written
in `.oxl` on the self-hosted parser's AST).

### Task E3: platform matrix [not started]

`items/e3-platform-matrix.md`. CI matrix x86_64+aarch64, macOS, then
Windows; runtime portability audit (pthreads everywhere today).

## Track F — Product pilots

### Task F1: GUI pilot [not started]

`items/f1-gui-pilot.md`. Third acceptance app from the design doc —
three paths kept open in charter (platform FFI, portable layer, Qt/GTK
bridge). Spec chooses the v0.2 path. Depends on nothing technically;
scheduled last because it's the biggest single item.

### Parked (no spec — do not implement)

`items/parked-web.md`. Web compilation stays parked per the design doc.
The only required action is a guardrail: keep the widget-tree design
DOM-mappable when F1 lands.
