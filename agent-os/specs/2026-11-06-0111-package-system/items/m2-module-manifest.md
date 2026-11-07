# M2 — `oxl.mod` + module-aware resolution

## Problem

There is no manifest: no way to name a project, pin a dependency, or
say "these files belong to one module". `resolve_import` walks
ancestors looking for `stdlib/` — fine for std, meaningless for
anything else.

## Design sketch

`oxl.mod` at the module root — the established manifest grammar,
renamed:

```
module github.com/oxidean/app

oxl 0.3

require github.com/oxidean/widget v1.4.2

replace github.com/oxidean/widget => ../widget
```

- Directives: `module <path>`, `oxl <ver>` (language floor — checked
  against toolchain version), `require <path> <ver>` (single line or
  `require ( ... )` block), `replace <path> [v] => <path-or-dir>`
  (v1: local-dir RHS only — covers local dev; module→module replace
  noted deferral).
- Root discovery: walk up from the entry file/cwd to the first
  `oxl.mod`. None found → module mode off: only `oxl run`/`fmt`/file
  args work, and only `std` imports resolve (module mode off —
  behavior).
- This repo migrates with the item: root `oxl.mod` declares
  `module oxl`, and M1's bare-path imports (`bootstrap/check`,
  `tests/d5a`) rewrite to `oxl/bootstrap/check`, `oxl/tests/d5a` —
  there are no module-relative bare imports, so rule 2 below covers
  every in-repo path once the prefix lands.
- Resolution order for import path `P` in file `F`:
  1. `std/...` → toolchain std root (shipped tree found via exe
     ancestors — today's last-resort mechanism becomes the only one;
     no more ancestor-scanning for `stdlib/`)
  2. `P` == module path or `P` under it → dir under module root
  3. `P` == a required module's path (its *root* package) or `P`
     under it — longest match wins — → the resolved copy:
     `replace` dir, else `OXLMODCACHE` entry (exists only after V2;
     until then an unfetched require errors `missing module — oxl get
     it or replace it`)
  4. else → `cannot find package "P"` (suggest `oxl get` when P looks
     like a repo path)
- `internal/` rule: a path containing `/internal/` imports only from
  packages rooted at `internal`'s parent dir — the visibility
  mechanism that replaces decl-level `private`.
- Import-cycle detection across packages → `import cycle not
  allowed: a → b → a` (per-package, not per-file).
- `oxl.sum` presence is *not* required to resolve — it verifies fetch
  integrity (V2).
- The checker loads the whole build list's packages lazily: a
  `require` that nothing imports is legal in the manifest (tidy
  prunes) but contributes zero to the compile.
- Self-host: resolution rules live in a shared spec so `check.oxl`
  reproduces them; the differential corpus gains module fixtures
  (temp module trees on disk during the diag run).

## Acceptance

- Fixture repo: module `app` requiring `widget` via `replace
  ../widget`; `import "github.com/oxidean/widget/thing"` resolves to
  the replaced dir; `widget` code itself builds
- `cannot find package`, `missing module`, `import cycle`, and
  `internal/` violations each produce corpus'd diagnostics — seed +
  self-hosted checker agree
- Module with an unimported require compiles and emits nothing for it
- `oxl 9.9` floor errors cleanly on a v0.3 toolchain

## Depends on

M1 — manifests name *packages*, which don't exist until the clause
lands.

## Risks

- Resolution is now environment-sensitive (cwd, cache) — diag corpus
  needs hermetic fixture dirs; keep the resolver pure-function shaped
  (paths in, decls out) so both checkers can be tested identically.
