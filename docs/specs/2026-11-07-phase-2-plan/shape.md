# Phase 2 — Milestone Map: Shaping Notes

## Scope

Plan every documented-but-unbuilt Phase-2 item: sequence, per-item
scope sketch, entry/exit criteria, and what stays parked with reopen
triggers. This spec produces the plan only — implementation specs are
written per-milestone when they start.

## The rename

The roadmap item "Full self-hosting (compiler compiles itself
end-to-end)" is renamed **toolchain-in-oxl** (ADR-0003/0004).

The term is wrong twice over. The compiler pipeline — lexlib, parser,
check, codegen — already compiles itself byte-identically; the classic
bootstrap chain is *closed and CI-gated* (`docs/self-hosting.md`,
which documents that finished work and keeps its name). What remains
is not the compiler: it is the *driver* — the `oxl` CLI, `oxl.mod`
resolution + MVS, git fetch + module cache + `oxl.sum`, `fmt`,
`doc`, LSP, and the test runner — still written in Rust. In bootstrap
terms the Rust implementation is the *seed toolchain*; this milestone
retires it.

## Milestones

| letter | name | summary |
|---|---|---|
| **T** | toolchain-in-oxl | port the driver to `.oxl`; cargo leaves the build path |
| **G** | portable GUI layer | widget layer in the language, DOM-mappable |
| **R** | package registry | central index + `oxl publish`/`oxl get` against it |
| **P** | polish | small tracked debt, pulled opportunistically |

Parked: LLVM backend, web target, D4 shape-generics, the parked-stdlib
list — see `parked.md`.

## Sequencing rationale

1. **T first.** Largest dogfooding payoff — every later milestone is
   built with the toolchain it ships. Also the only milestone whose
   absence falsifies "oxl is written in oxl".
2. **G second.** Charter path; gates the parked web target via the
   DOM-map guardrail.
3. **R third.** The module system shipped; a registry is an infra
   service, not a language change — it can wait until there's
   something worth publishing.
4. **P continuous.** Small debt has no gates; issues get pulled into
   whichever release they fit.

## Decisions

- Plan doc lives here (`docs/specs/2026-11-07-phase-2-plan/`); the
  roadmap's Phase-2 section names the milestones and links it.
- Small code-debt items are GitHub issues, not a doc — linkable,
  closeable, user-visible.
- `docs/self-hosting.md` stays named as-is: it documents the
  *compiler* bootstrap, where "self-hosted" is literally correct.
  Milestone references move to the new name.
