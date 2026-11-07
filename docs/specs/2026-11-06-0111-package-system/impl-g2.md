# G2 implementation — rt.c section GC + feature gates

Spec: `items/g2-runtime-sections.md`. Branch `feat/pkg-g2` on `feat/pkg-m3`.

## What landed

- `emit_and_cc` (single cc site for build/run/test): adds
  `-ffunction-sections -fdata-sections` plus the platform GC
  spelling — `-Wl,--gc-sections` (ELF/mingw) / `-Wl,-dead_strip`
  (Mach-O). MSVC unreachable (pick_cc only tries cc/gcc/clang
  driver spellings).
- OXL_NET gate: emitted C is scanned for `rt_sock_|rt_bind_any|
  rt_set_(reuseaddr|rcvtimo|sndtimo)|rt_htons|rt_htonl` — an exact
  "uses sockets" test since those names only appear via socket
  externs/builtins. Sets `-DOXL_NET`; `-lws2_32` on Windows now
  follows the gate (was unconditional — nothing else references
  WinSock).
- rt.c wraps in `#ifdef OXL_NET`: `rt_htons`/`rt_htonl` (the
  oxl-callable wrappers only — `rt_poke16/32` call the htons/htonl
  macros directly and stay ungated), the `rt_set_*`/`rt_bind_any`
  block, the `rt_sock_*` block. `WSAStartup` inside `rt_init`
  compiles only under `_WIN32 && OXL_NET`; the `_setmode` binary-stdio
  block stays unconditional (not socket work). `rt_sigpipe_ignore`
  stays ungated (libc `signal`, callable without sockets).
- rt.h gates the same prototypes so undeclared calls can't slip
  through implicitly.
- OXL_GUI does not exist as a gate: GUI code never lived in rt.c —
  it is `gui_shim.o` + `LANG_LDFLAGS` (gtk4 pkg-config), already
  opt-in per program. Spec adjusted implicitly; noted here.
- run.sh's `cgen`/`--test`/gui-todo cc lines mirror the gate
  (grep → `-DOXL_NET`) — without it emitted socket programs fail
  to compile (prototypes hidden) and link.
- run.sh prints `hello.oxl binary: N bytes` (informational, like
  the a1 timings — no assert per spec).

## Measured

`hello.oxl`: 52,880 → 17,496 bytes (-67%). nm shows zero
rt_sock_/rt_bind_/rt_hton*/rt_set_(sock)/rt_sqlite_ symbols.
http-crud-sqlite builds, serves POST+GET (gates fire on use).

## Verify

`bash tests/run.sh oxl/target/release/oxl` → ALL TESTS PASSED
(incl. bootstrap codegen cc paths through the new gates).
