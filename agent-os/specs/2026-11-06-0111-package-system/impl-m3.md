# M3 implementation plan — unused imports are compile errors

Spec: `items/m3-unused-imports.md` (builds on M1 bindings + M2
resolver). Branch `feat/pkg-m3` on `feat/pkg-m2@8337470`.

## Normative diagnostics

| case | message |
|---|---|
| unused whole import / alias | `imported and not used: "PATH"` |
| unused selective name | `imported and not used: "PATH".{NAME}` |
| `import "P" as _` | exempt — no binding registered |

Per-file rule: each file's import list is judged against that file's
decls only. One diag per unused binding, in source order; the compile
still aborts (loud → `None` / `load_fail`).

## Task 1 — Rust (`oxl/src/`)

1. Parser (`parser.rs`): after `as`, accept `P::Underscore` →
   `alias = Some("_")` (today `_` lexes as Underscore, not Ident —
   `as _` is a parse error without this).
2. Loader (`main.rs`, `load_pkg_inner` per-file pass):
   - New `struct Used { quals: HashSet<String>, sels: HashSet<String> }`.
   - `map_name` grows `used: &mut Used`: an `aliases.get(head)` hit
     records `used.quals.insert(head)` (local qualifier name); a
     `renames.get(n)` hit records `used.sels.insert(mapped)` (the
     *resolved target* `bound.s` — keys attribution so colliding sel
     names across imports can't alias each other). Callers in
     `qualify_expr`/`qualify_ty`/`qualify_block`/pattern paths thread
     it through (signature change is mechanical).
   - `as _`: skip `aliases.insert` + exempt the whole import.
   - While registering bindings in pass 1, collect
     `binds: Vec<(path, qual, sels_expected: Vec<String>)>` in import
     order (`sels_expected` = `bound.s` per `.{s}`).
   - After pass 2, walk `binds`: unmarked qual → `imported and not
     used: "path"`; unmarked sel target → `imported and not used:
     "path".{s}`. `loud`: eprintln each then `return None`. Quiet:
     skip entirely (LSP tolerates dead imports).
3. `cargo build --release`; hand-verify the four fixture cases.

## Task 2 — `.oxl` mirror (`bootstrap/`)

1. `lexlib` already lexes `_` (match wildcards) — `parser.oxl`'s
   `alias = peek(c)` accepts `as _` verbatim; no parser change
   expected — confirm with a fixture before assuming.
2. `check.oxl`:
   - `struct ImpBind { path: str, qual: str, tgt: str, used: int,
     next: own ImpBind }`; `FileAlias` gains `binds: own ImpBind`.
   - At the `file_aliases` push site (≈5441): per import append a
     bind per qualifier (`qual=localname`, `tgt=""`) — skipped for
     `as _` — plus one per selective (`qual=""`, `tgt=bound+"."+s`).
   - `alias_stem` hit → mark `fi`'s bind where `bind.qual == q`.
     `selective_name` hit → mark where `bind.tgt == returned stem`.
     (`ref Ck` field writes compile here — verified.)
   - Unused pass at the end of `check_file` (after
     `check_suspend_rules`): iterate `file_aliases` → binds → emit
     `diag(ck,"check","imported and not used: \"PATH\"")` /
     `"PATH".{SEL}` per unused bind, then `ck.load_fail = 1` if any.

## Task 3 — fixtures + acceptance

- `tests/diag/unused_import.oxl` — `import "std/strings"` unused →
  reject (corpus requires rejection).
- `tests/diag/unused_selective.oxl` — `.{split, join}`, uses `split`
  → reject naming `.{join}`.
- `tests/mod/import-blank/` `expected=ok` — `import "std/strings"
  as _` (positive; can't live in tests/diag — corpus asserts reject).
- `tests/mod/import-used/` `expected=ok` — both qualifier + sel used.
- Repo cleanliness: `oxl build` every pkg in-tree; fix any dead
  import the M1/M2 migration left.
- `tests/run.sh` unchanged (diag corpus + m2 section cover all four).

## Task 4 — verify + ship

`cargo build --release`, full `tests/run.sh`, parity check on all
fixtures, `oxl fmt -w` the touched .oxl, commit
`feat: unused imports are compile errors (M3)`, PR stacked on #6.
