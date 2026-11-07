# ADR-0006: Rust driver remains the bootstrap seed; fixpoint is C-text equality

Status: accepted (WI-0003) — 2026-11-07

## Decision
The `oxl/` crate remains in the tree as the documented **seed
toolchain**: it is the bootstrap compiler that builds the first
`.oxl`-written `oxl`. The product binary is `.oxl`-built. The dogfood
gate is `tests/run.sh` passing with the self-built driver. The
fixpoint proof is **C-text equality across successive generations
of the .oxl toolchain** — the seed-built driver emits `C_A` for the
driver sources; the driver built by that product emits `C_B`;
`C_A == C_B` because both run the same `.oxl` emitter. Seed-vs-self C
text is **not** compared: the Rust and `.oxl` emitters differ
cosmetically (e.g. `void oxl_main(void) { /*GLOBALS*/` inline vs
newline layout) and legitimately so — this is the same
successive-builds gate the compiler bootstrap already uses. Binary
hashes are not compared either: build-ids and embedded paths differ
legitimately. Dual-driver CI runs
only during the transition; afterwards the suite runs under the
self-built driver and the seed is exercised only as the bootstrap
input.

## Rationale
- Honest bootstrap semantics: every self-hosting toolchain keeps a
  seed (Go kept the C compiler, then the Go 1.4 tree). Deleting the
  Rust crate removes the bootstrap path entirely.
- Committing a pinned product binary was rejected — binary-in-repo is
  fragile, unreviewable, and breaks the new-contributor path.
- Perpetual dual-driver CI doubles suite time to parity-check the
  *seed*, which stops mattering once the product is `.oxl`-built.

## Consequences
- "Cargo out of the build path" (milestone exit criterion) means out
  of the *product* path — the seed remains a build input.
- CI needs a transition period where the suite runs under both
  drivers; the spec's exit criterion ends that period.
- `docs/` must describe the seed/product split so "the toolchain is
  written in oxl" stays literally true.
