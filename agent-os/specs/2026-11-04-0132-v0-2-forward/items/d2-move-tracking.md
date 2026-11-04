# D2 — `own` move tracking

## Problem

Passing an `own` value transfers ownership — but today the checker
doesn't *track* that: `spawn worker(c); spawn worker(c)` compiles and
both tasks race on the same cell. The runtime catches some cases via
borrow bits (Shared cells survive), but `own`-typed structs double-use
silently. Soundness gap, documented since the worker-pool landed.

## Design sketch

Compile-time move tracking, deliberately shallow (not a borrow
checker — the runtime stays the semantic backstop):

- `env` tracks a `moved` bit per `own`-typed binding
- Using a moved binding = compile error ("use of moved value `c`")
- Passing `own` to an `own` param, `return own`, `field = own` — all
  move-sites; assignment to a moved-from binding clears the bit
- Control flow: **union** `moved` bits at joins — a move on ANY branch
  is possible after the join, so the bit survives unless every
  fall-through path re-initializes the binding. Branches that end in
  `return`/`break`/`continue` can't reach the join and don't
  contribute their moves. A `for` body's moves require care
  (moved-in-body is a compile error unless the binding is re-assigned
  in the same iteration — keep it simple: error)
- `own` returned from a fn is a fresh binding — no tracking needed

Explicitly out: partial moves (`a.b` fields), lifetime inference,
borrow regions — that's Rust's problem-set; this is the lean version
that catches the actual bug class (use-after-move) statically.

## Acceptance

- `spawn worker(c)` twice on the same `own` → compile error naming `c`
- `v := own_thing(); use(v)` → error; `v = new_own(); use(v)` → ok
- if/else merge: moved-in-both-branches → moved after; moved-in-one →
  moved after (later reads error); the mark clears only when every
  fall-through path re-initializes the binding
- Existing valid code (`worker(ch: chan)` — `chan` isn't `own`) keeps
  compiling — `chan`, `int`, `str`, `bool` are copy types, untracked

## Depends on

Nothing — self-contained checker pass.

## Risks

- False positives on the `for`-body pattern (`s = s.next` is a
  re-assign, not a move — must classify assignment-before-use
  correctly); the `StrList`/`Json` walks in stdlib are the test corpus.
- `Shared` cells are `own` — passing the same cell to two workers is
  *intended* and currently breaks this rule; spec must special-case:
  `mkshared`'s return is `own`-but-shareable (a `shareable` type flag)
  or the API takes `ref` params. **Decide at implementation** — the
  clean fix is `Shared`-typed params, not `own`.
