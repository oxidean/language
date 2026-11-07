# Ox-Agent skills

Official Ox-Agent skills live here. Each skill package must contain:

- `skill.yaml` metadata
- `SKILL.md` instructions
- `schemas/input.schema.json`
- `schemas/output.schema.json`
- `evals/evals.yaml`

Projects consume these skills from the version-pinned `ox-agent` binary, which materializes them into `.ox-agent/skills` at `init`. They must not patch installed skills locally.
