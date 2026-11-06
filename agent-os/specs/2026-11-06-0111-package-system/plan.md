# Package System + Stdlib — Implementation Plan

Go-shaped packages, versions, fetching, dead-code elimination, and a
Go-parity `std/`. Per-item design sketches live in `items/`; this file
is the order of battle. Milestone-gated, not date-gated.

**Status (2026-11-06): M1 (PR #5), M2 (PR #6), M3 (PR #7) implemented; V1–S5 spec'd.**
Review findings folded into the item files — see threads on the PR.

Execution model: Track M is the critical path — every other track
consumes the package model. M1 is the breaking change (bundle its
migration into one commit, like D5). Tracks G and S parallelize once
their dependencies land. Each task must pass `tests/run.sh` plus its
own acceptance criteria before its status flips.

## Task 1: Save spec documentation [done]

This folder: plan.md, shape.md, standards.md, references.md, items/.

## Track M — Module model (critical path, sequential)

### Task M1: `package` clause + directory packages [done — PR #5]

`items/m1-package-clause.md`. The breaking change: every `.oxl` gets
`package <name>`; import paths resolve to directories; the bound name
is the declared package name. Migration commit covers all 86 in-tree
files + the bootstrap relayout (4 mains can't share a dir under the
new rule). Everything else in this spec assumes it.

### Task M2: `oxl.mod` + module-aware resolution [done — PR #6]

`items/m2-module-manifest.md`. Manifest grammar (`module`/`oxl`/
`require`/`replace`), root discovery by walking up, resolution order
(std → main module → requires → error), `internal/` rule, import-cycle
diagnostics. v1 resolves `require`d modules via `replace` dirs or the
cache only — fetching lands in V2.

### Task M3: unused imports = compile error [done — PR #7]

`items/m3-unused-imports.md`. Per-file usage, Go wording
(`imported and not used: "path"`), selective names checked
individually, `as _` exempt. Small — do it right after M1 so the
migration commit also fixes any newly-illegal imports.

## Track V — Versions & fetching (sequential after M2)

### Task V1: semver + pseudo-versions + MVS [done — PR #11]

`items/v1-semver-mvs.md`. Version grammar incl. `/vN` suffix rule,
pseudo-version encode/parse/compare, MVS build-list walk. Pure
library code — the resolver's foundation.

### Task V2: module cache + `oxl get` + `oxl.sum` [in progress — this PR]

`items/v2-module-cache.md`. `OXLMODCACHE` layout, git-based fetch
(shells out to `git`, like early Go), `h1:`-style tree hashes,
verify-on-load. `@latest`/`@vX.Y.Z`/`@<ref>` queries.

### Task V3: `oxl mod` commands [not started]

`items/v3-mod-commands.md`. `init` / `tidy` / `verify` / `list -m`.
`tidy` is the companion to M3's error: it prunes unused requires and
adds missing ones.

## Track G — Dead-code elimination (parallel-safe after M1)

### Task G1: reachability in codegen [done — PR #9]

`items/g1-reachability.md`. Mark live decls from `main`/`test` roots
over the merged AST; drop dead fns/types/globals/vtabs from emission.
Both `codegen.rs` and `codegen.oxl` — identical output stays the
parity bar.

### Task G2: `rt.c` section GC + feature gates [done — PR #10]

`items/g2-runtime-sections.md`. `-ffunction-sections`/`--gc-sections`
(+ macOS/Windows spellings via the E3 veneer), and generalized
`-D` feature gating (sqlite/gui/net) driven by which `rt_*` externs
the program actually uses. Independent of G1.

## Track S — Stdlib (S1 after M1; S2-S5 parallel-safe)

### Task S1: `stdlib/` → `std/` package tree [done — PR #12]

`items/s1-std-restructure.md`. Directory-package relayout of the 8
existing modules onto Go paths where they map (`json`→`encoding/json`,
`http`→`net/http`), toolchain-side std root resolution, repo-wide call
site updates. Lands with the M1 migration commit or immediately behind
it — same diff family.

### Task S2: io core + byte story [not started]

`items/s2-std-io-core.md`. `std/io` (Reader/Writer/Closer), `std/errors`
(err carries a message — runtime touch), `std/strconv`, `std/bytes`,
`std/bufio`, bounded `std/fmt`. Pulls the `byte`/`[]byte` type it
needs — that's part of the item, not a side quest.

### Task S3: util & data packages [not started]

`items/s3-std-util.md`. `std/path`, `std/path/filepath`, `std/sort` +
`slices` + `maps` + `cmp` (generics), `std/math`, `std/math/rand`,
`std/unicode/utf8`, `std/flag`, `std/log`.

### Task S4: net & encodings [not started]

`items/s4-std-net-encoding.md`. `std/net` (sockets→Conn iface),
`std/net/http` upgrade on io, `std/encoding/{base64,hex,csv,binary}`,
`std/hash`, `std/crypto/{sha256,md5,hmac}` written fresh in oxl.
TLS stays a documented non-goal.

### Task S5: `std/regexp` [not started]

`items/s5-std-regexp.md`. Thompson NFA, linear-time, no backrefs —
RE2's guarantee on an oxl-scale engine. Largest single S item;
schedule last in the track.

### Parked (no spec — do not implement)

`items/parked-stdlib.md`. Go packages deliberately out of reach for
this milestone — reflect/`%v`-any formatting, `encoding/gob`,
`database/sql`, `image`, `compress`, `crypto/tls`, `context` (scopes
replace it), and the rest — each with the reason it doesn't map or
doesn't pay yet.

## Non-goals

- Central registry/proxy/mirror — fetch is direct-git only
- `oxl mod vendor`, `work` workspaces, `mod graph/why/edit` — noted
  deferrals, not gaps
- Decl-level visibility (`private` keyword) — see shape.md
- Package `init()` / program-order side effects
- Semantic import compatibility *enforcement* beyond the `/vN` rule
