# References for Language Foundation

## Design doc (the spec this plan implements)

- **Location:** `docs/superpowers/specs/2026-11-03-language-design.md`
- **Relevance:** the approved product of the brainstorming session — all
  decisions trace back to it.

## Prior art studied during design

- **Go** — surface model: DX, `[v, err]` errors, generics shape, gcshape
  stenciling, goroutines/channels (and its runtime-weight failure mode)
- **Zig** — lean runtime philosophy, error unions, comptime discipline,
  colorless stackless async precedent; preferred compiler implementation
  language
- **Rust `RefCell<T>`** — existence proof for runtime borrow-rule
  enforcement
- **Swift dynamic exclusivity** — shipped precedent for runtime
  aliasing/exclusivity traps
- **Vale generational references** — precedent for runtime dangling
  checks
- **Rust async / embassy** — stackless coroutine state machines and
  minimal single-thread executors

## Codebase references

None — greenfield repo. Tooling conventions come from the repo's own
setup (`.agents/skills/`, `agent-os/`).
