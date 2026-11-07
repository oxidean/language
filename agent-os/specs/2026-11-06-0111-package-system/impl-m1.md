# M1 Implementation Plan — `package` clause + directory packages

> Implements `items/m1-package-clause.md`. Executed natively in-session.

**Goal:** every `.oxl` file opens with `package <ident>`; import paths
resolve to directories; all `.oxl` files in one directory form one
package sharing a decl namespace; bound name = declared clause name.

**Architecture:** the seed toolchain (`oxl/src/`) gains clause parsing
(`Crate.pkg`), directory-package loading (`load_pkg`), and file/dir
argument modes. The self-hosted bootstrap (`bootstrap/`) mirrors the
same semantics (`resolve_imp`/`load_mod` → dir versions) using a new
`rt_lsdir` runtime extern. The whole tree migrates in the same change:
`stdlib/` → `std/<name>/`, `bootstrap/` relayout to lib + `cmd/` dirs,
and every file gets a clause.

**Spec:** `items/m1-package-clause.md`, `shape.md` (decisions 1–5).

## Global constraints

- Diag parity: every diagnostic the seed side emits for package
  loading must be byte-identical from `check.oxl` — they share the
  `tests/diag/` corpus. Wording below is normative.
- `package` becomes a keyword in BOTH lexers (seed `lexer.rs`,
  `lexlib.oxl`).
- Canonical module qualifier = **clause name** (same-collision
  semantics as today's file stems; M2 upgrades identity to module
  paths).
- Dir scan order: filenames sorted ascending — deterministic loads in
  both toolchains.
- `package` clause must equal the directory basename unless `main`;
  enforced only for dir-loaded packages (imported dirs + dir-mode
  roots), never for file-mode roots.
- Commit format `type(scope): subject`, no attribution trailers
  (`.githooks/commit-msg` enforces).

## Diagnostics (normative strings)

| Condition | Message (kind `check` unless noted) |
|---|---|
| Missing clause | parse diag `expected package clause` (NOT in corpus — parse diags don't surface through check.oxl) |
| Clause not first | parse diag `expected package clause` |
| Mixed clauses in dir | `found packages a (a.oxl) and b (b.oxl) in <dir>` |
| Clause != dir basename | `package name "a" does not match directory "b"` |
| Dir has no .oxl files | `no .oxl source files in <dir>` |
| Importing a `package main` dir | `"<imp>" is a program, not an importable package` |
| Unresolvable path | existing `cannot resolve import `x` (from `f`)` |

`<dir>`/`<imp>` are the resolved path / written import path exactly as
each side sees them — corpus fixtures keep paths relative and
identical by construction.

## Task M1.1 — seed front-end: keyword + clause + `Crate.pkg`

**Files:** `oxl/src/lexer.rs`, `oxl/src/parser.rs`, `oxl/src/ast.rs`,
`oxl/src/fmt.rs`, `oxl/src/main.rs` (parse callers compile-fix only)

- `lexer.rs`: `"package" => Some(Kw::Package)`; add `Kw::Package`.
- `ast.rs`: `pub struct Crate { pub pkg: String, pub items: Vec<Item> }`
  (replacing `items`-only construction sites).
- `parser.rs`: `parse()` expects `package <ident>` first — missing →
  `expected package clause` diag at first token's span; clause is not
  an `Item`, so `item_start()` unchanged. Optional `;` after clause.
- `fmt.rs`: emit `package <pkg>` + blank line before items in `krate()`.
- Compile-fix every `ast::Crate { items }` / `krate.items` consumer.

**Verify:** `cargo build` in `oxl/`; scratch file `package main +
fn main(){println("x")}` compiles+runs; file without clause → parse
diag.

## Task M1.2 — seed loader: `resolve_pkg` + `load_pkg` + modes

**Files:** `oxl/src/main.rs`

- `resolve_import` → `resolve_pkg`: import path names a **directory**.
  Order: (1) `dir_of(from)/imp` is dir; (2) nearest ancestor `d` of
  from with `d/imp` a dir (generalizes today's `stdlib/` walk — `std/`
  stops being special); (3) same walk from `current_exe`. Nothing else
  resolves (a path hitting a file fails). No `.oxl` appending.
- `load_mod` → `load_pkg(dir, seen, loud)`:
  - dedup `seen` on canonical **dir**;
  - `read_dir` → `*.oxl` sorted; empty → `no .oxl source files in <dir>`;
  - per-file parse; collect clauses — mixed → `found packages a
    (a.oxl) and b (b.oxl) in <dir>` (first two sorted-order offenders);
  - clause != dir basename && != "main" → `package name "a" does not
    match directory "b"`;
  - clause == "main" && loaded-as-import → `"<imp>" is a program, not
    an importable package` (root load passes `as_import=false`);
  - pass A: collect decl names across ALL files → shared renames
    `n -> pkg.n` (pkg = clause); pass B: per file, renames = shared +
    that file's selective imports, aliases = that file's imports —
    file-scope; qualify items; extend merged items/item_files.
  - recursion per file's imports (a file's imports are its own).
- Root: `load_crate` unchanged role — root decls bare (`modname ""`).
- File list mode: `oxl build a.oxl b.oxl` — all listed files same dir +
  same clause `main` (diag reuse); virtual package = the listed files
  only. `oxl build` no arg → cwd dir package; dir arg → dir package.
  `run`/`test` same. Dir/file-mode root requiring `main` clause:
  `package "x" is not a main package`.
- `analyze()` (LSP): refactor onto `load_pkg` with a buffer override —
  the buffer's dir is the package dir, buffer text substitutes its
  file on disk.

**Verify:** scratch tree `pkg/a/x.oxl+pkg/a/y.oxl` (package a, cross-
file calls), `main/m.oxl` importing `pkg/a` — build+run; all error
diags fire; `oxl run` file-mode on old single file works.

## Task M1.3 — runtime extern `rt_lsdir`

**Files:** `runtime/rt.c`, `runtime/rt.h`

- `rt_lsdir(const char *path) -> OxlStr`: names in dir, `.`/`..`
  skipped, sorted `strcmp`, joined `\n`; `""` on any failure.
  POSIX `opendir/readdir`; Win32 `FindFirstFileW` through the E3
  veneer already in rt.c (check `#ifdef _WIN32` blocks nearby).
- Declared in `.oxl` as `extern fn rt_lsdir(path: cstr) -> str` where
  needed (bootstrap check). No checker/codegen wiring — externs are
  self-declaring.

**Verify:** scratch .oxl calling extern directly prints sorted names.

## Task M1.4 — bootstrap port

**Files:** `bootstrap/lexlib.oxl`, `bootstrap/parser.oxl`,
`bootstrap/check.oxl`, `bootstrap/codegen.oxl` (imports only)

- `lexlib.oxl` `is_kw`: add `"package"`.
- `parser.oxl`: `parse()` requires clause first → `Node.Package{name}`
  as first item in `Root.items` (new Node variant + `to_json` case);
  missing → lex-style error print + nil (parse failures aren't corpus
  diags). `p_item` dispatches `package` only at pos 0 — after items,
  `package` is not an item start (error path like the seed).
- `check.oxl`: `resolve_imp` → dir version (same 3-step order minus
  exe-walk — note: corpus only exercises in-repo paths; document the
  asymmetry in a comment). `load_mod` → `load_pkg(ck, ld, dir)`:
  `lsdir` extern → names → filter `.oxl` → sort → per file read/parse;
  clause checks (same strings); collect `mod` = clause name for
  FileAlias keying; dedup `ck.loaded` on dir; per-file alias tables
  unchanged in shape. `check_file(path)` → file-mode entry: dir of
  path + single-file package semantics — actually: check_file gets a
  FILE arg (CLI); treat as file-mode root (no dir-name check, siblings
  not loaded — matches seed file mode). BUT its imports still resolve
  to dir packages.
- `codegen.oxl`: no loader logic of its own (calls `check_file`) —
  import-path updates only.

**Verify:** seed-toolchain `oxl run bootstrap/check.oxl -- <fixture>`
matches seed diags on the same fixture.

## Task M1.5 — tree migration + bootstrap relayout

One commit covering:

- `stdlib/x.oxl` → `std/x/x.oxl`, `package x`; imports `"std/x"`
  unchanged (path now a dir).
- `bootstrap/lexlib.oxl` → `bootstrap/lexlib/lexlib.oxl`
  (`package lexlib`); `bootstrap/parser.oxl` → `bootstrap/parser/`
  (`package parser`, `fn main` extracted); `bootstrap/check.oxl` →
  `bootstrap/check/` (`package check`, `fn main` extracted);
  `bootstrap/codegen.oxl` → `bootstrap/cmd/codegen/main.oxl`;
  new thin mains `bootstrap/cmd/{lex,parse,check}/main.oxl`
  (`package main`, import the lib dir, call its entry).
  Bootstrap imports become repo-rooted: `import "bootstrap/parser"`.
- `tests/*.oxl`, `tests/diag/*.oxl`, `examples/*.oxl` loose files →
  `package main` in place (file-mode); `examples/{cli,gui-todo,
  http-crud-sqlite}` → `package main`.
- `tests/run.sh` paths: `bootstrap/X.oxl` → `bootstrap/cmd/X` dir arg
  (exercises dir mode end-to-end), `stdlib/` → `std/`, diag glob if
  needed, LSP test's `bootstrap/lexlib.oxl` URI → new path.
- `.github/workflows/*`, `scripts/`, `docs/*`, `README.md`,
  `AGENTS.md`, `agent-os/product/*` path references.

**Verify:** full `tests/run.sh` green — incl. self-compile
byte-identity, diag parity, fmt --check, LSP definition test.

## Task M1.6 — new corpus + coverage

- `tests/diag/pkg_mixed/` (two files, clauses a+b), `pkg_dirname/`
  (clause != dir), `pkg_main_import/main.oxl` importing a main dir,
  `pkg_empty/` — all must fail identically both checkers. NOTE corpus
  glob is `tests/diag/*.oxl` — these are DIR fixtures → harness needs
  a second loop over `tests/diag/pkg_*/` roots (`oxl build <dir>` vs
  `selfcheck <dir>/main.oxl`? — no: selfcheck takes a file arg; give
  each pkg dir a `main.oxl` and feed the IMPORTING root file to both
  checkers so paths match).
  → simpler: fixtures are an importing root `tests/diag/pkg_*.oxl`
    (file-mode) + a `tests/diag/pkg_*/` dir — root imports the dir and
    hits the error. Keeps the flat glob.
- Multi-file package positive test: `tests/pkg/` — main root
  (`tests/pkg_main.oxl`) importing a two-file `tests/pkg/` package,
  asserts cross-file call works.

## Risks / watch-items

- `check.oxl` resolution has no exe-adjacent fallback — std imports of
  out-of-tree targets already behaved that way; unchanged.
- `package` as identifier: repo grep found zero uses — keyword is
  safe.
- File-mode `oxl run bootstrap/check/check.oxl` would treat a lib file
  as a command — the bare-file hazard; scripts use dir mode.
