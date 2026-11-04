# D5 — Module qualification (`strings.split` namespaces)

## Problem

`import "std/strings"` merges every decl into one flat namespace —
`split`, `trim`, `expect` all collide globally. Two stdlib modules
exporting the same name already can't coexist; user code can shadow a
stdlib fn accidentally. The merged namespace was the right v0 call;
it's wrong as the surface grows.

## Design sketch

- `import "std/strings"` binds a namespace `strings`; members called
  `strings.split(s, sep)`
- `import "std/strings" as str` — alias
- `import "std/strings".{split, trim}` — selective flatten (opt-in,
  explicit — for names that deserve to be unqualified)
- Merged-namespace behavior stays for the *same* file and for
  `import "lexlib"`-style single-file libs? No — uniform rule: every
  import introduces a namespace; the last path segment is the default
  name. (Breaking change to all existing stdlib call sites — one-time
  sed-able, better now than at 10× the usage.)
- Compiler change: `fns`/`structs`/`globals` maps become
  module-scoped; lookup walks `current` then `import`-qualified.

## Acceptance

- `strings.split`, `json.jparse`, `sync.mlock` call sites rewritten
  across stdlib/examples/bootstrap — suite green after the migration
- Two modules exporting the same name coexist (`a.f` vs `b.f`)
- Unqualified calls to imported names = compile error (not silent
  shadow)
- `as` alias + selective import work

## Depends on

Nothing — but do it BEFORE Track C ports the checker (the ported code
should see the final module model, not the interim one). Also before
stdlib grows much further — each new module widens the migration diff.

## Risks

- The migration diff touches every `.lang` file — mechanical but wide.
  This is the one explicitly breaking change in the milestone; bundle
  it into a single commit and re-tag.
