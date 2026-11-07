# oxl

This file is the single source of truth for agent instructions in this repo.
`CLAUDE.md` is a symlink to this file. Edit only `AGENTS.md`.

## Conventions

- This repo follows the agentcanon convention: agent-facing skills
  resolve under `.agents/skills/` (a symlink to `.ox-agent/skills`;
  `.claude/skills` and `.devin/skills` point at it). Never place
  skill folders in `.claude/skills/` — extension skills are new
  directories under `.ox-agent/skills/`.
- Project docs live in `docs/`: coding standards in `docs/standards/`,
  product docs in `docs/product/`, and the spec archive (completed
  milestone plans) in `docs/specs/`.
- Planning flows through ox-agent work items: `ox-agent start <title>`
  opens one; the staged skills drive it from brainstorm through ship —
  see the Ox-Agent toolkit block below.
- Always commit with the repo's configured git identity (the local
  user's). Never use `-c user.name`/`-c user.email` overrides or an
  agent identity.
- No AI attribution on commits or PRs: no `Co-authored-by` /
  `Generated with` / `Made-with` trailers or bot signatures for any AI
  or tool, and no invisible markers — `.cursor/rules/no-ai-attribution.mdc`.
- Commit subjects are SemVer-prefixed `type(scope): subject` and PR
  titles the same — enforced by `scripts/commit-check.sh`
  (`.githooks/commit-msg` locally, CI `commit-lint` on every PR).

<!-- ox-agent:begin -->
## Ox-Agent toolkit

This repository is managed by `ox-agent` — a repository-local AI-SDLC
runtime. Project state, skills, and contracts live under `.ox-agent/`.

- `ox-agent doctor` validates the setup; `ox-agent start <title>` opens work items.
- Work items carry state in `index.json`; drive them with
  `ox-agent transition|archive|unblock|status`.
- Human input is durable, never synchronous. When you need an answer or a
  gated approval, file `ox-agent input request` and END THE TURN —
  chat prompts expire before humans see them. Poll with
  `ox-agent input pending` / `input wait --timeout <budget>`;
  answers arrive via `input respond <id> --by <name> --answer "..."`
  (approvals: `--approve|--reject`). Escalated runs and gate hits file
  these requests automatically; a skill asks a
  question with `next_action.type: await_input` — the run stays
  dispatchable and the answer lands in the next packet under `context.inputs`.
- Skills are contracts, not free-form prompts: `.ox-agent/skills/<name>/SKILL.md`
  plus input/output `schemas/`.
- Compile the bounded packet (`ox-agent context compile <wi> --skill <s>`)
  or dispatch via `ox-agent run <wi>`; results land in `runtime/runs/`
  and are schema-validated on collection.
- Materialized skills also resolve under `.agents/skills/` for harnesses
  that scan that path.

Do not edit `.ox-agent/skills`, `schemas`, `workflows`, `pipelines`,
`lifecycle`, or `integrations` in place — they are materialized assets
tracked by `assets.lock`. Add extension skills as new directories instead.
<!-- ox-agent:end -->
