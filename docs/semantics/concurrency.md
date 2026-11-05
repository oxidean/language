# Concurrency Semantics (v0)

## Surface

```oxl
scope {
    spawn worker(a, b)      // task in this scope
    go pump(ch)             // shorthand: fire within scope
    // ...
}                           // waits: all tasks joined or cancelled
```

- `scope { ... }` — creates a child scope bound to the lexical block.
  Closing the block waits for every task spawned inside. If any task
  traps, remaining tasks are cancelled and `ErrTrap` propagates.
- `spawn f(args)` — enqueue task in the innermost enclosing scope.
- `go f(args)` — identical to `spawn`; both spellings accepted (Go
  familiarity). v0 implements both as the same AST node.
- `spawnPinned f(args)` — reserved: task bound to the OS thread it was
  spawned on (GUI/main-thread work). Not yet parsed in v0 (the call
  resolves as an ordinary undefined ident); executor support later.

## Suspendability

A call is **suspend-capable** if its call graph transitively reaches
`scope`, `send`, `recv`, `sleep`, `select`, or a `spawn` whose call
arguments are suspend-capable.
The compiler computes the suspend set by reachability; only
suspend-capable functions get the state-machine transform. User writes
blocking-looking code; no `async`/`await` keywords exist.

## Frame-task implementation

Suspend-capable functions compile to **stackless state machines**:

- Each such function gets a generated frame struct (`o_frm_<fn>`):
  a header (`cur_scope`, resume `state`, `child` link), parameter
  slots, one slot per live local, embedded `RtScope`s for `scope {}`
  blocks, and a `result` slot when the function returns a value.
- The body becomes a `poll` driver (`o_poll_<fn>`). A `switch` on
  `fr->state` jumps to the resume label; at a suspend point the driver
  stores the next state and returns `RT_PENDING`. `RT_READY` means the
  function returned; `RT_CANCEL` means it observed scope cancel.
- Suspend calls in expressions are hoisted to statement level so every
  suspend point is a plain statement boundary. Short-circuit `&&`/`||`
  operands are never eagerly evaluated.
- A suspend-point *call* allocates a child frame, links it via
  `fr->child`, and drives `child`'s poll inline — the parent's task
  record is shared, so nested suspends cost one small `malloc` each and
  are freed when the child completes.
- `spawn f(args)` allocates one block (task header + frame) via
  `rt_task_new` and enqueues it on the work pool's shared queue.
  Parked tasks hold *no* OS thread — ~10k idle tasks fit in ~2 MB RSS.
  Exception: `sleep` and `select` `timeout` waits each hold one
  detached timer pthread for the wait's duration, so a program with
  many concurrent timer waits still holds that many OS threads.
- `spawnBlocking f(args)` sends the frame task to the blocking pool
  (one thread per task) so blocking FFI can never starve workers.

Tasks trap via `longjmp` to the driver thread's unwind slot; the trap
marks the scope chain trapped/cancelled and the driver settles the
pending count — identical containment to the thread model.

## Channels

```oxl
ch := mkchan()          // unbuffered, ownership-typed
send(ch, v)             // moves v into the channel; v dead after
v := recv(ch)           // blocks; returns the sent word
close(ch)
```

- `mkchan()` unbuffered rendezvous; `mkchan(n)` buffered ring of `n`
  words — `send` parks only when full, `recv` only when empty, close
  wakes all waiters (recv then drains the ring, same as before).
- `send`/`recv` are the only suspend points v0 code can name explicitly.
- Sending `ref`/`mut` handles or arena references is a compile error
  (ownership.md §arena).

## Select

```oxl
select {
    v := recv(a) => { println(v) }   // bind optional
    recv(b)      => { ... }          // fire-and-discard
    timeout(ms)  => { ... }          // at most one
}

select {
    recv(c)      => { ... }
    else         => { ... }          // non-blocking default
}
```

- recv arms only; send arms are rejected (cut for v0.2).
- Multiple ready arms: the poll picks uniformly at random over the
  ready set — a random-start first-ready sweep is NOT uniform (an arm
  right after the start point wins over equally-ready others).
  `else` fires when no arm is ready. `timeout` fires once after `ms`
  and never with `else`.
- A parked `select` holds one waiter node per arm chan; the first
  delivery wins via an atomic claim — losers unlink on resume.
- Scope cancellation force-wakes parked `select`/`recv`/`send` waiters;
  they die at their resume point rather than hanging the join.

## Executors

- `Executor` is a library type, not mandatory runtime: a program that
  never spawns pays nothing (no threads, no scheduler structures).
- `exe := newPool(n)` — planned library surface. v0.2 ships the
  built-in executor only.
- Default: the first `spawn` lazily creates a shared work pool sized
  to `GOMAXPROCS`-equivalent (CPU count, capped at 32). Workers pull
  tasks off one MPMC queue and drive each task's `poll` until it
  suspends or completes — cooperative, not preemptive.
- `spawnBlocking` tasks run on a separate blocking pool so they cannot
  starve task threads. Plain `extern` calls are NOT re-routed — they
  run inline on the caller. To offload a blocking extern, wrap it in an
  OXL function and pass the wrapper to `spawnBlocking`:

  ```oxl
  extern fn foreign_fn(fd: int) -> int

  fn blocking_call(fd: int) -> int {
      return foreign_fn(fd)
  }

  spawnBlocking blocking_call(fd)
  ```
- Scale: parking does not hold a thread — a channel-parked task costs
  only its suspended frame; a parked frame sender additionally holds one
  `SendReq` on the channel's send queue, and a parked `select` holds one
  `RtSel` sized by arm count. Timer waits are the exception: each holds
  a detached thread. On the channel-rendezvous benchmarks 10k parked
  receivers measured ~1.9 MB RSS and ~2x faster than the thread-pool
  executor at 1k+ tasks (`tests/conc_scale.oxl`, `tests/conc_ping.oxl`);
  those numbers do not cover select parking.
