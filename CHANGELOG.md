# Changelog

All notable changes to oxl are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Everything since v0.2.0: the module and package system, and the bulk of
the standard library.

### Added

- **Packages**: `package` clauses and directory packages; import paths
  resolve to directories and the bound name is the declared package name
- **Modules**: `oxl.mod` manifests (`module`/`oxl`/`require`/`replace`),
  root discovery, `internal/` visibility rule, import-cycle diagnostics
- **Unused imports are compile errors**, reported per file
- **Versioning**: semver requirements, pseudo-versions for commits, `/vN`
  module-path suffixes, and MVS (minimal version selection) build lists
- **Module cache**: content-addressed `~/.oxl/mod`, `oxl get` fetching
  over git, and `oxl.sum` integrity hashes verified on load
- **`oxl mod` commands**: `init`, `tidy`, `verify`, `list -m`
- **`std` module tree**: packages on their canonical paths —
  `encoding/json`, `net/http`, `net`, `path`, `path/filepath`,
  `strings`, `io`, `errors`, `strconv`, `bytes`, `bufio`, `fmt`, `os`,
  `time`, `sync`, `testing`, `sort`, `slices`, `maps`, `cmp`, `math`,
  `math/rand`, `unicode/utf8`, `flag`, `log`, `encoding/base64`,
  `encoding/hex`, `encoding/csv`, `encoding/binary`, `hash`, `crypto/md5`,
  `crypto/sha256`, `crypto/hmac`, `regexp`
- **`byte` / `[]byte`**: a distinct `u8` type with char-literal sugar and
  `as` conversions
- **`err` messages**: interned message registry so errors carry text —
  `errors.new(msg)`, `errors.msg(e)`, `errors.code(e)`
- **Dead-code elimination**: call-graph reachability pruning in both code
  generators, plus section GC and `OXL_*` feature gates so unused runtime
  machinery (sockets, sqlite, GUI, math) drops out of binaries
- **Windows support**: platform veneer, sockets, and a CI leg
- **Self-hosted codegen**: interface values and generic instantiations

## [0.2.0] - 2026-11-05

The v0.2 language: the full forward spec, a self-hosted compiler, and
pilots exercising every subsystem.

### Added

- **Concurrency**: suspending functions lower to a state-machine
  transform so `spawn`ed tasks suspend without OS threads; `select` with
  timeout and default arms; buffered channels
- **Types and flow**: sum enums and `match`, slices (`[]T`, `own []T`),
  maps, `float`, first-class function values and closures, `defer`,
  methods on any named type, interfaces, generics with monomorphization,
  dotted interface constraints (`T cmp.Ordered`)
- **Ownership**: compile-time move tracking on top of the dynamic
  borrow checks — use-after-move is now a diagnostic, not a trap
- **Toolchain**: single `oxl` binary — `build`, `run`, `test`, `fmt`,
  `doc`, `lsp`; C emission through the system `cc`
- **Self-hosting**: lexer, parser, checker, and code generator written in
  oxl under `bootstrap/`; the self-compiled chain emits byte-identical
  output for checker and code generator
- **Standard library** (v0.2 core): `strings`, `json`, `os`, `http`,
  `sync`, `testing`, `time`, `collections`
- **Pilots**: HTTP CRUD service on SQLite, GUI todo app (GTK4), CLI tools

### Fixed

- Review-loop hardening across the runtime (trapped-frame teardown,
  select lock ordering, cancel races) and checker (diagnostic parity
  corpus — 39 fixtures diffed line-for-line between the seed and
  self-hosted checkers)

## [0.0.1] - 2026-11-04

First tagged state: the v0.1 language foundation.

### Added

- Core language: `fn`/`struct`/`extern`, `let`, `if`/`for`,
  `int`/`bool`/`str`/`cstr`/`err`/`chan` plus user structs
- Ownership: `own T`/`ref T`/`mut T` handles with runtime borrow checks
- Errors: `[v, err]` result returns — a dropped error is a compile error
- Concurrency: `scope`/`spawn`/`go` + `spawnBlocking` structured
  concurrency on worker and blocking pools; channels
- `extern fn` C FFI as the interop boundary
- Lexer and parser producing a full AST; the `oxl` driver binary emits C
  through the system `cc`; runtime arena allocator
- Initial standard library (`strings`, `json`, `os`, `http`, `sync`,
  `testing`, `time`, `collections`) and `examples/` corpus
- Test harness (`tests/run.sh`) and CI on Linux and macOS

[Unreleased]: https://github.com/oxidean/oxl/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/oxidean/oxl/compare/v0.0.1...v0.2.0
[0.0.1]: https://github.com/oxidean/oxl/releases/tag/v0.0.1
