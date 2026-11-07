# Language Foundation — Shaping Notes

## Scope

Bootstrap the language through v0.0.1: semantics annex, de-risking
prototype, compiler skeleton (frontend → native codegen), minimal
runtime, concurrency runtime, service-shaped stdlib subset, toolchain
binary with LSP, pilot apps (CLI + HTTP CRUD/SQLite), open-source
release plumbing.

## Decisions

All design decisions live in the approved design doc —
`docs/superpowers/specs/2026-11-03-language-design.md` — including:

- Static types, explicit at boundaries, inferred inside bodies
- Dynamic ownership/borrow checking (no GC, no static borrow checker);
  hybrid reclamation; scope-contained traps; aliasing + dangling coverage
- Structured concurrency surface on stackless coroutines;
  executor-as-library
- `[v, err]` error returns; dropped error = compile error
- Constraint-interface generics; constants-only comptime; no macros
- v0.0.1 = CLI + services; GUI (all three paths) and web deferred
- Execution: single driver + parallel agent worktrees; milestone-gated

## Context

- **Visuals:** none
- **References:** see references.md
- **Product alignment:** `agent-os/product/mission.md`, `roadmap.md`,
  `tech-stack.md` — generated from the same design doc

## Standards Applied

- `global/language-principles` — the whole spec implements this standard
