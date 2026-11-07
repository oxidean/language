# v0.2 Forward Work — Shaping Notes

## Scope

Everything intentionally out of v0.0.1's scope, spec'd to execution
depth: the coroutine transform (critical path), the language features
the self-hosted compiler needs (enum/match/slices/fn values/defer/maps),
the self-hosted `check`/`codegen` pair, type-system depth (float, move
tracking, methods, shape-based generics, module qualification), tooling
(LSP hover, AST fmt, platform matrix), and the GUI pilot.

## Decisions

All product decisions remain governed by
`docs/specs/2026-11-03-language-design.md`. This spec adds
*implementation* decisions per item (in `items/`) — surface syntax,
runtime ABI, acceptance tests — without reopening product questions.
Where a design-doc TBD gets settled here (only `select`), it's called
out explicitly in the item file.

Cross-cutting rules for every item:

- Keep the shipped semantics: scope join by pending count, trap
  containment, channel discipline — the interim pool executor already
  implements the *final* semantics; the transform changes mechanics,
  not meaning.
- `unsafe` stays the opt-out for ownership checks; no item may weaken
  that boundary silently.
- Stdlib stays written in the language wherever possible.
- Interim choices stay behind clean seams (C-emit backend, pool
  executor, monomorphization) — each item that replaces one must keep
  the seam honest, per `docs/semantics/interfaces.md`.
- Every item lands with `tests/run.sh` coverage — the self-host
  components additionally get a self-application assertion (lexes/
  parses itself, like the existing ones).

## Context

- v0.0.1 tagged and released; `tests/run.sh` green; CI + release
  workflow live.
- Ordering rationale and per-item dependencies: `plan.md`.
- Measured motivation for the coroutine transform:
  `docs/prototype/report.md` (88× per-task memory, checks ~free).
- Self-host gap analysis this spec implements:
  `docs/self-hosting.md`.

## Standards Applied

- `global/language-principles` — applies to every item
