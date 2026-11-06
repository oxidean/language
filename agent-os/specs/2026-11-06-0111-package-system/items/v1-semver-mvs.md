# V1 — semver, pseudo-versions, MVS

## Problem

`require` lines name versions; nothing can parse, compare, or combine
them. Resolution needs Go's deterministic build list, not a solver.

## Design sketch

- Version grammar: `vMAJOR.MINOR.PATCH[-prerelease][+build]` —
  prerelease orders below release, build metadata ignored in compare
  (Go semver rules).
- Major ≥ 2: the module path *ends* `/vN` and the tag is `vN.x.y` —
  semantic import versioning, included per "full Go model".
- Pseudo-versions for non-tag pins (Go ref/mod forms): no base tag
  → `v0.0.0-yyyymmddhhmmss-<sha12>`; base tag `vX.Y.Z` →
  `vX.Y.(Z+1)-0.yyyymmddhhmmss-<sha12>` (so a commit after `v1.2.3`
  is `v1.2.4-0.…` — sorts after `v1.2.3`, before `v1.2.4`); base
  prerelease `vX.Y.Z-pre` → `vX.Y.Z-pre.0.yyyymmddhhmmss-<sha12>`.
- Compare: semver order; pseudo-version sorts after its base release
  (Go's rule — a commit after v1.2.3 beats v1.2.3, loses to v1.2.4).
- **MVS build list**: BFS from the main module's requires; for each
  module read *its* `oxl.mod` requires; per path keep the **max**
  version seen. Deterministic, no backtracking, no SAT — conflicts
  resolve to the higher floor, exactly like `go mod`.
- Surfaces: `oxl list -m all` prints the final build list (the debug
  story when resolution surprises).
- Where it lives: pure library — no I/O beyond reading `oxl.mod`s —
  so both the Rust driver and (eventually) self-hosted tooling use
  the same algorithm.

## Acceptance

- Unit corpus (Rust-side tests fine): parse/compare incl. prerelease
  and pseudo-version ordering
- Graph fixture: A→B@1.2 + A→C→B@1.1 resolves B@1.2; the B@1.1 bump
  doesn't *downgrade* anything and doesn't require C to exist
- `/vN` rule: `require x/v2 v2.1.0` against a repo whose tags jump
  v1→v2 picks the right tag line; `require x v2.0.0` (no `/v2` path)
  errors like Go does
- `oxl list -m all` on the fixture prints main + 3 modules with
  selected versions

## Depends on

M2 — versions only matter once the manifest exists.

## Risks

- Pseudo-version correctness needs a git call (`describe`) — keep the
  shell-out in V2's fetch path; V1's compare must never reach for the
  network/git by itself.
