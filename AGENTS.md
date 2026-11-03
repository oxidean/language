# language

This file is the single source of truth for agent instructions in this repo.
`CLAUDE.md` is a symlink to this file. Edit only `AGENTS.md`.

## Conventions

- This repo follows the agentcanon convention: skills live in
  `.agents/skills/` (`.claude/skills` is a symlink to it). Never place
  skill folders in `.claude/skills/`.
- Agent OS is installed in `agent-os/`: coding standards live in
  `agent-os/standards/` and are indexed in `agent-os/standards/index.yml`.
  Product docs live in `agent-os/product/`; work specs (plans) live in
  `agent-os/specs/`. Slash commands live in `.claude/commands/agent-os/`.
- Planning flows through Agent OS: after a brainstorming skill session
  produces an approved design doc, hand off to `agent-os` planning —
  see the `agent-os-planning` skill.
