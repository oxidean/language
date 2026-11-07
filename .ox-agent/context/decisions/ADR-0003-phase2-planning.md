# ADR-0003: Phase-2 planning shape + toolchain milestone name

Status: accepted (WI-0002) — 2026-11-07

## Decision
Phase 2 is planned as a **milestone map** — a single planning
document under `docs/specs/` giving sequence, per-item scope sketch,
entry/exit criteria, and what stays parked — rather than deep-spec'ing
only the next milestone or writing a thin charter per item.

The roadmap item "Full self-hosting" is renamed
**toolchain-in-oxl**: the compiler components (lexlib, parser, check,
codegen) already self-compile byte-identically; the remaining work is
porting the *driver* — CLI, `oxl.mod` resolution/MVS, git fetch +
module cache, fmt, doc, LSP, test runner — from Rust to `.oxl`.
"Self-hosting" misdescribes both halves: the compiler hosting is
done, and what remains is not the compiler.

## Rationale
- A milestone map is the artifact the planning request actually needs:
  it schedules everything documented-unbuilt without writing fiction-
  by-deadline charters for gated items (LLVM, web).
- Deep-spec'ing only the driver port leaves the rest of Phase 2 as
  roadmap one-liners and gives the small code-debt items no home.
- Bootstrap convention names the pre-existing toolchain the "seed";
  the milestone retires the Rust seed → "toolchain-in-oxl" is
  descriptive without inheriting the misleading term.

## Consequences
- The plan doc lives in `docs/specs/<date>-phase-2-plan/`; the
  roadmap's Phase 2 section links to it and adopts the new name.
- Small code-debt items (json surrogates, errors Is/As, fmt %v, net
  DNS, time stopgap) are filed as GitHub issues, not a doc.
- Rename sweep covers README, specs, bootstrap docs — tracked as an
  explicit task in the plan.
