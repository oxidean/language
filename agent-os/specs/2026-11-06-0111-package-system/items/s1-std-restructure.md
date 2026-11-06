# S1 — `stdlib/` → `std/` package tree

## Problem

`stdlib/` is 8 flat files. Under directory packages it must become a
tree — and this is the moment to lay it out on Go's paths so the
growth in S2-S5 lands in the right places.

## Design sketch

- `stdlib/` → `std/` at repo/toolchain root; each module a dir with
  `package <name>` files.
- Path mapping (Go paths where they map):
  `strings` → `std/strings`, `os` → `std/os`, `sync` → `std/sync`,
  `testing` → `std/testing`, `time` → `std/time`,
  `json` → `std/encoding/json`, `http` → `std/net/http`,
  `collections` → `std/collections` (kept — Go's analog is spread
  across `container`+`maps`; our `Map`/`Set` types don't split
  cleanly. Documented divergence in parked-stdlib notes).
- Import strings change: `"std/json"` → `"std/encoding/json"` —
  mechanical rename; call sites' qualified names (`json.jparse`)
  stay valid since package names don't change.
- Std root resolution: `std/...` imports resolve under the toolchain
  `std/` dir found via exe ancestors (today's last-resort becomes
  the only mechanism — the `stdlib/`-ancestor scan dies with it).
- `std` needs no `require` — its version is the toolchain version;
  `oxl 0.3` in `oxl.mod` is the compatibility floor.
- Repo call sites + diag corpus import strings updated in the same
  commit.

## Acceptance

- `tests/run.sh` green on the new tree; every `import "std/…"` in
  examples/tests/bootstrap resolves via the new root only
- `std/encoding/json` and `std/net/http` imported by path and called
  by their unchanged package names
- A `std/` tree outside the repo (installed toolchain location)
  resolves — the exe-ancestor walk is the sole mechanism

## Depends on

M1 — can't be a tree before directory packages exist. Lands with or
right behind the M1 migration commit.

## Risks

- Import-string renames touch every stdlib consumer — same D5-style
  mechanical sweep, same mitigation (one commit, suite gates it).
