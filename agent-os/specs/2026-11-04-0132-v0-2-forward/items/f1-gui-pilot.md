# F1 — GUI pilot app

## Problem

The third acceptance app from the design doc — deferred post-v0.0.1 by
explicit product decision. The charter keeps all three paths open:
(a) FFI to platform toolkits, (b) portable UI layer in the language,
(c) Qt/GTK bridge. This spec picks the v0.2 path.

## Recommended path: (c) FFI bridge — GTK via C FFI

Rationale vs the other two:

- **(a) platform-native bindings**: three separate huge APIs; the
  design doc already calls this a project the size of the language.
  Wrong first step.
- **(b) portable self-owned layer**: the right *end* state (same
  widget tree renders to DOM later — the parked web target) but it's
  renderer+text+IME+a11y — a quarter-scale project on its own. Doing
  it before the language has GUI-shaped users builds it in the dark.
- **(c) Qt/GTK bridge**: the design doc's own concern is heavyweight
  deps fighting lean binaries — but the *bridge* is what tests the
  language: does `extern fn` FFI + event loops + `scope`-managed
  lifecycle actually work for a real GUI? GTK4 via C FFI is the
  cheapest working answer that produces real signal.

## Design sketch

- `examples/gui-todo/` — the pilot: a GTK4 window, a list, add/remove —
  the classic todo shape (exercises event loop + callbacks + C structs)
- `extern fn` GTK4 declarations (`gtk_init`, `gtk_window_new`,
  `g_signal_connect` via a small C shim for the macro-based signal API —
  the shim is a `gui_shim.c` next to the pilot, honest about macros)
- Event loop on `spawnBlocking` (blocking `g_main_loop_run` — the
  blocking pool exists precisely for this)
- `unsafe` blocks where GTK's memory model crosses ours (widget
  pointers are `own`-opaque)
- Document honestly: this proves the FFI+concurrency story for GUIs;
  it is not the language's GUI *strategy* — (b) remains the charter.

## Acceptance

- `examples/gui-todo/` builds + runs on Linux GTK4; add/remove items
  works
- No leaks visible in a 60s run (valgrind or RSS-flat)
- `docs/gui-pilot.md`: what the FFI surface felt like, what the
  portable layer (path b) would need — the decision input for path b

## Depends on

- Nothing hard (FFI exists, blocking pool exists). Scheduled last in
  the milestone because it's the biggest standalone item and benefits
  from any stdlib hardening that lands meanwhile.

## Risks

- GTK's macro-heavy API needs the C shim — that's why the shim is in
  the spec explicitly, not hidden.
- macOS/Windows GTK availability varies — the pilot targets Linux
  (the dev box + CI); cross-platform GUI waits for path (b).
