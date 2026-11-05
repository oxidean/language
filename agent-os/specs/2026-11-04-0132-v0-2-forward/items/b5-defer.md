# B5 — `defer`

## Problem

Compiler passes allocate per-node; cleanup-on-error is error-prone
today (`[v,err]` returns that skip frees). `defer` is the lean answer
the design's Go-shape implies.

## Design sketch

- `defer expr` — runs when the enclosing fn returns (any exit: value,
  error return, or scope-trap unwind).
- LIFO order (Zig/Go): each defer's call arguments are evaluated when
  the defer registers; the captured calls run in reverse registration
  order when the fn exits — a loop deferring `f(i)` prints 2, 1, 0.
- Lowering: **runtime registration**, not statement reordering — each
  executed `defer` pushes a thunk (call + value-captured env) onto the
  running task's per-task defer stack; fns drain down to their entry
  mark at every exit. Compile-time collection can't express
  conditional/looping registration, so the stack lives in the runtime.
- In a trap/cancel unwind: the task driver drains the defer stack
  before settling the scope — registered defers run on every exit:
  value return, error return, trap, cancel.

## Acceptance

- `defer close(db)` fires on normal return, error return, and trap
- LIFO: three defers print 3-2-1
- `defer` inside a loop = per-iteration defer at fn scope (Go
  semantics — document, don't "fix")
- `defer` inside an untaken branch registers nothing — only executed
  `defer` statements run

## Depends on

Nothing — small, self-contained codegen transform.

## Risks

- Trap-path defer execution must not double-free arena'd values —
  arena bulk-free already handles memory; defers are for *external*
  resources (fds, conn handles). Document: defer ≠ memory management.
