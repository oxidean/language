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
  spawned on (GUI/main-thread work). Parsed in v0, executor support later.

## Suspendability

A call is **suspend-capable** if its call graph transitively reaches
`spawn`, `scope`, `recv`, `sleep`, or any `extern` marked `blocking`.
The compiler computes the suspend set by reachability; only
suspend-capable functions get state-machine transform (target impl) or
tasked-stack treatment (interim impl). User writes blocking-looking code;
no `async`/`await` keywords exist.

## Interim implementation (documented deviation)

v0.0.1 ships tasks as **pool OS threads** (one stack per live task,
allocated from a fixed pool) rather than compiled state machines. The
surface semantics — scopes, join, cancel, ownership-transfer channels —
are final; the execution strategy is interim. Rationale: the
state-machine transform is the program's #1 technical risk and is
de-risked separately (see `prototype/`); blocking on it would stall every
downstream track. Transition to stackless frames is ABI-internal.

## Channels

```oxl
ch := mkchan()          // unbuffered, ownership-typed
send(ch, v)             // moves v into the channel; v dead after
[v, ok] := recv(ch)     // ok=false when closed+empty
close(ch)
```

- Unbuffered rendezvous in v0; `mkchan(n)` buffered form reserved.
- `send`/`recv` are the only suspend points v0 code can name explicitly.
- Sending `ref`/`mut` handles or arena references is a compile error
  (ownership.md §arena).

## Executors

- `Executor` is a library type, not mandatory runtime: a program that
  never spawns pays nothing (no threads, no scheduler structures).
- `exe := newPool(n)` — work-stealing pool over n OS threads.
- Default: first `scope` in a program lazily creates a shared pool sized
  to `GOMAXPROCS`-equivalent (CPU count).
- Blocking `extern` calls run on a separate blocking pool so they cannot
  starve task threads.
