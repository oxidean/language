# WI-0002 spec: Phase-2 milestone map + toolchain rename

## Scope

Deliverable is a PR (CodeRabbit-approved, green CI) containing:

1. **`docs/specs/2026-11-07-phase-2-plan/`** — a milestone-map doc:
   - `shape.md`: scope, decisions, sequencing rationale
   - `plan.md`: per-milestone entries — scope sketch, entry criteria
     (what must be true to start), exit criteria (what "done" means),
     dependencies, and estimated track structure
   - `parked.md`: items staying parked with their reopen triggers
     (LLVM, web, D4, parked-stdlib)

2. **Milestones in the map** (sequenced):
   - **T — toolchain-in-oxl** (renamed from "Full self-hosting"):
     port the Rust driver to `.oxl`. Surface: `oxl` CLI arg parsing +
     dispatch, `oxl.mod` manifest parse + MVS build-list, git
     fetch/`~/.oxl/mod` cache/`oxl.sum` verify, fmt, doc, LSP server,
     test runner. Exit: `oxl` binary is self-built; `cargo` gone from
     the build path; full suite green under the self-built driver.
     Entry: none — can start now. Note: this is a *driver* port, not
     re-doing the (done) compiler bootstrap.
   - **G — portable GUI layer**: widget layer in the language on top
     of the GTK4 FFI pilot. Entry: after T (dogfoods the driver) or
     parallel — decide at start. Exit: reference app with real widgets;
     widget tree DOM-mappable (web guardrail).
   - **R — package registry**: central index + `oxl publish`/`oxl get`
     against it (git-fetch remains the fallback). Entry: module system
     stable (done). Exit: publish+fetch round-trip on a public instance.
   - **P — polish/papercuts**: the small code-debt batch — json
     surrogate pairs, errors Is/As design, fmt %v (needs reflection —
     likely parks), net DNS (needs getaddrinfo design), time
     blocking-worker revisit. Tracked as GitHub issues, pulled into
     releases opportunistically.

3. **Parked with triggers** (in `parked.md`): LLVM backend (reopen
   when C codegen is the measured bottleneck or a new target demands
   it), web target (reopen when G lands — DOM-map guardrail), D4
   shape-generics (existing trigger: corpus grows shared shapes),
   parked-stdlib list.

4. **Rename sweep**: "self-hosting"/"self-hosted" → accurate terms
   across README, `docs/specs/**`, `docs/product/roadmap.md`,
   `self-hosting.md` (likely renamed to `bootstrap.md` or
   `toolchain.md`), bootstrap comments. Keep "self-compile"/
   "self-hosted checker" where they describe the *compiler* chain —
   that's still literally true; replace where they mean the whole
   toolchain milestone. Roadmap Phase-2 section rewritten to name the
   milestones and link the plan.

5. **GitHub issues** for the five small-debt items (linked from the
   plan doc).

## Non-goals

- No implementation of any milestone — this WI produces the plan only.
- No re-litigating shipped decisions (module model, stdlib bounds).
- No new feature scope beyond what's already documented.

## Acceptance criteria

- `docs/specs/2026-11-07-phase-2-plan/{shape,plan,parked}.md` exist
  and cover all five Phase-2 items + deferrals + small debt.
- Roadmap Phase-2 section names the new milestones and links the plan.
- Every roadmap/parked/deferred item in
  `context/references/remaining-work.md` is either in the map, parked
  with a trigger, or filed as an issue — nothing dropped silently.
- No doc on main still uses "self-hosting" to mean the driver port;
  remaining uses refer only to the (finished) compiler bootstrap.
- GitHub issues filed for the five small-debt items.
- CI green, CodeRabbit approved, commit-lint clean.
