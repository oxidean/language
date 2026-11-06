# M2 Implementation Plan — `oxl.mod` + module-aware resolution

> Implements `items/m2-module-manifest.md`. Executed natively in-session.

**Goal:** `oxl.mod` at the module root names a module and declares its
language floor, requires, and replaces. Import paths resolve against
the module graph — not ancestor walks. Repo becomes `module oxl`; all
in-tree imports gain the `oxl/` prefix.

**Architecture:** manifest parsing + root discovery + resolution live in
`oxl/src/main.rs` (a new `mod manifest` would bloat main.rs — keep the
manifest parser + `ModCtx` + `resolve_pkg` together in main.rs beside
`load_pkg`, same as M1). `check.oxl` mirrors inside its `resolve_imp` /
`load_pkg` pair with identical diagnostics. New runtime extern
`rt_getenv` feeds `OXLMODCACHE`/`HOME` to the self-hosted side.

## Grammar (both parsers, identical)

```
module <path>
oxl <version>
require <path> <version>          |  require ( <path> <version>  ... )
replace <path> [@<ver>] => <dir>  |  replace ( ... )
```

- `//` comments; blank lines skipped; `)` closes a block.
- Unknown directive → `oxl.mod:<line>: unknown directive "x"`.
- Missing argument → `oxl.mod:<line>: expected <what>`.
- v1 accepts a `replace` whose RHS is a local dir (`./`, `../`, `/`,
  or a plain relative path) — module→module replace is a deferral.
- require versions: shape-checked `v<digits>.<digits>.<digits>` or
  pseudo-version (any `vX.Y.Z-*` suffix) — full semver grammar is V1.
- Duplicate `require`/`module`/`oxl` lines → `oxl.mod:<line>: duplicate directive "require"`.

## Root discovery

`find_root(start_dir)`: walk ancestors for `oxl.mod`. Nearest wins;
nested modules are separate modules. No `oxl.mod` → module mode OFF:
only `std/` imports resolve, everything else is `cannot find package`.

## Resolution (`resolve_pkg(ctx, from_file, imp)`)

In order:

1. `std/...` → `<ancestor>/std/<rest>` walking up from the toolchain
   exe (`rt_argv(0)` / `current_exe`) — shipped-tree mechanism.
   Missing dir → `cannot find package`.
2. `imp == module.path` or `imp` under `module.path/` →
   `<module.root>/<rest>`; rest empty → the root dir itself.
3. Longest require-prefix match (`imp == req.path` or under it):
   `replace` RHS (local dir) → `<rhs>/<rest>`; else
   `<modcache>/<path>@<ver>/<rest>` when that dir exists; else
   `missing module`.
4. `imp` starts `./` or `../` → `relative import paths not supported`.
5. Else → `cannot find package`.

`internal/` rule applied post-resolution: if the resolved dir contains
a `/internal/` segment, the importing package's dir must be inside the
tree rooted at that `internal`'s parent — else `use of internal package`.

Cycle detection replaces M1's silent dedup: `loading` stack of
(dir, written-imp) pairs; re-entering an in-flight dir →
`import cycle not allowed: <imp> → <imp> → <imp>` (each edge prints the
import path as written at that step; the first edge is the root's own
written path — deterministic across both checkers).

## Diagnostics (normative — corpus parity)

| Condition | Message (all `error [check]`) |
|---|---|
| Manifest | `oxl.mod:<line>: unknown directive "x"` / `expected <what>` / `duplicate directive "x"` / `unterminated require block` |
| Floor | `oxl.mod: requires oxl <v>, toolchain is <V>` |
| Remote-ish path, no match | `cannot find package "P" (missing module — oxl get it or add a require)` — remote-ish = first segment contains `.` |
| Local-ish path, no match | `cannot find package "P"` |
| Declared require, no replace, no cache dir | `missing module "P@V" — oxl get it or replace it` |
| Cycle | `import cycle not allowed: a → b → a` |
| internal | `use of internal package "P" not allowed` |
| Import path → missing dir after resolution | `cannot find package "P"` |
| `std/...` missing | `cannot find package "P"` |

(The M1 load diags — mixed clauses, dir-name, program-import,
no-.oxl-files — are unchanged.)

## Module context per package (v1 simplification)

Resolution always uses the MAIN module's ctx (its requires + replaces),
even for files inside required modules — MVS arrives in V1 and flattens
the build list anyway. `internal/` and cycles are filesystem/pure, so
module identity only matters for resolution. Documented in the item.

## Runtime: `rt_getenv`

`OxlStr rt_getenv(OxlStr name)` → value or `""`. rt.c + rt.h; oxl side
declares `extern fn rt_getenv(name: str) -> str`. Modcache dir:
`OXLMODCACHE` else `HOME`/`USERPROFILE` + `/.oxl/mod`.

## Tasks

- **M2.1** Rust: manifest parser, `find_root`, `ModCtx`, resolution
  rewrite, internal/ + cycle checks, `oxl <ver>` floor vs
  `env!("CARGO_PKG_VERSION")` (Cargo.toml → `0.3.0`).
- **M2.2** `.oxl`: `rt_getenv` extern (rt.c/rt.h + codegen extern ok
  by decl), manifest parser + ctx on `Ck`, mirrored resolve/cycle/
  internal + identical diags.
- **M2.3** Migration: root `oxl.mod` (`module oxl`, `oxl 0.3`);
  every `import "bootstrap|tests|examples|prototype/..."` → `oxl/...`;
  `std/` unchanged. run.sh heredocs + docs that show import lines.
- **M2.4** Module fixtures: `tests/mod/<case>/` trees; run.sh loop
  runs both checkers from each fixture root and diffs — covers
  replace-ok, cannot-find (remote+local), missing-module, cycle,
  internal, floor, unimported-require-ok.
- **M2.5** Docs: AGENTS.md/README module section; spec status flip.

## Deferred explicitly

Transitive dep requires (V1 MVS), `exclude`/`retract`, module→module
replace RHS, `oxl get`/`oxl.sum` (V2), `oxl mod` commands (V3), std
meta-imports (`std` alone = the std root — stays an error).
