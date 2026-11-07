# G2 — `rt.c` section GC + feature gates

## Problem

`rt.c` (~2,400 lines — sockets, sqlite, gui, sync) links wholesale
into `hello.oxl`. G1 trims emitted code; this trims the runtime
itself — the other half of the bloat fix.

## Design sketch

- Compile `rt.c` with `-ffunction-sections -fdata-sections`, link
  with `--gc-sections` (ELF) / `-dead_strip` (mach-O) /
  `/Gy`+`/OPT:REF` (MSVC) — spelled per-platform through the E3
  compiler-detection veneer in `main.rs`.
- Feature gates generalize: today `-DOXL_SQLITE` is set by scanning
  the source for `rt_sqlite_`. Extend to a scan over *used* externs:
  `rt_sqlite_*` → `OXL_SQLITE`, `rt_gui_*` → `OXL_GUI`, `rt_sock_*`
  (+ `rt_bind_any`, `rt_set_reuseaddr`) → `OXL_NET`. `rt.c` wraps each family in `#ifdef` so ungated programs
  don't even compile the code — belt + suspenders under section GC.
- Ordering rule learned the hard way (`48aae28`): `-l` flags always
  after all inputs on the link line — keep that invariant when gates
  add/remove libs (`-lsqlite3` only when `OXL_SQLITE`).
- Acceptance measurement: `hello.oxl` binary records a size before/after
  in the run.sh log (no hard assert — informational, printed like the
  timings already are).

## Acceptance

- `hello.oxl` binary contains no sqlite/gui/socket symbols
  (`nm`/platform equiv shows none) and drops vs baseline
- http pilot still builds + serves (gate sets correctly when used)
- gui example builds only when its lib is present — gate doesn't
  force `-lgtk` on unrelated programs
- All three platforms green in CI — each linker spelling works

## Depends on

Nothing beyond the E3 veneer (landed). Parallel-safe with G1 — both
halves combine multiplicatively but neither blocks the other.

## Risks

- MSVC section-GC flags differ from GCC/clang on Windows — the E3
  veneer already picks per-compiler flag sets; extend it, don't
  duplicate detection.
