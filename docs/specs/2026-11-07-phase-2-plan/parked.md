# Parked work — reopen only on trigger

Nothing here is scheduled. Each item has a stated trigger; a trigger
firing moves the item into the milestone map via a new work item.

## LLVM backend

- **Why parked:** the C backend works, is portable, and keeps emitted
  code inspectable. The codegen seam (`docs/semantics/interfaces.md`)
  exists precisely so LLVM can slot in later.
- **Reopen trigger:** C codegen is a *measured* bottleneck (compile-
  time or runtime) or a platform target requires it (e.g. wasm via
  LLVM).

## Web compilation target (JS/HTML/CSS)

- **Why parked:** design doc defers it; shipping a JS target without a
  DOM-shaped UI layer produces a second-class target.
- **Reopen trigger:** milestone G lands and the widget tree is
  DOM-mappable as required.
- **Guardrail (active):** the GUI abstraction must not foreclose a DOM
  backend — reviewed at G's spec time.

## D4 shape-based generics

- **Why parked:** deferred with a real measurement in the v0.2 spec —
  3 generic fns in the entire corpus, zero shared-shape groups; type-
  dictionary passing is pure complexity at that scale.
- **Reopen trigger:** the corpus grows enough generic code that two
  or more instantiations share a shape and the mono cost is measured.

## Parked stdlib list (`docs/specs/2026-11-06-0111-package-system/items/parked-stdlib.md`)

- reflection-class packages (`fmt %v`, `gob`, `xml`, `database/sql`,
  templates): need runtime type descriptors
- `crypto/tls`, `x509`, asymmetric crypto: a TLS stack is a project,
  not a package
- `image`, `compress`, `archive`, `net/url`, `container/heap`:
  deferred until a real consumer exists
- `context`: deliberate non-goal — `scope` carries cancellation and
  deadlines
- **Reopen trigger:** the language machinery exists (type descriptors
  / reflection) or a concrete consumer demands the package.
