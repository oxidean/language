# De-risking Prototype — Report

Date: 2026-11-03. Benchmarks: `prototype/` (`make run`), GCC 16.1.1 -O2,
WSL2 x86_64.

## Question 1: does a stackless task frame beat a task stack on memory?

Modeled a suspend-capable function (3 suspend points, 4 live locals +
ObjHdr) as a compiled `TaskFrame` vs a parked pthread, 2048 tasks each.

| | per-task memory |
|---|---|
| `TaskFrame` struct (static) | 48 B |
| `Task` (frame + step fn) | 56 B |
| 2048 stackless tasks (measured RSS) | ~96 B/task |
| 2048 pthread tasks (measured RSS, live) | ~8.5 kB/task |
| pthread default stack reservation | 8 MB (reserved, not committed) |

**~88×** less resident memory per suspended task, measured. Real frames
grow with locals-per-suspend-point, but the order-of-magnitude holds —
the design doc's "tens-to-hundreds of bytes vs kilobytes" claim
validates.

## Question 2: what do dynamic ownership checks cost per access?

`ObjHdr` = `{u32 gen, u8 borrow, u8 arena}` (8 B). Checked read = gen
compare + borrow-flag compare + payload load, branch-predictable.

| | ns/op |
|---|---|
| unchecked access | ~1.17–1.20 |
| checked access | ~1.19–1.21 |
| overhead | −0.7%…+3.8% — **within measurement noise** |

Two well-predicted branches sit under the load the access already pays.
On this synthetic read-only generation+borrow check the cost is
effectively free on hot paths — whether the full runtime check
(`rt_borrow_read` additionally tests poison and writes borrow state)
shares that profile is unmeasured. The real cost is the object
header per heap object — 24 bytes on x86_64 (`runtime/rt.h`'s `ObjHdr`:
gen + borrow/arena/poisoned + the scope heap-list links; the synthetic
8-byte header in `check_cost.c` only models the check fields, not the
full runtime layout) — and the checks that survive into cold paths.
Both bounded. Elision (proving safe sites statically) remains upside,
not a requirement.

## Verdict: GO

- Per-task memory and check overhead both land inside the design doc's
  bounds; nothing about the ownership/concurrency model needs redesign.
- The transform's *compile-time* cost is the remaining risk, deferred to
  the production implementation — the prototype measured runtime
  characteristics, which is where the user-facing claims live.
- Interim plan stands: pool-thread tasks give the final surface
  semantics now; the state-machine transform lands behind the same ABI.
