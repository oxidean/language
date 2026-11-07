## Summary

<!-- What this changes and why. Link issues (e.g. "Fixes #123") or the spec item it implements. -->

## Changes

-
-

## Test plan

<!-- Check what you actually ran. CI runs the full matrix: ubuntu, macos, windows, commit-lint. -->

- [ ] `cargo build --release` in `oxl/` — required for toolchain changes
- [ ] `./tests/run.sh oxl/target/release/oxl` — the full suite
- [ ] Self-check (`./tests/run.sh` already covers it) — required when `bootstrap/` changes
- [ ] `oxl fmt` clean on touched `.oxl` files
- [ ] Not run — explain below

## Checklist

- [ ] Behavior changes include tests (`tests/` or `tests/diag/` fixtures)
- [ ] Both checkers and both codegens stay in parity — seed toolchain and self-hosted must agree
- [ ] Diagnostics changes add or update `tests/diag/` fixtures
- [ ] Docs updated where behavior, commands, or the language surface changed
- [ ] Commit subjects follow `type(scope): subject` — see [CONTRIBUTING.md](https://github.com/oxidean/oxl/blob/main/CONTRIBUTING.md)
- [ ] No secrets or credentials committed
