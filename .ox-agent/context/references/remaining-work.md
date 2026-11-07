# Remaining work inventory (as of WI-0002)

> **Pre-planning snapshot.** Captured at WI-0002 discovery
> (2026-11-07), before the milestone map landed. Names below are the
> *old* names — "Full self-hosting" is now **toolchain-in-oxl**
> (milestone T), and the five small-debt items are filed as issues
> #26–#30 (`polish` label). Authoritative current state:
> `docs/specs/2026-11-07-phase-2-plan/plan.md`.

Everything documented-but-unbuilt, grouped by how hard it is to schedule.

## Phase 2 roadmap items (`docs/product/roadmap.md`)

- **Toolchain-in-language** (currently titled "Full self-hosting") —
  the four compiler components (lexlib, parser, check, codegen)
  self-compile byte-identically, but the *driver* is still Rust:
  `oxl` CLI, `oxl.mod` resolution + MVS, git fetch/cache, fmt, doc,
  LSP, test runner (~`oxl/src/`). Name is misleading: the compiler
  already compiles itself; what remains is porting the driver.
- **GUI portable layer** — only the GTK4 FFI pilot
  (`examples/gui-todo`) exists; widget layer unbuilt. Guardrail:
  keep the widget tree DOM-mappable (`items/parked-web.md`).
- **Package registry / `oxl publish`** — module system shipped
  (git-fetch + semver/MVS + oxl.sum) but no central registry.
- **LLVM backend** — parked behind `interfaces.md` codegen seam.
- **Web compilation (JS/HTML/CSS)** — parked by design doc.

## Spec-sanctioned deferrals (reopen only on trigger)

- **D4 shape-based generics** — deferred w/ measurement (3 generic
  fns, zero shared-shape groups); reopens when corpus grows.
- **`parked-stdlib.md`** — reflect/gob/xml/database-sql/templates
  (need runtime type descriptors), crypto/tls/x509, image/compress/
  archive, net/url, container/heap, context (non-goal: `scope` covers).

## Small code-level debt (documented in comments, untracked)

- `std/encoding/json` — \uXXXX surrogate-pair edge cases ("v1 work")
- `std/errors` — `Is`/`As` (needs error trees + typed targets)
- `std/fmt` — `%v` (needs reflection)
- `std/net` — DNS/getaddrinfo (dotted-IPv4 only today)
- `std/time` — blocking-worker API is deliberate-stopgap

## Housekeeping

- WI-0001 archived 2026-11-07 (shipped via PR #25).
- Auto-release live: every merge to main cuts a semver tag+release.
