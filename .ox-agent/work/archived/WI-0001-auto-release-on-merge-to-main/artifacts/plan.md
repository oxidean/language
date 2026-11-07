# WI-0001 plan: auto-release on merge to main

## Stage 1 — workflow rewrite (implementation)
Files: `.github/workflows/release.yml` only.
- Add triggers: `push: branches:[main]`, `workflow_dispatch`.
- `concurrency: {group: release, cancel-in-progress: false}` at workflow level.
- New `version` job (ubuntu): tag-ref → echo; else compute next tag via `gh api` tag list + subject scan of push range; outputs `tag`,`sha`.
- `release` job: `needs: version`, checkout `sha`, existing build/gate/package unchanged, then (non-tag runs) `gh api` annotated tag+ref, `action-gh-release` with `tag_name`.

## Stage 2 — local verification (verify)
- `actionlint` (or python yaml+shell syntax) on the file.
- Extract the bump logic; run against real history: v0.2.0 + `fix:` → v0.2.1; `feat:` → v0.3.0; `feat!:` → v0.3.0 (preMajor); multi-commit range picks max.
- Dry-run `gh api` tag-object payloads locally (no push).

## Stage 3 — PR + review loop (ship)
- `feat(ci): release on merge to main` PR; drive CodeRabbit threads + CI to green.
- Post-merge observation: the merge itself triggers the pipeline → first auto-release validates the whole chain live.

## Risks
- `before` sha on force-push/first-push may be absent → guard fallback to HEAD~1/HEAD.
- `gh api` tag endpoints need `contents: write` — already top-level.
- action-gh-release on tag-push path must keep working (regression check via `if:` scoping).
