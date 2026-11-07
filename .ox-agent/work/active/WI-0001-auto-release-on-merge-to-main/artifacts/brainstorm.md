# WI-0001 brainstorm: auto-release on merge to main

## Intent (verbatim)
"Currently we dont create new releases once a change merges into main branch, I do want this to happen"

## Options

### O1 — Extend release.yml: `push: branches: [main]` + in-run version job (RECOMMENDED)
One workflow, two triggers. `version` job: tag-push → echo tag; main-push → compute next semver from commit subject(s), emit tag name. `release` job needs it, checks out that ref, suite-gates, THEN pushes annotated tag + publishes. Serialize via `concurrency: release`.
- +: single pipeline, no duplicate build steps; tag pushed after gate = no burned versions; manual `v*` tags still work; `workflow_dispatch` adds recovery path.
- -: in-run tagging is the only way around the GITHUB_TOKEN no-retrigger limit; slightly more yml.

### O2 — Separate `auto-tag.yml` pushing a tag, release.yml unchanged
- Fatal: GITHUB_TOKEN tag pushes do not fire `on: push: tags`. Requires PAT or `gh workflow run release.yml` handoff → two-run hop, loses atomicity, race-prone. Rejected.

### O3 — release-please-style accumulating release PR
- + curated changelog PR humans approve. - : infra-heavy (action + manifest + commit back to main through branch protection); user asked for per-merge releases, not batched. Rejected for now; changelog stays release-notes-based.

## Decisions locked (from context + conventions)
- Bump: `!`/BREAKING CHANGE → major (minor while v0.x — preMajor convention); `feat` → minor; all other types → patch. Every merge releases (literal ask).
- Annotated tags (matches v0.0.1/v0.2.0 objects).
- Tag AFTER suite gate inside the run.
- Latest-version source: `git tag` + `gh api` tag listing, semver-sorted (not lexical — v0.10.0 > v0.9.0).
- Multi-commit pushes: max bump across `before..after` subjects.
- Scope: keep linux-only asset as today; macos/windows artifacts = follow-up, noted.

## Open question → parked as default
Docs/chore merges still cut a patch release (noise vs literal ask). Default: release everything; one-line filter later if wanted.
