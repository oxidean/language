# S5 Implementation Plan — std/regexp

> Implements `items/s5-std-regexp.md`. Executed natively in-session.

**Goal:** `std/regexp` — a rune-level Thompson NFA engine (Pike VM),
pure .oxl, Go-shaped API, linear-time guarantee by construction.

## Representation decisions (vs the sketch)

- **Rune-level, not byte-level.** `.` and classes operate on decoded
  runes (`std/unicode/utf8`, S3) — Go semantics: `.` is any rune
  except `\n`, `[α-ω]` are code-point ranges, malformed bytes are
  RuneError singletons. Byte-level would split multi-byte runes in
  findall output.
- **Pike VM with ordered thread lists** — the classic RE2 core: two
  lists of (pc, start) pairs in priority order, gen-stamp dedupe by
  pc (earlier start always wins = leftmost), epsilon closure via
  `addthread`. On `match`, record (start,pos) and suppress all
  lower-priority adds — threads already queued still step, so a
  later high-priority match overrides the recorded span.
- **Iterative closure, not recursive.** Explicit []int DFS stack
  (push y then x for splits — pops run x first). Depth is then
  heap-bounded; pattern-length-deep recursion in a{1000}-style
  expansions can't smash the C stack.
- **AST arena before emit.** `[]Node` structs (kind/ch/lo/hi/a/b/
  min/max/lazy) — the language has no tagged unions, and an arena
  avoids box-per-node churn. Emit is a second recursive pass doing
  the textbook Thompson construction with patch-back.
- **`{n,m}` compiles by expansion** — min literal copies then
  max-min sequential quests (language-equivalent to nesting),
  unbounded = min-1 copies + star. Go's 1000-per-repeat cap, plus a
  65536-instruction program cap → `regexp: expression too large`.
- **Lazy quantifiers included** (`*? +? ?? {n,m}?`) — free: swap
  split arm order at emit. Backrefs/lookaround/`(?P<>)`/`\Q..\E`
  rejected at parse with clear errors (Go also rejects backrefs).
- **`\b`/`\B` included** — epsilon assertions evaluated in closure:
  wordchar(prev rune) != wordchar(cur rune). Prev-rune tracked in
  the step loop; cur decoded on demand. `\A`/`\z` alias `^`/`$`
  (text-edge, no multiline flag in v1).
- **POSIX classes included** — `[[:alpha:]]` et al are a ~15-entry
  lookup table expanded at class-parse; the Go corpus uses them.
- **replaceall is literal** — Go's ReplaceAllLiteralString
  semantics (no `$` expansion): v1 records no capture spans, so
  expansion is impossible. Documented; `$` in repl is literal text.
- **Unanchored find = seed-per-position.** A low-priority seed
  thread enters at every byte offset (dedupe by pc keeps the
  earliest start — exactly RE2's leftmost-first). No `.*?` prefix
  compile; the loop seeds directly. `findall` restarts at match
  end, skipping empty matches abutting the previous end (Go's
  abutment rule) and advancing one rune after an accepted empty.

## API

```oxl
struct Re { ins: []Inst, nins: int, cls: []Cls, ncls: int, pat: str }
fn compile(pat: str) -> [Re, err]
fn (r ref Re) matchstr(s: str) -> bool
fn (r ref Re) find(s: str) -> str          // leftmost match or ""
fn (r ref Re) findidx(s: str) -> Span      // {s:-1} when none
fn (r ref Re) findall(s: str, n: int) -> []str   // n<0 = all, Go n
fn (r ref Re) replaceall(s: str, repl: str) -> str
fn (r ref Re) split(s: str, n: int) -> []str     // Go Split n
```

## Syntax subset (RE2-shaped)

literals (UTF-8 runes), `.`, `^`, `$`, `\A`, `\z`, `\b`, `\B`,
`[…]`/`[^…]` classes incl. `-` ranges, escapes, `[[:posix:]]`,
`\d\D\s\S\w\W`, `\n\r\t\f\v\a`, `\xNN`, `\x{N…}`, `\0`-`\377`
octal, `*`,`+`,`?`,`{n}`,`{n,}`,`{n,m}` + lazy `?` suffix,
`(...)`, `(?:...)`, `|`.

## Files

- `std/regexp/regexp.oxl` — everything: parse → NFA compile → VM.
- `tests/regexp_test.oxl` — ported Go-corpus core (~60 asserts:
  literals/anchors/classes/alternation/quantifiers/repeats/lazy/
  utf8/empty-match/abutment + ~15 compile-error cases), plus the
  pathological `(x+x+)+y` on 30 x's (completes trivially — the
  linear-time property), and findall/replaceall/split semantics.
- run.sh `== oxl test: regexp (s5) ==` + `--test` codegen block.
- README std table row, plan.md marker.

## Acceptance (per item)

- Ported Go-corpus core passes under `oxl test`
- Pathological pattern finishes (asserted by the suite's exit)
- Same test file compiles+runs through `bootstrap/cmd/codegen
  --test` + cc — exercises slices/structs/fn-free engine end to end
