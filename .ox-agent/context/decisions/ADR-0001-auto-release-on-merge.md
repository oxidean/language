# ADR-0001: auto-release on merge to main

Status: accepted (WI-0001) — 2026-11-07

## Decision
Every merge to `main` produces a release: `release.yml` gains a `push: branches: [main]` trigger and a `version` job that computes the next semver tag; the existing release job runs the suite gate, then pushes the annotated tag and publishes — all inside one run, serialized by a `concurrency` group.

## Rationale
- GITHUB_TOKEN pushes never retrigger `on: push: tags`, so a separate tag-workflow cannot drive the existing release pipeline; version+publish must co-locate.
- Tagging AFTER the gate means a failing suite never burns a version number.
- Annotated tags match the existing v0.0.1/v0.2.0 objects.
- Per-merge releases are the stated requirement; release-please batching was rejected as scope.

## Bump semantics
`type!`/`BREAKING CHANGE` → major (minor while major==0); `feat` → minor; all other conventional types → patch. Highest bump wins across a multi-commit push.

## Consequences
- Docs/chore-only merges also cut patch releases (accepted noise; filter is a one-line change).
- CHANGELOG.md remains manually curated; GitHub auto-notes carry per-release detail.
- macos/windows toolchain assets remain out of scope (linux-x86_64 only, as today).
