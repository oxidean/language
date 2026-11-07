---
name: ox-agent/brainstorm
description: Run structured divergent and convergent analysis for a work item while preserving disagreement and producing decision-ready options.
---

# brainstorm

Run structured divergent and convergent analysis for a work item while preserving disagreement and producing decision-ready options.

## Invocation contract

- You run against a work-item packet (JSON): `context` carries the bounded inputs, `constraints` the limits, and `output` declares where to write your result (`output.path`, relative to `.ox-agent/`) and the schema it must satisfy (`output.schema`).
- Write exactly one JSON object to `output.path`. `ox-agent` schema-validates it on collection, journals the outcome, and applies your declared `next_action`.
- `next_action.type` must be one of `advance_stage` (unit of work done — move to the next stage), `retry` (needs another pass — the run does not count as done, so the skill stays dispatchable), `escalate` (blocked on work — needs a human), `await_input` (needs a human answer — the run files a durable request and stays dispatchable), or `complete` (stage/pipeline finished). Always include a `reason`.
- On `escalate` the runtime files a durable input request and marks the item blocked — the human answers asynchronously via `ox-agent input respond`; do not wait for an in-turn reply.
- To ask the human a question without blocking the item, use `next_action.type: await_input` with the question in `reason` (add `options` for a decision request). The runtime files a durable request, the run stays dispatchable, and the answer lands in your next packet under `context.inputs`. Never rely on a synchronous host prompt — async-chat surfaces expire it before a human sees it.

## Consumes

- the problem framing in the packet plus `context.references` for prior exploration.

## Produces

- candidate directions as `artifacts` (one per option) with tradeoffs and open questions in `findings`; feeds `specify` once a direction is chosen.

## Operating rules

- Load only the current work-item packet and referenced artifacts.
- Preserve evidence and disagreement rather than inventing certainty.
- Do not exceed the permissions or paths in the input contract.
- Return a compact structured result matching the output schema.
- Promote durable decisions and blockers into the appropriate work-item artifact.

## Evaluation

Self-report quality flags in `eval` (booleans — set each true only when
genuinely satisfied): `divergent_options`, `tradeoffs_documented`, `preserves_disagreement`. `preserves_scope` reports that you stayed
inside the supplied context and constraints.
