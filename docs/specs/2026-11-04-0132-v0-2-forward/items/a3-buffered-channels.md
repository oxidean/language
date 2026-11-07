# A3 — Buffered channels

## Problem

`mkchan()` creates unbuffered channels only — sender blocks until a
receiver arrives. Producer/consumer patterns (log pipelines, work
queues, the HTTP accept loop's own handoff) want bounded buffering;
`std/sync` had to grow spin-counters partly because a bounded queue
didn't exist.

## Design sketch

- `mkchan(n)` → channel with capacity `n`; `mkchan()` → `n=0`
  (unbuffered, current behavior — no surface change).
- `RtChan` gains a fixed ring buffer of `int64` words (word payload
  stays — pointer payloads via `own` boxing, same as today).
- `send` blocks only when full; `recv` blocks only when empty;
  `close` wakes all waiters with closed-state (existing semantics
  extended to the buffered path).
- Broadcast-wakeup discipline kept (the lost-wakeup race fix).

## Acceptance

- `mkchan(3)` buffers 3 words: 3 non-blocking sends, 4th blocks
- `recv` drains in FIFO order
- `close` on a non-empty buffered chan: recv drains buffer, then gets
  closed state
- Existing unbuffered tests unchanged

## Depends on

- Soft dependency on A1: the same `RtChan` internals get re-shaped by
  the frame-based executor; buffer the ring on top of that shape, not
  the current one (avoid double-churn).

## Risks

- Low — textbook ring buffer. The only real choice is payload width
  (word vs `own`); keep word for consistency.
