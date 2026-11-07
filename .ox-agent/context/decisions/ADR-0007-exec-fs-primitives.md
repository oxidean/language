# ADR-0007: process and fs primitives land in std/os + std/os/exec

Status: accepted (WI-0003) — 2026-11-07

## Decision
The driver-port language gaps become standard-library packages, not
builtins: `std/os` gains `mkdir_all`, `remove_all`, `rename`,
`temp_dir`, `getenv`, `exists`, `ls`, `fullpath` alongside the
existing file I/O; a new `std/os/exec` package carries process
control — `cmd(name, args)` with `run()` (inherited stdio, exit
status) and `output()` (captured stdout/stderr, exit status). rt.c
gains the backing primitives plus their E3 Windows-veneer twins:
`posix_spawn`/fork+execvp+`waitpid`+`pipe` on POSIX,
`CreateProcess`/`WaitForSingleObject` on Windows; `mkdir`(parents),
`rename`, recursive-remove.

## Rationale
- Go-faithful placement is the established std convention; fs funcs
  belong beside `os.open`, and process control is a heavy enough
  surface to earn its own package.
- Keeping them out of the language core prevents the driver port from
  silently growing builtins — the ADR-0004 "language-gap findings are
  new work" boundary is honored by making the gap visible as packages.
- Two exec flavors cover every driver need: `cc` (status, inherit),
  `git` (captured output), the test binary (status).

## Consequences
- `std/os/exec` is the *only* sanctioned process-spawn surface; later
  milestones (registry tooling, GUI helpers) reuse it rather than
  adding more rt entry points.
- FFI signatures may need widening (argv `[]str`, env, pipes) — each
  widening lands with checker+codegen changes in the same stage.
