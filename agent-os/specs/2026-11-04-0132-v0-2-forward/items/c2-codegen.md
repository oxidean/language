# C2 — `bootstrap/codegen.oxl` (self-hosted C codegen)

## Problem

The last piece of the bootstrap chain: emit C from the typed AST so a
self-compiled `oxl` compiles itself. Until this lands, self-hosting
is a demo; after it, it's the actual compiler.

## Design sketch

Port `oxl/src/codegen.rs` (the C-emit backend — the LLVM target stays
a separate future seam, not part of bootstrap):

- `Expr`/`Stmt`/`Item` → C text via a `std/buffer`-style string builder
- monomorphized generic instantiations (`l_f_name__T` naming scheme —
  same as the Rust backend so `runtime/` links identically)
- spawn trampolines, `[v,err]` ret structs, borrow-check emission
  (must emit `rt_unborrow` — the exact discipline fixed in v0.0.1)
- imports already merged by `parser.oxl`'s crate walk

Output contract: the emitted C must compile against `runtime/rt.{c,h}`
byte-for-byte-compatible at the ABI level (same struct layouts, same
`rt_*` calls) — the runtime doesn't know which compiler emitted it.

## Acceptance

- `bootstrap/codegen.oxl` emits C for all `examples/` — `cc` builds
  them, they run identically
- **The bootstrap chain**: `oxl` (Rust) compiles
  `lexlib+parser+check+codegen.oxl` → that binary compiles
  `hello.oxl` → hello runs. This is the demo.
- Self-compile: the self-hosted compiler compiles *itself* (all four
  bootstrap .oxl files) — bit-identical output not required,
  behavior-identical is

## Status (2026-11-05): done

Verified in `tests/run.sh`'s `== bootstrap codegen ==` section:

- hello / features / cli / smoke-test plus `tests/match.oxl` and
  `tests/chanbuf.oxl` emit through `codegen.oxl`, compile with `cc`,
  and run identically to `oxl run`. The gui pilot emits and links when
  gtk4 dev files are present.
- All four bootstrap components self-compile and behave identically
  (lexer and parser output compared in full against `oxl run`,
  checker diagnostics on one corpus file, codegen self-compile
  byte-identical).

Interface values closed out (2026-11-05): the `RtIface`/vtable
machinery is ported — type mapping, concrete→iface boxing, per-impl
vtables, `x.m()` dispatch, receiver adaptation, pair-return iface
methods, generic-method vtab entries, and monomorphization for generic
fns. `tests/methods.oxl` and the http pilot (`Handler` param) are back
in `tests/run.sh` codegen coverage, emitting + running identically to
`oxl run`.

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
