# Tech Stack

## Frontend

N/A for v0.0.1 (CLI + services only). Post-v0.0.1 charter: platform-native
toolkit FFI bindings, a portable UI layer written in the language itself,
and a Qt/GTK FFI bridge. Web target (JS/HTML/CSS) parked.

## Backend

Compiler implementation: Rust (`oxl/`). Codegen: C emission compiled
through the system `cc` (`LANG_CC`/`LANG_LDFLAGS` knobs). The Rust
compiler remains the toolchain implementation — the `bootstrap/`
checker and C emitter track it in-language but are still partial
(no generics/interfaces/method lowering yet), so "self-hosted"
describes the goal, not the current state.
Runtime: C-compatible (`runtime/rt.c`), arena-based, no tracing GC.
LLVM remains a possible future backend once codegen size justifies it.

## Database

N/A for the language itself. The pilot CRUD app uses SQLite via C FFI
(libsqlite3).

## Other

- C FFI as the interop boundary, from day one
- LSP server inside the single toolchain binary
- Agent OS for planning, standards, and specs (`agent-os/`)
- Execution model: single driver + parallel agent worktrees
