# ADR-0004: milestone T is a driver port, not compiler work

Status: accepted (WI-0002) — 2026-11-07

## Decision
The Phase-2 milestone map letters items T/G/R/P (Track-style, matching
the v0.2 spec's A–F convention). **T — toolchain-in-oxl — is scoped to
the driver**: CLI dispatch, `oxl.mod` resolution + MVS, git fetch +
module cache + `oxl.sum`, fmt, doc, LSP, test runner. The compiler
pipeline itself is explicitly out of scope — lexlib/parser/check/
codegen already self-compile and need no porting.

## Rationale
- "Self-hosting" made the milestone sound like compiler work; the
  rename must not smuggle the compiler back into scope. Naming the
  seed/driver boundary once, durably, prevents re-scope at
  implementation time.
- Letter codes keep item references short in plans, branches, and PR
  titles (`feat: …(T1)`, `docs(phase-2): …`).

## Consequences
- Spec text for T names the excluded surface (compiler passes,
  runtime, stdlib growth) alongside the included one.
- If implementation later uncovers driver code that can't yet be
  expressed in `.oxl`, that's a language-gap finding — new work, not
  silent scope creep.
