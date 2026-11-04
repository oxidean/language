# B4 — Function values

## Problem

Recursive-descent parsing doesn't need them, but visitor-style AST
passes, `sort`/`map`/`filter` in stdlib, and callback APIs (the HTTP
`route()` indirection today) all do.

## Design sketch

- `fn(a: int) -> int` as a first-class type (fn pointer + optional
  captured env; closures below).
- Named-fn reference: `f := handler` where `handler` is a fn —
  zero-capture fn value (plain function pointer in C).
- Closures: `{ |x| body }` capture by value into an env struct —
  `fnv` = `{env*, fnptr}` fat pair in codegen (two words).
- Calling: `f(a)` — identical syntax to a named call; codegen emits
  indirect call through the fat pair.

## Acceptance

- `let f = add` then `f(1,2)`; pass fn as arg; store in struct field
- Closure captures a local by value: `n:=0; inc := {|| n+1 }; n=9;
  inc()` returns 1 — the copy is taken at closure creation (re-binding
  a captured name inside the closure is rejected; the Go-vs-Rust
  distinction collapses to copy — predictable, no escape analysis)
- `sort(xs, less_fn)` stdlib-shaped call works

## Depends on

Nothing. Independent of B1–B3.

## Risks

- Suspension through a fn value (calling a suspendable fn indirectly)
  — suspend-set analysis must treat indirect calls conservatively
  (mark "unknown callee" sites suspend-capable, or restrict fn values
  to non-suspending fns in v0.2 and document). **Decide at
  implementation**: restrict-first is honest and cheap.
- Copy-capture vs by-ref capture changes user-visible semantics —
  the spec picks copy; revisit only if real use demands refs.
