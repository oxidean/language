# E3 — Platform matrix (CI + runtime portability)

## Problem

CI is Linux-only; `runtime/rt.c` is POSIX/pthread all the way
(`pthread_cond`, `clock_gettime`, `usleep`, sockets via `<arpa/inet.h>`).
The design doc's recommendation: x86_64 + aarch64, Linux/macOS first,
Windows next. "Runs on the user's machine" is an adoption blocker.

## Design sketch

- CI matrix: `ubuntu-latest` + `macos-latest` (aarch64), then
  `windows-latest` — `tests/run.sh` on each
- Runtime portability:
  - macOS: pthreads fine; `clock_gettime`/`usleep` fine; sockets fine —
    likely works as-is
  - Windows: needs a portability layer — `#ifdef _WIN32` shims for
    threads (or `std::thread` via a C++ shim), `Sleep`, `WinSock` —
    the smallest honest ifdef surface, or a second `rt_windows.c`
    behind the same `rt.h` contract
- `lang`'s own C-compiler invocation: `cc` on POSIX, `cl` or `clang`
  on Windows — the driver already invokes `cc`; parameterize the
  compiler lookup (env override `LANG_CC` first)

## Acceptance

- Green CI on ubuntu + macOS (aarch64)
- Windows build compiles + suite passes (or honest "POSIX-only v0.2"
  if WinSock/portability drags — the matrix decides, not the spec)
- `LANG_CC` env override documented

## Depends on

- Nothing — CI + shim work. Independent worktree item.

## Risks

- Windows is the only real unknown; the spec says POSIX-first and
  lets the Windows slice slip within the milestone, not silently cut.
