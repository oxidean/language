# ADR-0002: version derivation from conventional subjects

Status: accepted (WI-0001) — 2026-11-07

## Decision
The release version is derived from conventional commit subjects on main:
highest bump across a push range wins; `!`/`BREAKING CHANGE` → major
(minor while major==0, preMajor convention); `feat` → minor; any other
allowed type (`fix perf revert docs chore test style refactor build ci`)
→ patch. Every push to main releases — no type filtering.

## Rationale
- Squash merges + commit-lint guarantee every main commit carries a
  parseable `type(scope)!:` subject, so the subject is a reliable,
  zero-config version signal. No manifest file to keep in sync.
- Per-merge releases are the stated requirement; filtering docs/chore
  adds a silent surprise (merged but unreleased) — rejected.
- Source of truth is git tags (semver-sorted), not a checked-in version
  file — no conflict surface, no manifest drift.

## Consequences
- A mis-typed subject produces a wrong bump (e.g. breaking work typed
  `fix` → patch release). Mitigation: commit-lint already enforces
  grammar; reviewers watch type accuracy.
- preMajor means the repo promotes to v1.0.0 only via an explicit
  manual tag — `!` on 0.x yields a minor bump.
