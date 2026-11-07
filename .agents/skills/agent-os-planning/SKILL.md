---
name: agent-os-planning
description: "Route finished brainstorming into Agent OS work planning. Use after a brainstorming skill's design doc is approved, when the user asks to plan work, create a spec, or run agent-os plan-product/shape-spec — instead of the superpowers writing-plans skill."
---

# Agent OS Planning

Turn an approved design into Agent OS work plans. This is this repo's
canonical planning flow: brainstorming produces the design doc, Agent OS
produces the actionable spec.

This skill does not replace brainstorming or write design docs — it only
routes what happens after one is approved.

## Process

1. Generate `agent-os/product/` docs from the design doc if missing.
2. Capture conventions as `agent-os/standards/` entries.
3. Discuss the open design decisions before writing the spec — present
   them as a short numbered list with a recommendation per item; write
   the spec only once the user locks the choices.
4. Create a spec folder under `agent-os/specs/` with plan, shape,
   standards, and references files.
5. Present the plan for approval before any implementation.

Begin with step 1 immediately.

## Product docs

If `agent-os/product/` does not exist or its files are empty, create:

- `mission.md` — Problem / Target Users / Solution, distilled from the
  design doc's intent, users, and differentiator.
- `roadmap.md` — Phase 1: MVP (the release-gated scope), Phase 2:
  Post-Launch (deferred and parked items).
- `tech-stack.md` — implementation technologies (compiler impl language,
  codegen backend, FFI, tooling), not the product being designed.

Ask the user only what the design doc does not already answer.

## Standards

Write or update `agent-os/standards/` so future specs can inject the
project's rules: one file per topic (e.g. `global/language-principles.md`)
listing build-time conventions an implementer must follow. Update
`agent-os/standards/index.yml` with each file's one-line description.

## Spec folder

Create `agent-os/specs/YYYY-MM-DD-HHMM-<feature-slug>/` containing:

- `plan.md` — the task list. Task 1 is always "Save spec documentation".
  Remaining tasks are specific, ordered, and annotated with which can run
  in parallel worktrees vs. which are sequential critical path.
- `shape.md` — scope, decisions (point at the design doc, don't repeat
  it), context, standards applied.
- `standards.md` — full content of each `agent-os/standards/` file that
  applies to this work.
- `references.md` — paths to the design doc and any reference
  implementations studied.
- `items/<track>.md` — one file per work item or track: its scope, the
  decisions that apply, and an acceptance bar.
- `impl-<item>.md` — the detailed implementation plan, written when the
  item is picked up: file-level changes, ordering, verification steps.

Keep it lightweight — enough for a fresh agent to execute, not an essay.

## Execution notes

- Keep `plan.md` task markers (`[done]`, `[in progress]`) current as items
  ship — they are how a later session sees what remains.
- Before implementing an item, check open PRs and worktrees — a parallel
  implementation of the same item may already exist; compare approaches
  and converge on one rather than landing a second version.

## Handoff

Present the plan structure and ask: approve / adjust. Implementation
starts only after approval.

## Self-check

- `agent-os/product/` files exist and are non-empty.
- Every standard listed in `standards.md` exists in
  `agent-os/standards/` and is indexed in `index.yml`.
- `plan.md` Task 1 is "Save spec documentation" and tasks carry
  parallel/sequential annotations.
