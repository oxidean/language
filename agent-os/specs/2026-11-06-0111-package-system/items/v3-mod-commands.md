# V3 — `oxl mod` commands

## Problem

Manifests shouldn't be hand-maintained. Go's ergonomics — `mod init`,
`mod tidy`, `mod verify` — are the DX bar.

## Design sketch

- `oxl mod init <path>` — write a skeleton `oxl.mod` (`module`,
  `oxl <toolchain-ver>`); refuses to overwrite.
- `oxl mod tidy` — the M3 companion:
  - walk every import in the main module's packages
  - `std` imports skipped; module-path imports skipped; missing →
    `oxl get` — but on the *module*, not the package path: resolve
    the import to its containing module first (longest match against
    existing requires; otherwise strip trailing segments until a
    module root probes true — `github.com/a/b/c` may live in module
    `github.com/a/b`), then fetch `@latest` and require that
  - requires that no package in the build list imports → dropped
    (this is what *keeps unused deps out of the build* — pairs with
    M3's error at the manifest level)
  - rewrites `oxl.mod` (sorted, formatted) + `oxl.sum`
- `oxl mod verify` — re-hash cache entries vs `oxl.sum`.
- `oxl mod list` / `oxl list -m all` — build list (V1's debug surface
  lands here as the user-facing command).
- Deferred (noted, not gaps): `vendor`, `work`, `graph`, `why`,
  `edit`.

## Acceptance

- `init` → valid minimal manifest that `oxl build` accepts
- Tidy fixture: unused `require` dropped; import of an un-required
  module fetched + required; `oxl.mod` diff is minimal and sorted
- `verify` catches a hand-edited cache file
- Everything above with `-mod=mod`-free UX — no flags needed for the
  normal path

## Depends on

V2 (fetch + sum exist), M3 (the error tidy moots).

## Risks

- Tidy editing `oxl.mod` must preserve comments/replace lines users
  wrote — keep the manifest AST comment-attached or do surgical
  line edits; don't pretty-print the user's file away.
