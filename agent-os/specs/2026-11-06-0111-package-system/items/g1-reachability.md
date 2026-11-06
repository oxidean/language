# G1 — Call-graph reachability in codegen

## Problem

The merged AST emits *everything*: every fn in every imported package
reaches the binary whether called or not. Unused imports are now
errors (M3), but *used* packages still drag their dead fns. Go's
linker GCs this by default; ours must too — "no bloat" is the user's
requirement, not a nice-to-have.

## Design sketch

- Roots: `main` (build/run), each `#[test]`-style entry (test mode),
  `extern fn` decls that are called, fns taken as **values** (a fn
  value makes its target reachable — the conservative call is to mark
  any fn whose name is referenced outside a call), `main`'s package
  globals, iface vtab entries of instantiated `(iface, implkey)`
  pairs (reuse the existing `vtab_uses` set).
- Mark pass over the merged AST: call edges via resolved callee
  names; `x.m()` iface calls mark the vtab slots (per-impl mark via
  `vtab_uses`, which is already exact — not all impls, just used
  pairs); generic instantiations mark under their mangled names.
- Type reachability: a live fn's param/ret/field types must survive
  even if never called "on" — walk `gcty`-consumed types transitively
  (struct fields pull their field types; enum payloads likewise).
- Drop: dead fns, structs, enums, ifaces, globals, `o_vt_*`/`o_vtt_*`
  rows, `o_vret_*` decls, uninstantiated generic decls (already
  skipped).
- Both backends: `codegen.rs` and `codegen.oxl` — identical marking,
  identical output; the self-compile byte-identity gate catches drift.
- Always on (Go-default). `-gc-off` debug escape hatch noted but not
  required.

## Acceptance

- Fixture: import a package, call one of three exported fns — emitted
  C names the called one only; the other two don't appear
- Binary size: `hello.oxl` shrinks measurably vs pre-G1 baseline
  (assert a recorded % in run.sh? no — assert emitted-C size / symbol
  count instead; binary size varies by platform)
- `tests/methods.oxl` + http pilot still emit and run identically —
  vtab marking must not drop a needed impl
- Fn-value fixture (`tests/fnval.oxl`): targets of `let f = g` are
  emitted even though `g` is never "called" by name

## Depends on

M1 (package boundaries make the merged AST's provenance clean; the
mark pass itself works either way, but landing it pre-M1 rewrites it
once for free).

## Risks

- Fn values + iface dispatch are the leak vectors — both are handled
  conservatively above; verify with `fnval`/`methods`/`pairfwd`
  fixtures before trusting the pass.
