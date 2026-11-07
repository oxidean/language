# S3 Implementation Plan — util & data packages

> Implements `items/s3-std-util.md`. Executed natively in-session.

**Goal:** the core util surface — `path`/`filepath`, `cmp`/`sort`/
`slices`/`maps`, `math`/`math/rand`, `unicode/utf8`, `flag`, `log` —
all pure .oxl over existing generics, fn values, ifaces, and a small
set of new `rt_*` externs.

## Representation decisions (vs the sketch)

- **`cmp.Ordered` wants `str` — and `str < str` doesn't exist.** The
  comparison arm assigns `<` to Int operands; `Str` never reaches it.
  `cmp.Ordered` covers strings, and sorting `[]str` is the single
  most common sort. The fix is small and follows the S2 pattern:
  `rt_str_cmp(a,b)` (memcmp over the shared prefix, len tiebreak —
  byte-wise lexicographic order), `check` allows `str op str`
  under Lt/Le/Gt/Ge, `codegen` emits `rt_str_cmp(l, r) <op> 0`.
  `==`/`!=` keep the existing byte-compare emit — no semantic change.

- **Constraint tables are widened to match the concrete language.**
  `op_ok`/`satisfies` today accept `ordered: Int` only — `float` can't
  satisfy `ordered` even though `x < y` on floats compiles, and `add`
  omits `Float`/`Byte` though `f + f` compiles. Widen both checkers
  identically: `ordered += Float, Byte, Str`; `eq += Float, Byte, Err`;
  `add += Float, Byte`. Widening-only — no new diagnostics, corpus
  stays byte-identical. (A new positive corpus row proves the widening;
  existing constraint negatives still fail.)

- **`own []T` completes in the type grammar — the escape hatch the
  checker's own diag recommends.** `new(slice)` already produces
  `own []T` (`rt_slice_own` heap-copies), but `starts_ty`/`p_ty`
  rejected `[` after `own`/`ref`/`mut`, so the type couldn't be
  *declared* anywhere — `slices.clone`-style producers had no
  returning shape. Adding `[` to the type-start set is one token
  each side. Follow-on: `own []T` → `[]T` read-coercion in
  `assignable`+`bind_vars` (`sort(clone(s))` composes; the view is
  scope-bounded like every slice) and the `(*v)` emit at call args
  — builtins like `len(os)`/`os[i]` already accepted own slices,
  this just lets user fns do the same.

- **`maps` is concrete, not generic.** `collections.Map` is `str→int`
  — a generic `maps[K,V]` can't exist until the container does. The
  package ships `keys`/`values`/`merge`/`equal`/`has`/`keys_sorted`
  over the concrete type; doc notes the bound.

- **`rand` is pure .oxl on a mutable global** — top-level `let` globals
  are assignable (verified). xorshift64* written fresh (not ported):
  state `int`, `x ^= x>>12; x ^= x<<25; x ^= x>>27; return x * K` —
  passes the same acceptance bar without borrowing the lagged-Fibonacci
design
  machinery. `Rand` struct for explicit-source users; top-level fns
  mirror `math/rand`'s global-source API.

- **`math` rides `rt_m_*` wrappers + the sqlite-style link gate.**
  Direct `extern fn sqrt` would emit a bare `sqrt` call — nothing to
  grep for `-lm` gating, and proto name could shadow. `rt_m_*` wrappers
  in rt.c (one line each over `math.h`) keep the `rt_` prefix so
  `needs_math = c_src.contains("rt_m_")` mirrors `needs_sqlite`. `-lm`
  POSIX-only (msvcrt/mingw carry libm) — same treatment as `-pthread`.

- **`flag` can't hand back pointers** — binding a `*T` at
  registration. oxl shape: `fs.str/num/bool` register name+default+
  usage; `fs.parse(args)` fills internal state; typed getters
  `get/geti/getb`, `rest()` for positionals. Internally a small
  slice-backed string map (`collections.Map` is str→int — can't hold
  str values). Parse errors are `errors.new` with familiar messages;
  caller decides exit.

- **`log` can't be `log.err`** — `err` is a type keyword. Levels are
  `debug/info/warn/error/fatal` (fatal logs + `os.quit(1)`). Output is
  `YYYY-MM-DD HH:MM:SS LEVEL msg` to `os.stderr()` — needs one extern,
  `rt_timestr()` (localtime veneer: `localtime_r` ⇄ `localtime_s`),
  surfaced as `time.stamp()` for reuse. `setts(false)` strips the
  timestamp for deterministic asserts.

- **`filepath` learns the OS from `rt_is_windows()`** — a compile-time
  const under `_WIN32`, matching how the E3 veneer decides everything
  else. oxl-canonical paths are `/`-separated (what `rt_fullpath` and
  every fs builtin already produce/accept); `\` is a separator on
  Windows only. `abs` wraps `rt_fullpath`.

## Runtime additions (rt.c/rt.h)

```c
int64_t rt_str_cmp(OxlStr a, OxlStr b);   /* byte-lex order           */
int64_t rt_is_windows(void);              /* 1 under _WIN32           */
OxlStr  rt_timestr(void);                 /* "YYYY-MM-DD HH:MM:SS"    */
double  rt_m_sqrt/pow/exp/log/log10/floor/ceil/trunc/round/fmod(x,y)…
double  rt_m_sin/cos/tan/asin/acos/atan/atan2/hypot…
double  rt_m_nan(void);  double rt_m_inf(int64_t sign);
int64_t rt_m_isnan(double); int64_t rt_m_isinf(double);
```

math wrappers sit under `#include <math.h>` — unguarded (they're one
line each; `--gc-sections` drops them when uncalled).

## Language changes (×2 toolchains)

| Change | Seed | Self-hosted |
|---|---|---|
| `str op str` under Lt/Le/Gt/Ge | check.rs | check.oxl |
| `rt_str_cmp` emit | codegen.rs | codegen.oxl |
| `op_ok` widening (ordered/eq/add) | check.rs | check.oxl |
| `-lm` gate on `rt_m_` | main.rs `emit_and_cc` | run.sh cgen lines |

## Packages

- `std/cmp` — `iface Ordered { ordered }`; `compare/min/max[T Ordered]`.
- `std/sort` — `sort[T Ordered]` (median-of-3 quicksort + insertion
  cutoff ≤16), `sortby[T any](s, less)`, `issorted[T Ordered]`,
  `issortedby`, `search(n, f)` (binary search by predicate).
- `std/slices` — `contains`/`index`/`lastindex`/`equal[T eq]`,
  `clone`/`concat`/`reverse`/`insert`/`delete[T any]`,
  `min`/`max`/`binarysearch[T Ordered]`, `sorted`/`sortby` convenience.
- `std/maps` — `keys`/`values`/`has`/`merge`/`equal`/`keys_sorted`
  over `collections.Map` (str→int concrete).
- `std/path` — `join/dir/base/ext/clean/isabs/match` (`*`,`?`,`[set]`
  glob subset) — slash-only path semantics.
- `std/path/filepath` — `sep/listsep/volume_name/join/dir/base/ext/
  clean/isabs/abs/toslash/fromslash` — OS-aware via `rt_is_windows`;
  `abs` = `rt_fullpath`.
- `std/math` — wrappers on `rt_m_*` + `pi/e/sqrt2/ln2/ln10` consts +
  `abs(i)` int helper.
- `std/math/rand` — `Rand{state}` + `new(seed)` + methods
  `intn/int/floatn`; top-level `seed/intn/int/floatn` on the global
  source; `shuffle[T any](r mut Rand, s)` — generic methods verified at
  impl time, else pkg fn form.
- `std/unicode/utf8` — `len/decode/valid/fullrune/encode/runeerror`
  (invalid input decodes as replacement rune 0xFFFD, size 1).
- `std/flag` — `FlagSet` + `new(usage)` + `str/num/bool`/`parse`/
  `get/geti/getb/rest`; `-x=v`,`-x v`,`--x`,`-x`(bool),`--` terminator;
  `-h`/`-help` → `flag: help requested` err.
- `std/log` — `setlevel/setprefix/setts` + `debug/info/warn/error/
  fatal`; `YYYY-MM-DD HH:MM:SS LEVEL msg` via `os.stderr()`.

## Call-site + test wiring

- `examples/cli` → flag consumer: `cli <name> [-n=N]` positional name.
- `tests/util_test.oxl` — test fns per package (path/cmp/sort/slices/
  maps/math/rand/utf8); `tests/flaglog_test.oxl` — flag parse cases +
  log level/quiet asserts (stderr checked via run.sh redirect).
- `filepath` branches assertions on `filepath.sep()` — one file proves
  both OSes across the CI matrix.
- run.sh: `== oxl test: util (s3) ==` section + cli/flag args cases +
  log shape grep.
- README std table + plan.md status line.

## Acceptance (per item)

- generic fns instantiate and run through both codegens — sort/slices
  tests run under `oxl test` AND the codegen-cc section
- filepath on Windows CI green (same file, both semantic sets)
- `cli -name=x -n 3` / `--n` / bad-flag→usage paths in run.sh
- utf8 edge cases: surrogates, truncated sequences, 4-byte, overlongs
- log shape asserted (`setts(false)` → `LEVEL msg`)
