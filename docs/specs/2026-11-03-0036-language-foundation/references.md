# References for Language Foundation

## Design doc (the spec this plan implements)

- **Location:** `docs/specs/2026-11-03-language-design.md`
- **Relevance:** the approved product of the brainstorming session — all
  decisions trace back to it.

## Prior art studied during design

- Scripting-class managed languages — surface model: DX, `[v, err]`
  errors, generics shape, shape
  stenciling, goroutines/channels (and its runtime-weight failure mode)
- Lean systems languages — runtime philosophy, error unions, comptime
  discipline,
  colorless stackless async precedent; preferred compiler implementation
  language
- **Interior-mutability types** — existence proof for runtime borrow-rule
  enforcement
- **Dynamic exclusivity enforcement** — shipped precedent for runtime
  aliasing/exclusivity traps
- **Generational references** — precedent for runtime dangling
  checks
- **Stackless async runtimes** — coroutine state machines and
  minimal single-thread executors

## Codebase references

None — greenfield repo. Tooling conventions come from the repo's own
setup (`.ox-agent/`, `docs/`).
