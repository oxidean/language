# Standards for Language Foundation

The following standards apply to this work.

---

## global/language-principles

Build-time conventions every implementation task must follow. Source of
truth: `docs/superpowers/specs/2026-11-03-language-design.md`.

- **No tracing GC.** Reclamation is hybrid: scope arenas by default
  (bulk-freed at scope close), owned heap objects for data that escapes
  or outlives its scope. Escaping data is explicit.
- **Memory safety is dynamic.** Ownership/borrow rules are enforced at
  runtime — both mutable-aliasing and dangling checks — on by default;
  `unsafe` only via explicit opt-out. Violations trap. One gap: an
  ordinary arena-backed `[]T` stored into an outer struct can still
  dangle past scope close with no general runtime check (the B3 spec
  scopes this out) — `own []T` or heap backing is the escape hatch.
- **Traps are scope-contained.** A trap kills the task, cancels siblings,
  unwinds the scope, propagates to parent. Process abort only outside
  any scope or for corruption a scope cannot unwind.
- **Go-style errors.** `[value, err]` multiple returns;
  `if err != nil` idiom; silently dropping an error is a compile error.
- **Shallow type system.** Static, explicit at boundaries, inferred
  inside bodies. Go-style generics with constraint interfaces. No
  macros, no metaprogramming; comptime evaluation for constants only.
- **Structured concurrency.** Tasks spawn inside `scope` blocks;
  channels transfer ownership on send; cross-thread shared state only
  via `Shared<T>`.
- **One toolchain binary.** build, run, test, fmt, doc, LSP — no separate
  tools.
- **AOT native + C FFI.** Everything compiles ahead-of-time to native;
  C FFI is the only interop boundary.
