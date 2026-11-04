# A1 — State-machine coroutine transform [critical path]

## Problem

`scope`/`spawn` semantics are final, but every spawned task is a pooled
OS thread (~8.5 kB/task measured vs ~96 B for a state-machine frame —
88×, `docs/prototype/report.md`). At service scale this is the
language's core pitch: thousands of concurrent tasks with arena
reclamation and trap isolation, at near-zero per-task cost.

## Design sketch

The transform is a codegen-level lowering of *suspendable* fns:

- **Suspend set** (already computed in `check.rs` `suspend_set`):
  transitive callees of `recv`/scope-join/`sleep`. Everything else
  stays plain C fns — zero cost.
- Each suspendable fn lowers to a **frame struct** (params + live
  locals + state counter + result slot) and a **driver fn**
  `state -> Poll` emitting `switch(state)` resumption blocks.
- Suspend points become "save state, return Pending"; channels/scope
  wakeups resume the task by pushing its frame back to the run queue.
- The executor (worker pool, same pending-count join protocol as
  today) swaps `pthread` for `poll_frame` — `runtime/rt.c` gains a
  resume path; `Task` carries `frame` not `fn(arg)`.
- `spawnBlocking` is untouched — blocking FFI keeps thread-per-task.

Seam kept honest: `concurrency.md` semantics (pending-count join,
sibling cancel, trap containment, arena bulk-free) unchanged — only
the task representation underneath swaps.

## Acceptance

- `tests/run.sh` green unchanged — semantics identical
- Frames heap-allocated once per task; RSS at 10k spawned tasks < 5 MB
  (today: threads refuse to scale past ~1k)
- Channel ping-pong benchmark: no regression vs interim pool at ≤100
  tasks; wins at ≥1k
- `spawnBlocking` unchanged; `send/recv` inside scopes behave identically
- `docs/semantics/concurrency.md` updated to describe the executor

## Depends on

Nothing — it IS the critical path. Land early in the milestone so
self-hosted codegen (C2) can target frames, not threads.

## Risks

- **Hardest compiler feature in the spec** (design doc's own words).
  Mitigation: the suspend set is already computed; frames are a
  mechanical CPS-style transform; prototype measured the endpoint.
- Local-address escapes (`&local` across suspend) — v0 has no `&`
  operator, so all locals safely live in the frame. If `&` lands
  later, address-taken locals must be marked frame-resident.
- Debugging through suspension (design doc calls this out) — accept
  v0: frame state ids map back to source lines in generated C comments.
