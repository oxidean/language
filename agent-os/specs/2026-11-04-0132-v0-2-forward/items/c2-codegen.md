# C2 — `bootstrap/codegen.lang` (self-hosted C codegen)

## Problem

The last piece of the bootstrap chain: emit C from the typed AST so a
self-compiled `lang` compiles itself. Until this lands, self-hosting
is a demo; after it, it's the actual compiler.

## Design sketch

Port `lang/src/codegen.rs` (the C-emit backend — the LLVM target stays
a separate future seam, not part of bootstrap):

- `Expr`/`Stmt`/`Item` → C text via a `std/buffer`-style string builder
- monomorphized generic instantiations (`l_f_name__T` naming scheme —
  same as the Rust backend so `runtime/` links identically)
- spawn trampolines, `[v,err]` ret structs, borrow-check emission
  (must emit `rt_unborrow` — the exact discipline fixed in v0.0.1)
- imports already merged by `parser.lang`'s crate walk

Output contract: the emitted C must compile against `runtime/rt.{c,h}`
byte-for-byte-compatible at the ABI level (same struct layouts, same
`rt_*` calls) — the runtime doesn't know which compiler emitted it.

## Acceptance

- `bootstrap/codegen.lang` emits C for all `examples/` — `cc` builds
  them, they run identically
- **The bootstrap chain**: `lang` (Rust) compiles
  `lexlib+parser+check+codegen.lang` → that binary compiles
  `hello.lang` → hello runs. This is the demo.
- Self-compile: the self-hosted compiler compiles *itself* (all four
  bootstrap .lang files) — bit-identical output not required,
  behavior-identical is

## Depends on

- C1 (checker port — codegen consumes its typed AST + suspend set)
- B3 (slices) — codegen's output buffers want `[]T`/`[]byte` not
  linked lists
- A1 only loosely: if the transform lands first, codegen emits frames
  for suspendable fns; if not, it emits the interim pool-thread calls
  and migrates with the rest. Either order works — the runtime ABI is
  the contract.

## Risks

- The two biggest files in the language (~1200 lines combined) — the
  real DX/perf test for the stdlib; expect to land `std/buffer` and
  stdlib hardening as side-effects.
- ABI drift vs the Rust backend — the runtime contract pins it; the
  differential test (same input → same C behavior) catches drift.
