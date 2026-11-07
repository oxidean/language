# V3 Implementation Plan — `oxl mod` commands

> Implements `items/v3-mod-commands.md`. Executed natively in-session.

**Goal:** manifests are never hand-maintained. `oxl mod init` writes the
skeleton; `oxl mod tidy` syncs requires/oxl.sum with what the source
actually imports (fetching missing modules, dropping unused ones);
`oxl mod verify` re-hashes the cache against oxl.sum; `oxl mod list`
surfaces the build list. All driver-side — the checker (`check.oxl`)
needs no parity work, same as `oxl get` in V2.

**Already on main:** `mod_ctx`/`build_list`/`module_dir`/`resolve_pkg`
(module machinery), `fetch::fetch` + `cmd_get` (fetch + pin),
`modcache::{slot,dir_h1,mod_h1,verify_slot,sums_text,parse_sums}`,
`parse_pkg_file` (AST import scan), `upsert_require` (single-line
require upsert), `cmd_list` (`oxl list -m all`).

## Design decisions

- **Dispatch**: `oxl mod <init|tidy|verify|list>` → `cmd_mod`. `oxl mod
  list` shares `oxl list -m all`'s printer (spec names both as the
  build-list surface).
- **Module = repo root** (V2 bound, restated): fetch resolves a module
  path to `https://<path>` / explicit URL and requires the root oxl.mod
  to declare that exact path. Subdir-in-repo modules can't be fetched —
  tidy's prefix probing still *finds* the containing module correctly
  because probes that 404 keep falling to shorter prefixes.
- **Tidy keeps `// indirect` requires**: the require set
  ends up = every module providing a package in the transitive import
  closure — unmarked when imported by main-module sources, `// indirect`
  when only imported by dep sources. Pinned at the *selected* version
  for indirects, kept floor for surviving directs. This is what makes
  `oxl list -m all` + oxl.sum stable across tidies.
- **Surgical manifest edits**: require entries are rewritten in place —
  dead lines dropped, versions swapped in place (`replacen`-style),
  `// indirect` markers added/removed, new paths inserted at sorted
  position inside the existing require region (block body or run of
  single lines). All non-require lines and comments survive byte-for-
  byte; a free-standing comment inside a rewritten region is preserved
  by re-emitting it before the region's entries.
- **Replace + tidy**: an import provided only through a replace dir
  still needs a require. Missing → `require <mp>
  v0.0.0-00010101000000-000000000000` (the local-replace placeholder form).

## Steps

### V3.1 — dispatch + init + verify + list (main.rs)

- `cmd_mod(args)` subdispatch; `oxl mod` with no/bad subcommand → usage.
- `mod init <path>`: cwd `oxl.mod` exists → `error: oxl.mod already
  exists`; validate path (non-empty, no whitespace/`@`); write
  `module <path>\n\noxl <tool_ver()>\n`. Print `created oxl.mod` +
  the module path.
- `mod verify`: needs module root (`find_root` from cwd, same
  no-module error as get). For every `(mp, mv)` in `ctx.selected`:
  replace-covered → skip (never verified by design); slot missing →
  `error: <mp>@<mv>: not in module cache`; else `verify_slot` —
  Err → `error: <that text>`. Success →
  `all modules verified`. Nonzero exit on any failure.
- `mod list` → `print_build_list(&ctx)` (extracted from `cmd_list`).

### V3.2 — `oxl mod tidy` (main.rs)

Phase A — main-module import walk:
- `main_mod_files(root)`: recursive `*.oxl` walk skipping `.`/`_`
  -prefixed dirs, `testdata`, and nested module roots (dir holding an
  oxl.mod that isn't `root`).
- Imports = union of `Item::Import` paths over `parse_pkg_file(_, [],
  false)` — lenient (a file with syntax errors still yields imports;
  real diagnostics stay the build's job).
- Classify each path: `std/` → skip; `ctx.path`-prefixed → skip
  (in-module); else external.

Phase B — fixpoint satisfy loop. Repeat until a full pass changes
nothing (bounded by module count):
- BFS package dirs seeded from main-module dirs: per file → imports →
  classify `std/`·in-module (skip)·external. `Resolve::Dir` results
  enqueue unseen dirs — dep sources get walked too. Importer class
  = file under root ∧ not inside a nested-module dir.
- An external import is *provided* when its longest require- OR
  replace-prefix covers it and that dir exists (`provided()`).
  Provider `mp` → marked `direct` (main importer) or `indirect`.
- Unprovided, importer is a dep package: read the dep's own oxl.mod —
  its longest require-prefix gives the (mp, ver) constraint it asks;
  require `mp` at the MVS-selected version (marked indirect), fetching
  the slot at that version when absent. Dep's manifest doesn't cover
  it → generic probe (below).
- Unprovided, importer is main (or dep fallback): for `M` = prefixes
  of `imp` longest→shortest (≥1 segs, `HashSet` of probed paths):
  fetch `M@latest` (+ declared==M) → extract to tmp → `tmp/rest` of
  `imp` is_dir? yes → install slot + require `M@ver` (or upgrade the
  existing require) and mark; no → next prefix. Required `M` whose
  pinned copy lacks the package still gets probed at @latest (the
  upgrade path). All fail →
  `error: no required module provides package "imp"; to add it: oxl get imp`.
- Replace-only coverage (import provided by a replace dir, no
  require): require `mp v0.0.0-00010101000000-000000000000`.

Phase C — final require set:
- Rebuild ctx from the in-memory manifest; `selected` = MVS build list.
- `direct` providers → `require p <kept floor|fetched ver>`;
  `indirect` providers → `require p <selected> // indirect`.
- Every non-replaced build-list member needs sums — fetched modules
  got pairs at install; still-missing → compute from the cache slot
  (`dir_h1` + `mod_h1` on `slot/oxl.mod`).

Phase D — rewrite files:
- `oxl.mod`: surgical edit per decisions above.
- `oxl.sum`: keep lines for still-required non-replaced modules,
  add fetched/computed pairs, drop the rest — `sums_text` output.
- NOTE (why indirect requires are load-bearing, not bookkeeping):
  `resolve_pkg` only ever scans the MAIN manifest's requires — a dep
  package's import of `b` can't resolve unless `b` is a main require.
  `// indirect` entries are how oxl's model flattens transitive deps
  into the main manifest (the pruned-module shape).
- Report: `added`/`upgraded`/`dropped`/`marked indirect` lines like
  get's.

### V3.3 — corpus + docs

- run.sh section (POSIX-gated, same `file://` constraint as V2's):
  `mod init` fresh dir → build accepts manifest; re-init refuses.
  Tidy: main imports `dep/util` with no require → fetches file:// dep,
  writes require+sums; unused `require other` dropped; `// indirect`
  appears for a transitive-only dep (dep requires leaf). Sorted
  output + comments/replace lines preserved. `mod verify` clean run
  prints `all modules verified`; hand-edit a cached file → fails with
  the familiar mismatch wording. `mod list` shows main + selected.
- README: `oxl mod` subsection beside the `oxl get` docs.
- plan.md: V3 → `[done — PR #n]` on merge.

## Out of scope (spec-deferred)

`oxl mod vendor`/`work`/`graph`/`why`/`edit`; `-mod` flags; `mod
download` (fetching stays import- or get-driven).
