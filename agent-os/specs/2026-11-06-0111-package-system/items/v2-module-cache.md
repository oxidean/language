# V2 — module cache, `oxl get`, `oxl.sum`

## Problem

`require`d modules need somewhere concrete to live and a way to get
there: a cache, a fetcher, and integrity pinning.

## Design sketch

- Cache: `$OXLMODCACHE`, default `~/.oxl/mod`
  (`%LOCALAPPDATA%\oxl\mod` on Windows — reuse the E3 path veneer).
  Layout `<cache>/<escaped-path>@<version>/` — path elements
  `!`-escaped on case-folding chars (the established escape scheme).
  Cache dirs read-only (`a-w`) — corruption-by-edit is a real
  footgun worth pre-empting.
- `oxl get <path>@<query>`:
  - `@latest` → highest semver tag in the repo
  - `@vX.Y.Z` / `@vX.Y.Z-rc` → that tag
  - `@<sha>`/`@<branch>` → resolve via git → pseudo-version
  - Fetches by shelling out to `git` (clone/fetch into a temp dir,
    `git archive <resolved-sha>` extracted into the cache — canonical
    tree content, never a checkout, so `core.autocrlf` / smudge
    filters can't fork the bytes; `export-ignore` is honoured, as in
    upstream module archives) — the module-proxy model applied to git;
    `git` binary
    is a documented toolchain dependency for `get` only.
  - Adds/updates the `require` line in `oxl.mod` and refreshes
    `oxl.sum`.
- `oxl.sum`: one line per module: `<path> <ver> h1:<b64-sha256>` —
  hash of the deterministic dir tree (sorted relpaths + contents,
  dirhash) plus `<path> <ver>/mod h1:<...>` for the
  manifest itself. One procedure serves both sides: it runs over the
  extracted archive at `get` time and over the cached dir at load
  time — identical inputs, since the cache is the archive extracted
  byte-for-byte. Verified on every load: cached dir whose hash
  mismatches → `checksum mismatch`.
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
  fork `oxl.sum` between machines. Recorded names join path
  *components* with `/` (never a string replace — a POSIX filename
  may legally contain `\`), and a component with a literal
  backslash is rejected outright: it can't survive a Windows
  extraction anyway. The same rule covers case-folding collisions —
  an archive containing both `Icon.png` and `icon.png` is rejected
  before extraction (a tree one filesystem can hold and another
  can't is unportable by definition; the model enforces the identical
  restriction in module zips). Every component is also validated on
  all platforms against Windows-reserved device names — `CON`,
  `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, matched
  case-insensitively and with any extension stripped (`aux.oxl` is
  `AUX` to Windows) — and against characters Windows forbids in
  filenames (`<`, `>`, `:`, `"`, `|`, `?`, `*`, and trailing
  dots/spaces). A Linux checkout may hold `aux.oxl`, but a Windows
  user could never fetch the same module — same unportable-tree
  rule, same all-platforms check.
