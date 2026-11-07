# WI-0001 spec: auto-release on merge to main

## Scope
Modify `.github/workflows/release.yml` so a push to `main` produces a GitHub release end-to-end. No other workflow, source, or doc changes except a CONTRIBUTING/AGENTS note if the release flow is documented there.

## Contract

### Triggers
- `push: tags: ["v*"]` — unchanged manual path.
- `push: branches: [main]` — new auto path.
- `workflow_dispatch` — recovery path (re-runs version+release for main HEAD).

### `version` job (new)
- tag ref → output `tag` = pushed tag name, `sha` = tagged commit.
- main/dispatch →
  1. fetch tags (`fetch-depth: 0` or `gh api` tag list);
  2. `latest` = highest `v\d+\.\d+\.\d+` by semver order (numeric, not lexical); fallback `v0.0.0` if none;
  3. bump = max over subjects of `github.event.before..after` (or `HEAD` for dispatch): `!`/`BREAKING CHANGE` → major (minor if major==0); `feat` → minor; else patch;
  4. output `tag` = next `vX.Y.Z`, `sha` = `github.sha`;
  5. does NOT push the tag.
- outputs consumed by `release` via `needs.version.outputs`.

### `release` job
- `needs: version`; `concurrency: {group: release, cancel-in-progress: false}` serializes the whole workflow.
- checks out `needs.version.outputs.sha` (`ref` input to checkout); builds; runs `tests/run.sh` gate; packages tarball (existing steps unchanged).
- NEW final steps (main-triggered runs only): create annotated tag object + ref via `gh api` on `sha`, then `softprops/action-gh-release@v2` with `tag_name` = computed tag.
- tag-push runs skip the create-tag step (`if: github.event_name != 'push' || !startsWith(github.ref,'refs/tags/')` equivalent) and release under the pushed tag.

### Guarantees
- A failing suite gate → no tag, no release, version unburned.
- Serialized runs → no duplicate tag computation.
- Tag push by GITHUB_TOKEN inside the run does NOT retrigger the workflow (verified semantics, no loop).

## Acceptance criteria
1. Merge a `fix:` PR → within one workflow run: annotated `v+patch` tag on the merge sha, GitHub release with tarball + auto-notes.
2. Merge a `feat:` PR → minor bump.
3. `feat!:`/`BREAKING CHANGE` → major bump (minor while v0.x).
4. Two rapid merges → two sequential runs, distinct ascending versions.
5. Manual `git tag -a v9.9.9 && git push` → release still publishes for that tag, no version job interference.
6. Suite failure on main → no tag pushed (simulate via a run where gate fails; observable in logs).
7. yaml syntax valid (`actionlint` or `python -c yaml.safe_load`), shellcheck-clean inline scripts.
