# C2 closeout — iface values + generics in `bootstrap/codegen.oxl`

**Goal:** the self-hosted codegen compiles `tests/methods.oxl` and
`examples/http-crud-sqlite` — both run identically to `oxl run`.

**Spec:** `docs/specs/2026-11-04-0132-v0-2-forward/items/c2-codegen.md`
(remaining-gap section). Reference implementation: `oxl/src/codegen.rs`
(lines noted per task).

**Constraint:** emitted C must match the seed backend's ABI — same
`RtIface` layout, `o_vt_*`/`o_vtt_*`/`o_vret_*`/`o_f_<inst>` names —
the runtime can't tell which compiler emitted it.

## Task 1 — method receivers + concrete dispatch

`Node.Fn.recv` is a `Node.Param`; `FnSig.params` already prefixes it.
Today `gfunc`/`gemit_proto` ignore `recv` (methods emit with no
receiver param) and `x.m()` has no dispatch at all.

- Emit `recv` as first param in protos + bodies (`gfunc`, `gemit_proto`,
  frame path via `qn`). Param ty via `gty(recv.ty)` → `gcty` (value →
  `struct o_s_T`, own/ref/mut → `struct o_s_T*`).
- `gcall`/`xty_call`: when `f` is `Node.Field{obj,m}` and `name` isn't a
  plain fn → `check.method_target(ck, gsub(xty(obj)), m)`. `mt.k==1` →
  `o_f_<q>` with receiver as arg0 adapted by `adapt_recv` (codegen.rs
  2224-2246: value-want + ptr-got → `(*e)`; ptr-want + value-got → `(&e)`).
  Bare `m(x,..)` → `mt.k==1` on args[0] without adapt (codegen.rs:2187-2195).
- `xty_call`: `k==1` ret = sig.retk/ret1, generic sigs subst'd by call
  bindings (recv+args vs sig.params via `check.bind_vars`).

Verify: scratch .oxl with `fn (p Point) n()` value+mut+own receivers,
both call shapes, enum receiver — emits, cc builds, runs.

## Task 2 — monomorphization

codegen.oxl currently skips `typarams` entirely. Port `insts`/
`inst_done`/`inst_name`/`call_bindings`/`func_inst` (codegen.rs:373-388,
549-600) over check.oxl primitives (`subst_ty`, `bind_vars`,
`FnSig.typarams`).

- `G.subst` (own `collections.Map`, nil = inactive) — `gsub(g,t)` =
  `check.subst_ty` when active; `gcty` and every type-kind decision on
  a declared/sig type applies it.
- `gcall` regular path: `sig.typarams != nil` → bind via `bind_vars`
  (want=sig.params, got=xty of effective args) → `o_f_<inst_name>` +
  queue unless done. `xty_call`: ret1 subst'd.
- `genall`: after bodies, drain `g.insts` — `gfunc_inst` = gfunc body
  under `g.subst` + mangled qn (susp insts emit frames under the
  mangled name). Loop to fixpoint (insts spawn insts).
- `gfunc`: skip `typarams != nil` fns (no uninstantiated emission).

Verify: `id[T any]`/generic-struct-param scratch file emits, builds,
runs.

## Task 3 — RtIface / vtables

- `gcty`: `k_iface()` → `"RtIface"`.
- `conv(g, e, want)` (codegen.rs:2250-2268): want-iface → got Err `"{0,0}"`,
  same iface passthrough, other iface rewrap `iface:<g>` implkey,
  concrete → `iface_wrap` (ptr impls hand data over; values box via
  `rt_alloc_own` — codegen.rs:2270-2295). Call sites mirror the seed emitter: let/global
  init, assign, return, call args (sig/vtab/method params), struct +
  enum literal fields, slice elem.
- `mt.k==2` → vtab call (codegen.rs:2364-2384): `({ RtIface _iv = <obj>;
  ((ret (*)(void*, params...))_iv.vtab[i])(_iv.data, args); })` —
  pair-ret casts to `struct o_vret_<W>_<m>`; args conv'd to declared
  `mt.ps`. `xty_call` ret from `mt.rk/r1`.
- `o_vret_<W>_<m>` decls in `genall` for pair-ret iface methods.
- `emit_vtabs` (codegen.rs:2041-2160): per `(w, implkey)` in `vtab_uses`
  (insertion-ordered list + dedup map) — thunk per method
  (`o_vtt_<w>_<impl>_<i>`) + `o_vt_<w>_<impl>[]`. Concrete impl:
  receiver-normalize `(*cn*)_d`/`((cn*)_d)`, forward to `o_f_<T>_<m>`
  or `o_f_<inst>` via `vtab_bind` (generic impl, codegen.rs:2005-2016);
  pair-ret wraps `o_ret_` → `o_vret_`. `iface:` impls forward through
  the source pair's matching vtab slot.
- `seed_vtab_insts` in the inst fixpoint (codegen.rs:2018-2037).
- `==`/`!=` on iface → compare `.data` (codegen.rs:1585-1595).

Verify: `tests/methods.oxl` + http pilot emit, build, run identically.

## Task 4 — coverage + docs

- `tests/run.sh`: re-add `tests/methods.oxl` and the http pilot to
  `cgen` coverage; drop the exclusion comment.
- `codegen.oxl` header: the "no generics" subset line is stale.
- `items/c2-codegen.md` status + `plan.md` C2 marker → done.
- Full `tests/run.sh` green.
