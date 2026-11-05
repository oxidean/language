# E1 — LSP hover/goto + position index

## Problem

Diagnostics land on didOpen/didChange (v0.0.1), but hover and
goto-definition are the DX gap that makes the editor feel real — the
feature early adopters hit within minutes of opening a file.

## Design sketch

- Position index: after check, walk the merged crate collecting
  `(file,line,col) → decl` — fn sigs, struct fields, locals (from the
  typed env), globals, imports
- `textDocument/hover` → the decl's signature text (fn sig, struct
  field type, `let` inferred type, `own`/`mut` annotations shown)
- `textDocument/definition` → the decl's source span (`{file,line,col}`)
- Keep v0 honest: no references/find-all, no completions — hover +
  goto only; the index architecture supports them later

## Acceptance

- Hover over `jadd` → `fn jadd(parent: own Json, key: str, val: own Json)`
- Goto on `lex(` in `parser.oxl` → jumps to `lexlib.oxl:51` (imports
  resolve to the definition file)
- Diagnostics keep working; index rebuilds on didChange

## Depends on

Nothing — pure `main.rs` tooling work over the existing checker data.

## Risks

- Imported-file spans must map back to *their* files, not the merged
  crate's — the import-resolution span bookkeeping is the fiddly part
  (diagnostics already do this; reuse it).
