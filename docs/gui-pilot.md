# GUI pilot report (F1)

What was built: `examples/gui-todo/` — a GTK4 todo list (add/remove
rows) written in OXL, talking to GTK through `gui_shim.c`, a ~160-line
C ABI shim. This file is the decision input for the charter's
eventual portable-UI layer (path b); the pilot itself is path (c),
the FFI bridge.

## What the FFI surface felt like

**Good — better than expected.**

- *Handles-as-ints was enough.* Every GTK object is an opaque `int`
  (same idiom as socket fds in `std/net/http/http.oxl`). No wrapper types,
  no unsafe blocks needed — GTK owns widget memory, OXL never
  dereferences. The ownership story simply doesn't engage because
  nothing `own`-typed crosses the boundary.
- *Closures are the interface.* `gui_connect` takes `cb: fn(w: int)`
  and the shim calls `cb.fp(cb.env, widget)` — the B4 uniform
  `{env, fp}` convention means a C trampoline is six lines, and OXL
  closures carry all their context (the delete handler captures
  `lb` + `row`; no global widget registry exists in the app).
- *Signals round-trip real.* The selftest emits actual GTK signals
  (`g_signal_emit_by_name`) — the path tested is GTK → C trampoline →
  OXL closure → GTK mutation, not a shortcut.

**The friction points — all real signal for path (b):**

1. **Macro APIs need a shim.** `g_signal_connect`, `GTK_BOX(x)` casts,
   `G_CALLBACK` — all macros. The shim is ~15 one-line functions. Any
   C library with a macro-heavy API needs the same treatment; this is
   a *pattern*, not a one-off. A future FFI improvement: letting
   `extern` declare inline C (header snippets or a `.c` companion
   wired into the build) would cut the shim to zero files.
2. **Linking needed a new knob.** `LANG_LDFLAGS` was added to the
   driver for this (objects + libs appended to the `cc` line). Works,
   but a real FFI story wants per-file directives (`// link: gtk4`
   resolved through pkg-config) so consumers don't carry build
   scripts.
3. **`cstr → str` needed a builtin.** `gui_entry_text` returns
   `cstr`; `str(c)` was added (borrow, no copy — cheap and correct for
   GTK's borrowed buffers, but the borrow lifetime is on the
   programmer: the `str` aliases GTK's internal buffer until the
   widget's text changes).
4. **Thread affinity doesn't map to `spawnBlocking`.** GTK requires
   every call on the thread that ran `gtk_init`, so the main loop
   (`gui_run`) blocks `main` — you *cannot* push it to the pool and
   keep calling GTK elsewhere. The blocking pool is for *leaf* calls
   (sqlite, fs), not for a thread-bound subsystem. If the language
   grows native GUI support it needs "pin this task to this thread"
   (or a dedicated UI-thread primitive), not more pool semantics.
5. **Callback lifetime vs scopes.** The shim heap-copies the `RtFnv`
   and frees it (plus `rt_free` on the env) when GTK destroys the
   signal. Closure envs are `rt_alloc_own` — escaping storage, so a
   closure created inside `scope{}` already outlives its scope. What
   must not escape is captured *borrowed* data: a capture that points
   into the scope arena (e.g. a non-`own` slice's backing storage)
   dangles once that scope closes, and GTK holding it past that point
   would read freed memory. A path-(b) layer must document that rule
   as sharply as GTK documents its thread rule.

## What path (b) — a portable UI layer — would need

Measured against this pilot, the layer needs:

- **A retained widget tree** (the OXL app already thinks in trees:
  box/listbox/label/button — the mapping is 1:1).
- **Event → closure dispatch** as the only callback model (proven:
  `{env, fp}` is exactly a vtable slot; signals became closures
  without impedance).
- **String ownership rules** for borrowed platform buffers (the
  `str(c)` borrow is a place bugs will breed — path b wants an
  explicit `borrowed str` type or copies at the boundary).
- **A UI-thread model**, not `spawnBlocking` — one pinned thread owns
  the toolkit; app logic reaches it via channel/message, which OXL's
  `chan` + `select` already express well (the event loop becomes a
  `select` loop over UI events + app messages — this is the strongest
  argument that path b fits the language's existing concurrency
  story).
- **~15 functions worth of platform surface** per backend — the shim
  is the whole portability seam: window/box/entry/button/label/list/
  scrolled/connect/run. That is a small, honest contract.

## Leak check (acceptance)

`--selftest N` churns N add→click→remove cycles through real GTK
signals (each cycle heap-copies a `RtFnv`, runs a closure, then frees
both via the signal destroy-notify). Peak RSS vs N (WSLg, software
rendering):

| churn | peak RSS |
|------:|---------:|
|   400 | 274 MB |
| 4,000 | 278 MB |
|20,000 | 281 MB |
|100,000| 287 MB |

Marginal cost decays with N (~970→200→78 B/cycle) — a GTK/Pango
cache-warm-up curve, not a constant-slope leak. The OXL-side
per-connection footprint (~40 B/cycle) is verified freed by the
destroy-notify design; byte-level confirmation wants valgrind, which
isn't installed on the dev box.

## Reproduce

```sh
cd examples/gui-todo
./build.sh              # pkg-config gtk4 + ../../oxl/target/release/oxl
./todo                  # interactive (needs a display)
./todo --selftest       # signal-path assertions, auto-quits
./todo --selftest 50000 # churn-scaled RSS probe
```

`tests/run.sh` builds the pilot when `pkg-config gtk4` resolves and
runs `--selftest` when `DISPLAY` is set — headless CI compiles it
without launching.
