# WI-0002 brainstorm: plan remaining work + rename "self-hosting"

## The naming problem

"Full self-hosting" is wrong twice over: the compiler already
self-compiles (bootstrap chain, byte-identical, tested in CI), and the
remaining work is not the compiler at all — it's the *driver* (CLI,
module resolution, git fetch, fmt, doc, LSP) still in Rust. Bootstrap
terminology already has a word for this: the Rust driver is the **seed
toolchain**. Candidate names:

| name | reads as | tradeoff |
|---|---|---|
| **seed-toolchain retirement** | "retire the Rust seed" | accurate, evocative; jargon-y to outsiders |
| **toolchain-in-oxl** | "the toolchain, written in oxl" | descriptive, parallels "compiler-in-oxl" which is done |
| **driver port** | "port the driver to oxl" | exact; undersells it as a milestone |
| **full bootstrap** | "finish the bootstrap" | collides: bootstrap chain already exists |
| **oxl-on-oxl** | cute | vague |

Recommendation: **toolchain-in-oxl** for the spec/milestone name —
self-documenting, and honestly scoped: everything needed to build and
develop oxl is itself oxl.

## Plan-shape directions

### A. Phase-2 milestone map (recommended)
One planning doc: sequencing + per-item scope sketch + entry/exit
criteria + what stays parked. Deliverable is the plan itself in a PR —
no implementation. Items become WIs as they start.

- Pro: matches the ask ("plan the changes"); bounded diff; reviewable
- Con: thinner than a full spec for whichever item lands first

### B. Deep-spec the driver port only
Full spec for toolchain-in-oxl (the clear next milestone), leave
GUI/registry/LLVM/web as roadmap one-liners.

- Pro: next milestone becomes executable immediately
- Con: doesn't "plan the changes you described" — plans only one of
  them; small-debt items get no home

### C. Charter set — one thin doc per item
Five small docs, one per Phase-2 item, shape-level scope only.

- Pro: uniform treatment
- Con: speculative docs rot; LLVM/web/reg aren't actionable yet, so
  most charters are fiction-by-deadline

## Sequencing logic (invariant across A/B/C)

1. **toolchain-in-oxl** first — biggest dogfooding payoff; every later
   milestone benefits from a self-built toolchain. Also the only item
   whose absence blocks "oxl is written in oxl" claims.
2. **GUI portable layer** second — charter path, gates web target.
3. **registry** third — module system exists; registry is an infra
   service, not a language change.
4. **LLVM backend** — parked behind codegen seam; reopen when C
   codegen is the bottleneck or a target needs it.
5. **web target** — parked until GUI layer exists (DOM-map guardrail).
6. **small debt** — bundled as a "polish" milestone or tracked issues;
   none gate anything.

## Open questions for specify

- Does the plan doc live in `docs/specs/` (a Phase-2 plan spec) or
  `docs/product/roadmap.md` rewritten? → recommend a spec dir like
  prior phases, roadmap links to it.
- Small debt → GitHub issues vs. a `docs/known-debt.md`? → recommend
  issues: they're user-visible, linkable, closeable.
- Rename blast radius: "self-hosting" appears in README, specs,
  bootstrap docs, tests — plan includes the sweep.
