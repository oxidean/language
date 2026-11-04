# B5 — `defer`

## Problem

Compiler passes allocate per-node; cleanup-on-error is error-prone
today (`[v,err]` returns that skip frees). `defer` is the lean answer
the design's Go-shape implies.

## Design sketch

- `defer expr` — runs when the enclosing fn returns (any exit: value,
  error return, or scope-trap unwind).
- LIFO order (Zig/Go), evaluated at return-site in reverse.
- Lowering: codegen wraps each return in the fn's defer stack —
  mechanical statement reordering; no new runtime machinery.
- In a trap unwind: defers run during scope teardown (trap containment
  already unwinds — defer hooks into that path).

## Acceptance

- `defer close(db)` fires on normal return, error return, and trap
- LIFO: three defers print 3-2-1
- `defer` inside a loop = per-iteration defer at fn scope (Go
  semantics — document, don't "fix")

## Depends on

Nothing — small, self-contained codegen transform.

## Risks

- Trap-path defer execution must not double-free arena'd values —
  arena bulk-free already handles memory; defers are for *external*
  resources (fds, conn handles). Document: defer ≠ memory management.
