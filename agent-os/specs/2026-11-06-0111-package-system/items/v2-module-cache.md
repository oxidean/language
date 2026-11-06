# V2 — module cache, `oxl get`, `oxl.sum`

## Problem

`require`d modules need somewhere concrete to live and a way to get
there: a cache, a fetcher, and integrity pinning.

## Design sketch

- Cache: `$OXLMODCACHE`, default `~/.oxl/mod`
  (`%LOCALAPPDATA%\oxl\mod` on Windows — reuse the E3 path veneer).
  Layout `<cache>/<escaped-path>@<version>/` — path elements
  `!`-escaped on case-folding chars like Go's `module.EscapePath`.
  Cache dirs read-only (`a-w`) — corruption-by-edit is a real Go
  footgun worth pre-empting.
- `oxl get <path>@<query>`:
  - `@latest` → highest semver tag in the repo
  - `@vX.Y.Z` / `@vX.Y.Z-rc` → that tag
  - `@<sha>`/`@<branch>` → resolve via git → pseudo-version
  - Fetches by shelling out to `git` (clone/fetch into a temp dir,
    `git archive <resolved-sha>` extracted into the cache — canonical
    tree content, never a checkout, so `core.autocrlf` / smudge
    filters can't fork the bytes; `export-ignore` is honoured, as in
    Go module zips) — same model as the Go module proxy; `git` binary
    is a documented toolchain dependency for `get` only.
  - Adds/updates the `require` line in `oxl.mod` and refreshes
    `oxl.sum`.
- `oxl.sum`: one line per module: `<path> <ver> h1:<b64-sha256>` —
  hash of the deterministic dir tree (sorted relpaths + contents,
  same idea as Go's dirhash) plus `<path> <ver>/mod h1:<...>` for the
  manifest itself. One procedure serves both sides: it runs over the
  extracted archive at `get` time and over the cached dir at load
  time — identical inputs, since the cache is the archive extracted
  byte-for-byte. Verified on every load: cached dir whose hash
  mismatches → `checksum mismatch` (Go wording).
- Private repos work through git's own auth (ssh agent, credential
  helper) — nothing oxl-specific needed; document that.
- `replace` dirs bypass cache + sum entirely (local code is already
  trusted).

## Acceptance

- Offline fixture: `get` from a `file://` git repo with tags —
  `oxl get x@v1.0.0` populates cache + writes require + sum; a second
  `get` at `@latest` after a new tag lands bumps both
- Tampered cache file → `checksum mismatch` on next build
- Build with all requires satisfied from cache works with the network
  "off" (fixture is file:// anyway)
- Cache dir is read-only on disk

## Depends on

V1 (queries need semver; MVS runs over the fetched manifests).

## Risks

- `git` availability/behaviour varies (Windows!) — keep all git calls
  behind one small veneer; error messages quote the failing command.
- Hash stability across platforms: the hash input is canonical git
  tree content — the `git archive` extraction path means checkout
  filters never touch it, so `core.autocrlf` / smudge filters can't
  fork `oxl.sum` between machines. Normalize `\\`→`/` in recorded
  names; document the canonicalization.
