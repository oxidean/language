# V2 Implementation Plan — module cache, `oxl get`, `oxl.sum`

> Implements `items/v2-module-cache.md`. Executed natively in-session.

**Goal:** required modules live in a content-addressed cache (`$OXLMODCACHE`,
default `~/.oxl/mod`), fetched by `oxl get <path>@<query>` through a small git
veneer, pinned by `oxl.sum` `h1:` hashes verified on every load. `replace` dirs
bypass both.

**Already on main (landed with V1):** `oxl/src/modcache.rs` — escape_path,
cache_dir, slot, sha256/b64/hex, dir_h1, mod_h1, parse_sums, sums_text,
verify_slot, check_component, untar, seal, remove_tree, slurp + unit tests;
`oxl/src/fetch.rs` — git veneer (bare clone → rev-parse → archive → manifest),
repo_url, tag/semver/branch/sha → pseudo-version resolution. `ModCtx` resolves
requires to `<cache>/<path>@<ver>` via `module_dir` and MVS `selected`.

**Remaining:** wire it together + parity.

## Steps

### V2.1 — seed: load-time verification (main.rs)

- `ModCtx` gains `sums: Vec<SumLine>`; `mod_ctx` parses `oxl.sum` beside
  `oxl.mod` (absent = empty; malformed = hard manifest error).
- `module_dir` → `Result<Option<PathBuf>, String>`: replace ⇒ `Ok(Some)`
  (never verified); cache slot exists ⇒ `verify_slot` (`Err` propagates —
  the `missing oxl.sum entry` / `checksum mismatch` texts);
  absent ⇒ `Ok(None)`. Slot name via `modcache::slot()` (fixes raw
  `{mp}@{ver}` — uppercase paths would never resolve).
- `build_list` treats `Err` like `None` (untrusted dir can't feed MVS).
- `resolve_pkg` gains `Resolve::Tampered(String)`; import loop emits
  `error [check] <msg>` from it.
- Dedupe: `main.rs::modcache_dir` delegates to `modcache::cache_dir`.

### V2.2 — seed: `oxl get <path>@<query>` (main.rs)

- Requires a module root from cwd (`error: no oxl.mod in current directory
  or any parent`); bare `@`/absent query ⇒ `latest`.
- fetch::fetch → slot exists ⇒ reuse (still re-upsert sums — idempotent);
  else untar to a sibling tmp dir → seal → rename into place
  (remove_tree on failure).
- oxl.mod upsert: `require <path>` line exists ⇒ version replaced in
  place (single-line or inside a `require (` block); else a
  single-line `require <path> <ver>` is appended at the end.
- oxl.sum upsert: drop stale lines for that path, add tree + `/mod`
  lines, write `sums_text` output.
- Report: `added` / `upgraded` / `unchanged` `{path}@{ver}`.

### V2.3 — check.oxl parity

Port, same diagnostics: escape_path, modcache_dir (already), slot
resolution in its module_dir equivalent, parse_sums/sum_for,
sha256+b64+hex (no shift ops — rotates via `*2^`/`/2^` + `& 0xffffffff`;
ints are wrapping i64 so all u32 math masks), dir_h1 (rt_lsdir+readfile),
mod_h1, verify_slot → identical `error [check]` text. `oxl get` itself
stays driver-only — fetching is a toolchain command, not a language
feature.

### V2.4 — corpus + docs

- run.sh section (POSIX-gated — `file://` paths can't survive Windows
  slot names; documented): build a git repo fixture with two tags;
  `oxl get x@v1.0.0` populates cache+require+sum; new tag → `@latest`
  bumps both; tampered cached file → `checksum mismatch`; slot dirs
  read-only; build resolves purely from cache.
- tests/mod fixture for verify-on-load may reuse a generated fixture.
- README/self-hosting note; item file marked implemented.

## Verify

`cargo build --release` warning-clean; fixture flow end-to-end on Linux;
selfcheck on module fixture produces byte-identical diagnostics incl.
tamper case; full `tests/run.sh`.
