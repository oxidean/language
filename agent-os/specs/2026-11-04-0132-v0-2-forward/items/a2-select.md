# A2 — `select` multi-channel wait [settles design-doc TBD]

## Problem

Services routinely wait on N channels (fan-in, timeout+cancel). Design
doc marks `select` "desired, design TBD" — this spec settles it so the
surface is decided before the runtime work (A1) makes re-design costly.

## Design sketch (proposed — needs approval before build)

```
select {
    recv(ch_a) => handle_a(v)          // arm: channel op + body
    recv(ch_b) => handle_b(v)
    timeout(50) => on_timeout()        // optional
    else => fallback()                 // optional: non-blocking
}
```

Semantics (Go-shaped, deliberately):

- Arms are `recv` channel ops in v0.2 (send arms are future work — see
  the exclusion note below); exactly one fires; uniform random pick when
  several are ready (no starvation by ordering).
- No `else` and no ready arm → suspend until an arm is ready (the
  suspend point A1's transform already knows how to compile).
- `timeout(ms)` arm = recv on a runtime-created timer channel — keeps
  `select` to "pick among channel ops" with no special timer syntax.
- Inside a `scope`, sibling-cancel propagates: a cancelled scope
  unblocks `select` with the scope trap (same as `recv` today).

Not in v0.2: `send` arms inside `select` on unbuffered channels
(pick a winner or wait — same semantics as Go; just flag complexity in
the implementation note if the runtime can't express it yet).

## Acceptance

- `select` over 2 recv chans: fires each arm; random distribution
  roughly uniform over 10k iterations
- `timeout` arm fires at ≥ms, `else` arm is non-blocking
- Suspended `select` inside a cancelled scope unwinds correctly
- Compile error for non-channel arm expressions

## Depends on

- A1 (the transform) — `select`'s suspend is a multi-way channel
  suspend; landing it on the pool executor would mean writing the
  waiting machinery twice (once threaded, once framed).

## Risks

- Fairness/priority questions (Go chose uniform-random; mirroring is
  the safe call).
- `send` arms on unbuffered channels — may need a two-phase ready
  handshake in `RtChan`; cut from v0.2 if it churns the channel
  internals beyond A1's reshape.
