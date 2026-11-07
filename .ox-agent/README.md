# .ox-agent — project state managed by ox-agent

This directory is the project-local AI-SDLC runtime for `ox-agent`. Work items,
skills, contracts, and the event journal live here.

- `framework.yaml`, `config.yaml`, `policy.yaml` — the project's contract files; these are yours to edit.
- `skills/`, `schemas/`, `workflows/`, `pipelines/`, `lifecycle/`, `integrations/` —
  materialized assets tracked by `assets.lock`; do not edit in place. Add
  extension skills as new directories under `skills/`.
- `work/`, `state/`, `discussions/` — live project state; committed so context
  travels through PRs. `state/events.ndjson` is the append-only journal.
- `runtime/` — generated packets and runs; gitignored.

`ox-agent doctor` validates this setup. `ox-agent integrate` wires it into
agent harnesses (a managed `AGENTS.md` block plus `/.agents/skills` and
`/.claude/skills` links) so the materialized skills are discoverable during
agentic work — without it, nothing outside this directory advertises the toolkit.
