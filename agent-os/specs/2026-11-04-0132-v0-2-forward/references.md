# References for v0.2 Forward Work

## Design doc (product decisions)

- `docs/superpowers/specs/2026-11-03-language-design.md` — all surface
  decisions trace back here; items only add implementation detail.

## Semantics annex (contracts items must keep)

- `docs/semantics/concurrency.md` — scope/pending/cancel/channel
  semantics (the transform keeps this contract, changes mechanics)
- `docs/semantics/ownership.md` — borrow/poison/trap taxonomy
- `docs/semantics/interfaces.md` — stage contracts; the seams interim
  implementations hide behind
- `docs/self-hosting.md` — the gap analysis Track B/C implements

## Measurements

- `docs/prototype/report.md` — SM frames ~96 B vs ~8.5 kB/task (88×);
  ownership checks at noise. The empirical case for Task A1.

## Prior art per item

- Stackless async runtimes — state-machine transform (A1)
- Multi-channel `select` prior art — wait surface semantics (A2)
- Tagged-union prior art — for B1 shape
- Structural-interface prior art — D3 (shallow, no trait solving)
- Shape-stenciling prior art — D4
- Scoped `defer` prior art — B5
- AST pretty-printer prior art — E2 direction
