# WI-0002 plan: Phase-2 milestone map + rename

## Stage 1 — branch + plan doc skeleton
- Branch `docs/phase-2-plan` off main.
- `docs/specs/2026-11-07-phase-2-plan/shape.md` — scope, the
  toolchain-in-oxl naming decision (link ADR-0003/0004), sequencing
  rationale.
- `plan.md` — T/G/R/P milestone entries: scope sketch, entry/exit
  criteria, dependencies.
- `parked.md` — LLVM, web, D4, parked-stdlib with reopen triggers.

## Stage 2 — rename sweep
- Inventory every "self-host"/"self-hosted" occurrence; classify
  compiler-bootstrap (keep) vs toolchain-milestone (rename).
- `docs/self-hosting.md` → retitle to reflect it documents the
  (finished) bootstrap chain; milestone references point at the new
  name.
- README + roadmap Phase-2 section rewritten: new milestone names,
  link to the plan.
- Sweep specs/comments for stale "full self-hosting" phrasing.

## Stage 3 — file small-debt issues
- `gh issue create` ×5: json surrogate pairs, errors Is/As, fmt %v,
  net DNS, time blocking-worker. Label `debt`/`polish`; link from
  plan.md P-milestone.

## Stage 4 — verify + PR + review loop
- Docs-only diff: no suite impact, but run commit-lint-relevant checks
  (subject format, no attribution markers) and confirm markdown links
  resolve.
- PR `docs: phase-2 milestone map + toolchain-in-oxl rename`; drive
  CodeRabbit threads to zero + CI green; merge on approval.

## Risks
- Rename sweep over-matches: "self-hosted compiler/checker" is correct
  and must be kept — classify each hit before editing.
- Issue-filing needs `gh` auth — available (used all session).
- Spec dir name embeds today's date; keep consistent with the
  2026-11-06-0111-package-system convention (date-seq-slug).
