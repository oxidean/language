# WI-0001 context: auto-release on merge to main

## Current state
- `.github/workflows/release.yml` fires ONLY on `push: tags: ["v*"]`. Jobs: cargo build → tests/run.sh gate → tarball `dist/oxl-toolchain-linux-x86_64.tar.gz` → softprops/action-gh-release@v2 with generate_release_notes.
- Workflow perms: `contents: write`. checkout uses `persist-credentials: false`.
- CI `.github/workflows/ci.yml` has separate `commit-lint` job; PR+main builds on ubuntu/macos/windows.
- Existing tags: v0.0.1, v0.2.0 — both ANNOTATED tag objects.
- No release on merge today; last release v0.2.0 predates the whole package-system spec (PRs #5-#19) and productionization (#20).

## Conventions in force
- Commit subjects/PR titles: `type(scope)!: subject`; types = feat fix perf revert docs chore test style refactor build ci (scripts/commit-check.sh).
- No AI attribution on commits/PRs (`.cursor/rules/no-ai-attribution.mdc`, enforced by commit-lint).
- Squash merges → one conventional subject per merged PR on main.

## Design constraints
- GITHUB_TOKEN tag pushes do NOT retrigger `on: push: tags` workflows → version+release must run in the SAME workflow run.
- Race between consecutive merges → serialize tagging via workflow `concurrency`.
- Bump semantics: `!`/BREAKING CHANGE → major (minor while major==0, pre-1.0 convention); feat → minor; all other types → patch. Every merge releases.
- Tag must be pushed AFTER the suite gate passes so a failed gate never burns a version.
