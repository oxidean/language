#!/usr/bin/env bash
# tests/run.sh <path-to-oxl-binary> — end-to-end suite run by CI.
set -euo pipefail
# trap-path tests abort on purpose; without this each run litters the
# repo root with core.* dumps (they're gitignored but still junk)
ulimit -c 0 2>/dev/null || true
OXL="${1:-oxl}"
cd "$(dirname "$0")/.."
# absolutize — subshell gates (gui pilot) invoke $OXL from other dirs
case "$OXL" in /*) ;; *) OXL="$PWD/$OXL";; esac

# platform knobs — Git Bash/MSYS (windows-latest legs) spell .exe
# explicitly, drop -pthread (pthread APIs come from the rt.h Win32
# veneer, not a lib), link ws2_32 for the socket shims, and resolve
# python by whatever name the image gives it
EXE=""; THREAD_FLAG="-pthread"; NET_LIBS=""; PY=python3
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        EXE=".exe"; THREAD_FLAG=""; NET_LIBS="-lws2_32"; PY=python ;;
esac
command -v "$PY" >/dev/null 2>&1 || PY=python3
CC="${LANG_CC:-cc}"
# sqlite reaches a build either as a system lib (POSIX dev packages) or
# as an amalgamation source (LANG_SQLITE_SRC — the only form a stock
# Windows box provides); -I its dir supplies sqlite3.h
if [ -n "${LANG_SQLITE_SRC:-}" ]; then
    SQLITE_LINK="-I$(dirname "$LANG_SQLITE_SRC") $LANG_SQLITE_SRC"
else
    SQLITE_LINK="-lsqlite3"
fi

# peak RSS of pid $1 in kB — ps reads /proc, which doesn't exist for a
# native child on Windows; ask the OS instead
rss_kb() {
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*)
            # git-bash tracks native children by msys pid (not the winpid)
            # and exposes their WorkingSet in /proc — no powershell hop
            # (powershell.exe is not on the msys PATH anyway)
            awk '/VmRSS/{printf "%d", $2}' "/proc/$1/status" 2>/dev/null ;;
        *) ps -o rss= -p "$1" 2>/dev/null | tr -d ' ' ;;
    esac
}

# GNU timeout doesn't exist on stock macOS; fall back to gtimeout
# (brew coreutils), else a bash watchdog that kills the child at the
# deadline — a hang must never run unbounded.
with_timeout() {
    if command -v timeout >/dev/null 2>&1; then timeout "$@"
    elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$@"
    else
        local secs=$1; shift
        "$@" & local pid=$!
        # watchdog redirects away from the caller's capture pipe: inside
        # out=$(with_timeout ..) the subshell would inherit that pipe, and
        # a killed dog can orphan its sleep child still holding it open —
        # the $() then hangs past the deadline it was enforcing
        ( sleep "$secs"; kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 & local dog=$!
        local rc=0
        wait "$pid" 2>/dev/null || rc=$?
        if kill -0 "$dog" 2>/dev/null; then
            kill "$dog" 2>/dev/null; wait "$dog" 2>/dev/null
        else
            rc=124 # watchdog fired — same sentinel as GNU timeout
        fi
        return "$rc"
    fi
}

# one run-owned scratch dir: fixed /tmp paths collide between parallel
# runs (CWE-377), and an EXIT trap guarantees the http pilot's server
# dies even when a probe fails mid-suite
scratch=$(mktemp -d)
srv_pid=""
cleanup() {
    if [ -n "$srv_pid" ]; then kill "$srv_pid" 2>/dev/null || true; fi
    # v2's sealed module-cache slots are read-only — restore write
    # permission or rm leaves them behind
    chmod -R u+w "$scratch" 2>/dev/null
    rm -rf "$scratch"
}
trap cleanup EXIT

echo "== hello =="
out=$("$OXL" run examples/hello.oxl)
[ "$out" = "hello, world" ] || { echo "FAIL hello: $out"; exit 1; }
# G2: informational binary size — section GC + feature gates should
# keep hello tiny; printed like the a1 memory timings, no hard assert
"$OXL" build examples/hello.oxl -o "$scratch/hello$EXE" || { echo "FAIL hello build"; exit 1; }
echo "  hello.oxl binary: $(wc -c <"$scratch/hello$EXE") bytes"

echo "== features =="
out=$("$OXL" run examples/features.oxl)
[ "$(tail -1 <<<"$out")" = "done" ] || { echo "FAIL features"; exit 1; }

echo "== cli pilot =="
out=$("$OXL" run examples/cli/main.oxl -- Jesse)
grep -q "hello, Jesse" <<<"$out" || { echo "FAIL cli"; exit 1; }

echo "== oxl test: smoke =="
out=$("$OXL" test examples/smoke_test.oxl)
printf '%s\n' "$out" >&2
grep -q "test_chan ... ok" <<<"$out" || { echo "FAIL smoke: $out"; exit 1; }

echo "== oxl test: stdlib =="
out=$("$OXL" test tests/stdlib_test.oxl)
grep -q "test_loops ... ok" <<<"$out" || { echo "FAIL stdlib: $out"; exit 1; }

echo "== oxl test: os =="
out=$("$OXL" test tests/os_test.oxl)
grep -q "test_file ... ok" <<<"$out" || { echo "FAIL os: $out"; exit 1; }

echo "== oxl test: io (s2) =="
out=$("$OXL" test tests/io_test.oxl)
printf '%s
' "$out" >&2
for t in test_bytes test_strconv test_fmt test_errs test_stdstreams test_io_files; do
    grep -q "$t ... ok" <<<"$out" || { echo "FAIL io: $out"; exit 1; }
done
# char-literal lex errors — '' and multi-char are rejected at lex time
cat > "$scratch/badchar.oxl" <<'EOF'
package main
fn main() { x := '' }
EOF
diag=$("$OXL" build "$scratch/badchar.oxl" -o "$scratch/badchar$EXE" 2>&1 || true)
grep -q "char literal" <<<"$diag" || { echo "FAIL charlit diag: $diag"; exit 1; }

echo "== stdmods (json/sync/time) =="
out=$("$OXL" run tests/stdmods_test.oxl)
grep -q '"name":"widget"' <<<"$out" || { echo "FAIL stdmods json: $out"; exit 1; }
grep -q "^4000$" <<<"$out" || { echo "FAIL stdmods sync counter: $out"; exit 1; }
[ "$(tail -1 <<<"$out")" = "done" ] || { echo "FAIL stdmods time: $out"; exit 1; }

echo "== float (d1) =="
out=$("$OXL" test tests/float_test.oxl)
grep -q "test_json_float ... ok" <<<"$out" || { echo "FAIL float: $out"; exit 1; }
# int/float mixing must be a compile error, not silent widening
cat > "$scratch/floatmix.oxl" <<'EOF'
package main
fn main() { x := 1 + 2.5 }
EOF
if "$OXL" build "$scratch/floatmix.oxl" -o "$scratch/floatmix$EXE" 2>/dev/null; then
    echo "FAIL: int+float compiled (should be a type error)"; exit 1
fi

echo "== enum (b1) =="
out=$("$OXL" run tests/enum_test.oxl)
[ "$(tail -1 <<<"$out")" = "enum fns ok" ] || { echo "FAIL enum: $out"; exit 1; }

echo "== match (b2) =="
out=$("$OXL" run tests/match.oxl)
[ "$(tail -1 <<<"$out")" = "match ok" ] || { echo "FAIL match: $out"; exit 1; }
# non-exhaustive match must name the missing variant
cat > "$scratch/matchnex.oxl" <<'EOF'
package main
enum E { A(int), B, }
fn f(e: E) -> int { return match e { E.A(v) => v } }
fn main() { }
EOF
if "$OXL" build "$scratch/matchnex.oxl" -o "$scratch/matchnex$EXE" 2>/dev/null; then
    echo "FAIL: non-exhaustive match compiled"; exit 1
fi
diag=$("$OXL" build "$scratch/matchnex.oxl" -o "$scratch/matchnex$EXE" 2>&1 || true)
grep -q "missing B" <<<"$diag" || { echo "FAIL: missing-variant diagnostic: $diag"; exit 1; }

echo "== slices (b3) =="
out=$("$OXL" run tests/slices.oxl)
[ "$(tail -1 <<<"$out")" = "slices ok" ] || { echo "FAIL slices: $out"; exit 1; }
# out-of-range index must trap, not corrupt — build first so a compile
# break can't pass as a runtime trap
"$OXL" build tests/slice_oob.oxl -o "$scratch/slice_oob$EXE" || { echo "FAIL: slice_oob did not compile"; exit 1; }
if "$scratch/slice_oob$EXE" >/dev/null 2>&1; then
    echo "FAIL slice_oob: expected nonzero exit"; exit 1
fi
# inverted view range must trap, not corrupt
"$OXL" build tests/slice_range.oxl -o "$scratch/slice_range$EXE" || { echo "FAIL: slice_range did not compile"; exit 1; }
if "$scratch/slice_range$EXE" >/dev/null 2>&1; then
    echo "FAIL slice_range: expected nonzero exit"; exit 1
fi

echo "== fn values (b4) =="
out=$("$OXL" run tests/fnval.oxl)
[ "$(tail -1 <<<"$out")" = "fnval ok" ] || { echo "FAIL fnval: $out"; exit 1; }
# fn values can't suspend, capture susp, compare, or wrap generics/pairs
for bad in susp clsusp ctrans gen pair eq; do
    cat > "$scratch/fnv_$bad.oxl" <<EOF
package main
$(case $bad in
  susp)   printf 'fn s() { sleep(1) }\nfn main() { f := s }' ;;
  clsusp) printf 'fn main() { f := { |x: int| sleep(1); return x } }' ;;
  ctrans) printf 'fn w() { sleep(1) }\nfn main() { f := { |x: int| w(); return x } }' ;;
  gen)    printf 'fn id[T any](x: T) -> T { return x }\nfn main() { f := id[int] }' ;;
  pair)   printf 'fn p() -> [int, err] { return 1, nil }\nfn main() { f := p }' ;;
  eq)     printf 'fn a() {}\nfn main() { f := a; if f == a { println(1) } }' ;;
esac)
EOF
    if "$OXL" build "$scratch/fnv_$bad.oxl" -o "$scratch/fnv_$bad" 2>/dev/null; then
        echo "FAIL: fnval/$bad compiled (should be rejected)"; exit 1
    fi
done

echo "== collections (b6) =="
out=$("$OXL" run tests/collections_test.oxl)
[ "$(tail -1 <<<"$out")" = "collections ok" ] || { echo "FAIL collections: $out"; exit 1; }

echo "== defer (b5) =="
out=$("$OXL" run tests/defer_order.oxl)
grep -q "defer order ok" <<<"$out" || { echo "FAIL defer order: $out"; exit 1; }
out=$("$OXL" run tests/defer_test.oxl)
# fixed sequence: LIFO drain (3 2 1), error-path defer (9), per-iteration
# loop defers (2 1 0), the untaken branch registering nothing (7 only)
[ "$out" = $'3\n2\n1\n9\n2\n1\n0\n7\ndone' ] || { echo "FAIL defer: $out"; exit 1; }
# trap unwind still drains the task's defer stack before aborting
cat > "$scratch/d5trap.oxl" <<'EOF'
package main
fn dmark(x: int) { println(itoa(x)) }
fn willtrap() {
    defer dmark(6)
    defer dmark(5)
    s := mkslice(int, 2)
    println(s[9])
}
fn main() { willtrap() }
EOF
"$OXL" build "$scratch/d5trap.oxl" -o "$scratch/d5trap$EXE" || { echo "FAIL: d5trap did not compile"; exit 1; }
trapout=$(with_timeout 10 "$scratch/d5trap$EXE" 2>&1 || true)
grep -q "^5$" <<<"$trapout" || { echo "FAIL defer trap drain: $trapout"; exit 1; }
grep -q "^6$" <<<"$trapout" || { echo "FAIL defer trap drain order: $trapout"; exit 1; }

echo "== methods + interfaces (d3) =="
out=$("$OXL" run tests/methods.oxl)
[ "$out" = "methods ok" ] || { echo "FAIL methods: $out"; exit 1; }
# a method-less type must not satisfy a method interface
cat > "$scratch/d3sat.oxl" <<'D3EOF'
package main
interface Writer { write(s: str) -> int }
struct Bare { n: int }
fn emit(w: Writer) -> int { return w.write("x") }
fn main() { emit(Bare{n: 1}) }
D3EOF
if "$OXL" build "$scratch/d3sat.oxl" -o "$scratch/d3sat" 2>/dev/null; then
    echo "FAIL: unsatisfied iface compiled"; exit 1
fi
# methods can't suspend — vtable calls carry no task frame
cat > "$scratch/d3susp.oxl" <<'D3EOF'
package main
struct S { n: int }
fn (s S) wait() { sleep(1) }
fn main() { s := S{n: 0}; s.wait() }
D3EOF
if "$OXL" build "$scratch/d3susp.oxl" -o "$scratch/d3susp" 2>/dev/null; then
    echo "FAIL: suspending method compiled"; exit 1
fi

echo "== move tracking (d2) =="
# use-after-move must be a compile error naming the binding
cat > "$scratch/d2move.oxl" <<'EOF'
package main
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); take(p); take(p) }
EOF
if "$OXL" build "$scratch/d2move.oxl" -o "$scratch/d2move" 2>/dev/null; then
    echo "FAIL: use-after-move compiled"; exit 1
fi
diag=$("$OXL" build "$scratch/d2move.oxl" -o "$scratch/d2move" 2>&1 || true)
grep -q 'use of moved value `p`' <<<"$diag" || { echo "FAIL moved diag: $diag"; exit 1; }
# the headline bug class — one own cell into two spawned workers
cat > "$scratch/d2spawn.oxl" <<'EOF'
package main
struct P { v: int }
fn w(p: own P) { }
fn main() { p := new(P); spawn w(p); spawn w(p) }
EOF
if "$OXL" build "$scratch/d2spawn.oxl" -o "$scratch/d2spawn" 2>/dev/null; then
    echo "FAIL: double-spawn of own compiled"; exit 1
fi
# moved inside a loop body — the next iteration would read a moved value
cat > "$scratch/d2for.oxl" <<'EOF'
package main
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); for true { take(p) } }
EOF
if "$OXL" build "$scratch/d2for.oxl" -o "$scratch/d2for" 2>/dev/null; then
    echo "FAIL: move-in-loop compiled"; exit 1
fi
# re-init clears the mark
cat > "$scratch/d2ok.oxl" <<'EOF'
package main
struct P { v: int }
fn take(p: own P) { }
fn main() {
    p := new(P)
    take(p)
    p = new(P)
    take(p)
    println("ok")
}
EOF
out=$("$OXL" run "$scratch/d2ok.oxl")
[ "$out" = "ok" ] || { echo "FAIL d2 reinit: $out"; exit 1; }
# moved in one branch: the join can't prove which path ran — reject the
# later read unless EVERY path re-initializes the binding
cat > "$scratch/d2branch.oxl" <<'EOF'
package main
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); if len("x") > 0 { take(p) }; take(p) }
EOF
if "$OXL" build "$scratch/d2branch.oxl" -o "$scratch/d2branch" 2>/dev/null; then
    echo "FAIL: one-branch move compiled"; exit 1
fi
# shared cells are copy/shareable — same cell to four workers is intended
out=$("$OXL" run tests/stdmods_test.oxl)
grep -q "^4000$" <<<"$out" || { echo "FAIL d2 shared workers: $out"; exit 1; }

echo "== d5 module qualification =="
out=$("$OXL" run tests/d5_qual.oxl)
[ "$(tail -1 <<<"$out")" = "d5 qual ok" ] || { echo "FAIL d5 qual: $out"; exit 1; }
# an imported name with no qualifier and no selective flatten must not
# resolve — silent shadowing was the bug d5 exists to kill
cat > "$scratch/d5unqual.oxl" <<'EOF'
package main
import "std/strings"
fn main() { println(strings.startswith("ab","a")); println(trim(" x ")) }
EOF
if "$OXL" build "$scratch/d5unqual.oxl" -o "$scratch/d5unqual" 2>/dev/null; then
    echo "FAIL: unqualified import call compiled"; exit 1
fi
diag=$("$OXL" build "$scratch/d5unqual.oxl" -o "$scratch/d5unqual" 2>&1 || true)
# an unresolved import emits "undefined `trim`" too — without it the
# probe tests nothing. Fail on resolution diags so the check really
# exercises qualification, not import fallback.
if grep -q "cannot resolve import" <<<"$diag"; then
    echo "FAIL unqual probe: stdlib unresolved: $diag"; exit 1
fi
grep -q "undefined \`trim\`" <<<"$diag" || { echo "FAIL unqual diag: $diag"; exit 1; }

echo "== bootstrap lexer =="
out=$("$OXL" run bootstrap/cmd/lex -- examples/hello.oxl)
grep -q "13 tokens" <<<"$out" || { echo "FAIL lexer"; exit 1; }

echo "== bootstrap parser =="
out=$("$OXL" run bootstrap/cmd/parse -- examples/features.oxl)
printf '%s' "$out" | $PY -c 'import json,sys
d=json.load(sys.stdin)
assert len(d["items"])==7 and not any("skip" in i for i in d["items"]), d' || { echo "FAIL parser"; exit 1; }
out=$("$OXL" run bootstrap/cmd/parse -- bootstrap/parser/parser.oxl)
# stdin not argv for the payload — a self-parse AST is far past the
# ~32KB command-line cap Windows puts on every child process
printf '%s' "$out" | $PY -c 'import json,sys
d=json.load(sys.stdin)
assert len(d["items"])>20 and not any("skip" in i for i in d["items"]), d' || { echo "FAIL parser self-parse"; exit 1; }

echo "== bootstrap checker =="
# the self-hosted typechecker must check itself and representative sources
for f in bootstrap/check examples/features.oxl tests/match.oxl std/collections; do
    out=$("$OXL" run bootstrap/cmd/check -- "$f")
    [ "$out" = "ok" ] || { echo "FAIL checker $f: $out"; exit 1; }
done

echo "== differential diagnostics (c1) =="
# tests/diag/ holds files that parse cleanly in both pipelines but fail
# checking — the Rust checker's diagnostics must equal the self-hosted
# one's, line for line and in order. The Rust driver prefixes each diag
# `file:line:col:` (stripped here); check.oxl reports {kind,msg} only.
# Out of corpus scope by design: parse/lex failures (the [parse]/[lex]
# kinds don't surface through check.oxl's diag list) and cases neither
# checker diagnoses (cc or the runtime catches those downstream — that
# silence is already parity). The self side runs a prebuilt binary:
# `oxl run` would recompile check.oxl's C per fixture, and clang's
# -Wparentheses-equality warnings on the generated code (macOS cc) land
# in the captured output and break the diff.
"$OXL" build bootstrap/cmd/check -o "$scratch/selfcheck$EXE"     || { echo "FAIL selfcheck build"; exit 1; }
for f in tests/diag/*.oxl; do
    # both checkers must reject every fixture (pipefail carries the
    # build's nonzero status through the sed) — and their diagnostics
    # must be identical. A fixture either side accepts is a test bug.
    if rust=$("$OXL" build "$f" -o "$scratch/diagbin" 2>&1 | sed 's/^[^:]*:[0-9][0-9]*:[0-9][0-9]*: //'); then
        echo "FAIL diag parity $f: rust checker accepted"; exit 1
    fi
    if self=$("$scratch/selfcheck$EXE" "$f" 2>&1); then
        echo "FAIL diag parity $f: self checker accepted"; exit 1
    fi
    if [ "$rust" != "$self" ]; then
        echo "FAIL diag parity $f"
        echo "-- rust --"; printf '%s\n' "$rust"
        echo "-- self --"; printf '%s\n' "$self"
        exit 1
    fi
    # a corpus file neither side diagnoses is a test bug, not parity
    if [ -z "$rust" ] || [ "$self" = "ok" ]; then
        echo "FAIL diag corpus $f: no diagnostics from either checker"; exit 1
    fi
done

echo "== module resolution (m2) =="
# tests/mod/ holds self-contained module trees; each carries its own
# oxl.mod so root discovery never escapes into the repo's module.
# `expected` marks the outcome — `ok` fixtures must build in Rust and
# print `ok` in the self-hosted checker; error fixtures must be
# rejected by both checkers with byte-identical diagnostics. The
# module cache is pointed at an empty dir so `missing module` is
# deterministic; `tests/mod/widget` and `tests/mod/intmod` are shared
# replace-targets, not cases.
mkdir -p "$scratch/oxlmodcache"
export OXLMODCACHE="$scratch/oxlmodcache"
for f in tests/mod/*/; do
    case "$f" in tests/mod/widget/|tests/mod/intmod/) continue ;; esac
    rust_out=$("$OXL" build "$f" -o "$scratch/modbin" 2>&1 | sed 's/^[^:]*:[0-9][0-9]*:[0-9][0-9]*: //') && rust_rc=0 || rust_rc=1
    self_out=$("$scratch/selfcheck$EXE" "$f" 2>&1) && self_rc=0 || self_rc=1
    if [ "$(cat "$f/expected")" = ok ]; then
        if [ "$rust_rc" != 0 ]; then
            echo "FAIL module $f: rust checker rejected"; printf '%s\n' "$rust_out"; exit 1
        fi
        if [ "$self_rc" != 0 ] || [ "$self_out" != ok ]; then
            echo "FAIL module $f: self checker rejected"; printf '%s\n' "$self_out"; exit 1
        fi
    else
        if [ "$rust_rc" = 0 ]; then
            echo "FAIL module $f: rust checker accepted"; exit 1
        fi
        if [ "$self_rc" = 0 ]; then
            echo "FAIL module $f: self checker accepted"; exit 1
        fi
        if [ "$rust_out" != "$self_out" ]; then
            echo "FAIL module parity $f"
            echo "-- rust --"; printf '%s\n' "$rust_out"
            echo "-- self --"; printf '%s\n' "$self_out"
            exit 1
        fi
        if [ -z "$rust_out" ]; then
            echo "FAIL module corpus $f: no diagnostics from either checker"; exit 1
        fi
    fi
done
# module-off: a tree with no oxl.mod ancestor resolves std only —
# the local import must fail identically on both checkers.
mkdir -p "$scratch/modoff"
printf 'package main\n\nimport "localpkg"\n\nfn main() {}\n' > "$scratch/modoff/main.oxl"
rust_out=$("$OXL" build "$scratch/modoff" -o "$scratch/modbin" 2>&1 | sed 's/^[^:]*:[0-9][0-9]*:[0-9][0-9]*: //') && rust_rc=0 || rust_rc=1
self_out=$("$scratch/selfcheck$EXE" "$scratch/modoff" 2>&1) && self_rc=0 || self_rc=1
if [ "$rust_rc" = 0 ] || [ "$self_rc" = 0 ] || [ "$rust_out" != "$self_out" ]; then
    echo "FAIL module-off parity"
    echo "-- rust --"; printf '%s\n' "$rust_out"
    echo "-- self --"; printf '%s\n' "$self_out"
    exit 1
fi
unset OXLMODCACHE

echo "== v1 build list (oxl list -m all) =="
# MVS: depb wins at v1.2.0 (depc's edge), versioned replace supplies
# the dir; depb@v1.0.0 is never fetched and needn't exist. Point the
# cache at scratch so nothing in the user cache can leak a depb
# manifest (or any edge) into selection.
export OXLMODCACHE="$scratch/modcache-empty"
( cd tests/mod/mvs && "$OXL" list -m all ) > "$scratch/mvlist" 2>&1 || { echo "FAIL oxl list rc"; cat "$scratch/mvlist"; exit 1; }
grep -qx 'mvstest' "$scratch/mvlist" || { echo "FAIL list main"; cat "$scratch/mvlist"; exit 1; }
grep -q 'depb v1\.2\.0 => ' "$scratch/mvlist" || { echo "FAIL list depb"; cat "$scratch/mvlist"; exit 1; }
grep -q 'depc v1\.0\.0 => ' "$scratch/mvlist" || { echo "FAIL list depc"; cat "$scratch/mvlist"; exit 1; }

echo "== v2 module cache + oxl get =="
# `oxl get` fetches through git into a sealed, content-addressed slot
# and pins both h1 lines in oxl.sum; every build then verifies the
# cache against those sums. file:// module paths hold ':' which can't
# form a Windows-legal slot name, so the fetch fixture is POSIX-only —
# the verify path itself is platform-clean and covered above.
case "$(uname -s)" in
MINGW*|MSYS*|CYGWIN*) ;;
*)
getwork="$scratch/getwork"
mkdir -p "$getwork/dep/util" "$getwork/app/app" "$getwork/cache"
# upstream module repo: v1.0.0 now, v1.1.0 tagged mid-test
( cd "$getwork/dep" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/dep\noxl 0.3\n' "$getwork" > oxl.mod
  printf 'package util\nfn answer() -> int { return 42 }\n' > util/u.oxl
  printf 'root readme\n' > README && printf 'ext readme\n' > README.md
  git add -A && git commit -qm init && git tag v1.0.0 )
printf 'module local/app\noxl 0.3\n' > "$getwork/app/oxl.mod"
printf 'package main\nimport "file://%s/dep/util"\n\nfn main() { println("answer=" + itoa(util.answer())) }\n' "$getwork" > "$getwork/app/app/main.oxl"
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" get "file://$getwork/dep@v1.0.0" ) > "$getwork/get1" 2>&1 || { echo "FAIL get rc"; cat "$getwork/get1"; exit 1; }
grep -q 'added file://' "$getwork/get1" || { echo "FAIL get report"; cat "$getwork/get1"; exit 1; }
grep -q "require file://$getwork/dep v1.0.0" "$getwork/app/oxl.mod" || { echo "FAIL get require"; cat "$getwork/app/oxl.mod"; exit 1; }
[ "$(grep -c ' h1:' "$getwork/app/oxl.sum")" = 2 ] || { echo "FAIL get sum"; cat "$getwork/app/oxl.sum"; exit 1; }
# slot sealed read-only — find the escaped slot (scratch paths may hold
# case-folding chars, which `!`-escape in the slot name)
slot=$(find "$getwork/cache" -name '*@v1.0.0' -type d)
[ -n "$slot" ] || { echo "FAIL cache slot missing"; exit 1; }
touch "$slot/probe" 2>/dev/null && { echo "FAIL cache not read-only"; exit 1; }
# resolve + run purely from cache — dep repo moved away (network "off")
mv "$getwork/dep" "$getwork/dep-gone"
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" build app -o "$getwork/appbin" ) || { echo "FAIL offline build"; exit 1; }
[ "$("$getwork/appbin")" = "answer=42" ] || { echo "FAIL offline run"; "$getwork/appbin"; exit 1; }
mv "$getwork/dep-gone" "$getwork/dep"
# tamper a sealed file -> checksum mismatch on BOTH checkers
depf=$(find "$getwork/cache" -name u.oxl -path '*/util/*')
[ -n "$depf" ] || { echo "FAIL cached source missing"; exit 1; }
chmod u+w "$depf" && printf '// tampered\n' >> "$depf" && chmod a-w "$depf"
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" build app -o /dev/null ) > "$getwork/tamp.rust" 2>&1 && { echo "FAIL tamper accepted (rust)"; exit 1; }
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$scratch/selfcheck$EXE" app ) > "$getwork/tamp.self" 2>&1 && { echo "FAIL tamper accepted (self)"; exit 1; }
grep -q 'checksum mismatch' "$getwork/tamp.rust" || { echo "FAIL tamper text"; cat "$getwork/tamp.rust"; exit 1; }
diff "$getwork/tamp.rust" "$getwork/tamp.self" >/dev/null || { echo "FAIL tamper parity"; cat "$getwork/tamp.rust" "$getwork/tamp.self"; exit 1; }
# restore: re-get into a fresh cache, then @latest upgrade bumps require+sum
chmod -R u+w "$getwork/cache" 2>/dev/null; rm -rf "$getwork/cache" && mkdir -p "$getwork/cache"
( cd "$getwork/dep" && printf 'package util\nfn answer() -> int { return 43 }\n' > util/u.oxl && git add -A && git commit -qm bump && git tag v1.1.0 )
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" get "file://$getwork/dep@v1.0.0" >/dev/null && OXLMODCACHE="$getwork/cache" "$OXL" get "file://$getwork/dep@latest" ) > "$getwork/get2" 2>&1 || { echo "FAIL latest rc"; cat "$getwork/get2"; exit 1; }
grep -q 'upgraded file://' "$getwork/get2" || { echo "FAIL upgrade report"; cat "$getwork/get2"; exit 1; }
grep -q "require file://$getwork/dep v1.1.0" "$getwork/app/oxl.mod" || { echo "FAIL upgrade require"; cat "$getwork/app/oxl.mod"; exit 1; }
# v1.0.0's sum lines survive the upgrade — another module may still
# select it via MVS and needs them to verify (4 lines: 2 per version)
[ "$(grep -c ' h1:' "$getwork/app/oxl.sum")" = 4 ] || { echo "FAIL upgrade sum"; cat "$getwork/app/oxl.sum"; exit 1; }
# re-get of the same version hits the sealed slot — validated against
# the archive and reported unchanged
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" get "file://$getwork/dep@v1.1.0" ) > "$getwork/get3" 2>&1 || { echo "FAIL reget rc"; cat "$getwork/get3"; exit 1; }
grep -q 'unchanged file://' "$getwork/get3" || { echo "FAIL reget report"; cat "$getwork/get3"; exit 1; }
# missing oxl.sum -> missing-entry error (Go wording), byte-identical
rm "$getwork/app/oxl.sum"
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$OXL" build app -o /dev/null ) > "$getwork/nosum.rust" 2>&1 && { echo "FAIL missing-sum accepted (rust)"; exit 1; }
( cd "$getwork/app" && OXLMODCACHE="$getwork/cache" "$scratch/selfcheck$EXE" app ) > "$getwork/nosum.self" 2>&1 && { echo "FAIL missing-sum accepted (self)"; exit 1; }
grep -q 'missing oxl.sum entry for module' "$getwork/nosum.rust" || { echo "FAIL missing-sum text"; cat "$getwork/nosum.rust"; exit 1; }
diff "$getwork/nosum.rust" "$getwork/nosum.self" >/dev/null || { echo "FAIL missing-sum parity"; cat "$getwork/nosum.rust" "$getwork/nosum.self"; exit 1; }

echo "== v3 oxl mod commands =="
# tidy syncs the manifest with what the source imports — an
# un-required import is fetched through the same veneer (its dep's
# own requires land as // indirect), a require nothing imports is
# dropped, and oxl.sum is rewritten to the final require set.
# Same file:// POSIX-only constraint as the get fixture.
tv="$scratch/tidywork"
mkdir -p "$tv/dep/util" "$tv/leafpkg/leaf" "$tv/app/app" "$tv/cache" "$tv/locdep/thing"
( cd "$tv/dep" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/dep\noxl 0.3\nrequire file://%s/leafpkg v1.0.0\n' "$tv" "$tv" > oxl.mod
  printf 'package util\nimport "file://%s/leafpkg/leaf"\n\nfn answer() -> int { return leaf.leafval() + 41 }\n' "$tv" > util/util.oxl
  git add -A && git commit -qm dep && git tag v1.0.0 )
( cd "$tv/leafpkg" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/leafpkg\noxl 0.3\n' "$tv" > oxl.mod
  printf 'package leaf\n\nfn leafval() -> int { return 1 }\n' > leaf/leaf.oxl
  git add -A && git commit -qm leaf && git tag v1.0.0 )
printf 'package thing\n\nfn val() -> int { return 7 }\n' > "$tv/locdep/thing/thing.oxl"
printf 'module file://%s/locdep\noxl 0.3\n' "$tv" > "$tv/locdep/oxl.mod"
printf 'package main\nimport "file://%s/dep/util"\n\nfn main() { println("answer=" + itoa(util.answer())) }\n' "$tv" > "$tv/app/app/main.oxl"
# init: skeleton is a valid manifest, second init refuses
mkdir "$tv/fresh" && ( cd "$tv/fresh" && "$OXL" mod init example.com/fresh ) > "$tv/init1" 2>&1 || { echo "FAIL init rc"; cat "$tv/init1"; exit 1; }
grep -q '^module example.com/fresh$' "$tv/fresh/oxl.mod" || { echo "FAIL init manifest"; cat "$tv/fresh/oxl.mod"; exit 1; }
( cd "$tv/fresh" && "$OXL" mod init example.com/other ) > "$tv/init2" 2>&1 && { echo "FAIL re-init accepted"; cat "$tv/init2"; exit 1; }
grep -q 'already exists' "$tv/init2" || { echo "FAIL re-init text"; cat "$tv/init2"; exit 1; }
# tidy with no requires: fetches dep@latest, leafpkg lands // indirect
( cd "$tv/app" && printf 'module file://%s/app\noxl 0.3\n' "$tv" > oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy1" 2>&1 || { echo "FAIL tidy rc"; cat "$tv/tidy1"; exit 1; }
grep -q "added file://$tv/dep@" "$tv/tidy1" || { echo "FAIL tidy fetch"; cat "$tv/tidy1"; exit 1; }
grep -q "file://$tv/leafpkg v1.0.0 // indirect" "$tv/app/oxl.mod" || { echo "FAIL tidy indirect"; cat "$tv/app/oxl.mod"; exit 1; }
[ "$(grep -c ' h1:' "$tv/app/oxl.sum")" = 4 ] || { echo "FAIL tidy sums"; cat "$tv/app/oxl.sum"; exit 1; }
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" build app -o "$tv/appbin" ) || { echo "FAIL tidy build"; exit 1; }
[ "$("$tv/appbin")" = "answer=42" ] || { echo "FAIL tidy run"; "$tv/appbin"; exit 1; }
# tidy drops an unused require; second tidy is a no-op (idempotent)
printf 'module file://%s/app\noxl 0.3\n\nrequire (\n\tfile://%s/unused v1.9.9 // stale\n\tfile://%s/dep v1.0.0\n\tfile://%s/leafpkg v1.0.0 // indirect\n)\n' "$tv" "$tv" "$tv" "$tv" > "$tv/app/oxl.mod"
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy2" 2>&1 || { echo "FAIL tidy2 rc"; cat "$tv/tidy2"; exit 1; }
grep -q 'dropped file://' "$tv/tidy2" || { echo "FAIL tidy drop"; cat "$tv/tidy2"; exit 1; }
! grep -q unused "$tv/app/oxl.mod" || { echo "FAIL unused kept"; cat "$tv/app/oxl.mod"; exit 1; }
cp "$tv/app/oxl.mod" "$tv/m.save" && cp "$tv/app/oxl.sum" "$tv/s.save"
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) >/dev/null 2>&1 || { echo "FAIL tidy3 rc"; exit 1; }
diff "$tv/m.save" "$tv/app/oxl.mod" >/dev/null || { echo "FAIL tidy idempotent"; diff "$tv/m.save" "$tv/app/oxl.mod"; exit 1; }
# replace-only coverage: require is added at Go's placeholder version
printf 'module file://%s/app\noxl 0.3\n\nreplace file://%s/locdep => ../locdep\n' "$tv" "$tv" > "$tv/app/oxl.mod"
rm -f "$tv/app/oxl.sum"
printf 'package main\nimport "file://%s/locdep/thing"\n\nfn main() { println("thing=" + itoa(thing.val())) }\n' "$tv" > "$tv/app/app/main.oxl"
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy4" 2>&1 || { echo "FAIL tidy4 rc"; cat "$tv/tidy4"; exit 1; }
grep -q 'v0.0.0-00010101000000-000000000000' "$tv/app/oxl.mod" || { echo "FAIL replace placeholder"; cat "$tv/app/oxl.mod"; exit 1; }
# verify + list + unresolvable error
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod verify ) > "$tv/vfy" 2>&1 || { echo "FAIL verify rc"; cat "$tv/vfy"; exit 1; }
grep -q 'all modules verified' "$tv/vfy" || { echo "FAIL verify text"; cat "$tv/vfy"; exit 1; }
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod list ) > "$tv/mlist" 2>&1 || { echo "FAIL mod list rc"; cat "$tv/mlist"; exit 1; }
grep -q "file://$tv/locdep" "$tv/mlist" || { echo "FAIL list locdep"; cat "$tv/mlist"; exit 1; }
printf 'package main\nimport "file://%s/nosuch/pkg"\n\nfn main() {}\n' "$tv" > "$tv/app/app/main.oxl"
printf 'module file://%s/app\noxl 0.3\n' "$tv" > "$tv/app/oxl.mod" && rm -f "$tv/app/oxl.sum"
( cd "$tv/app" && OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy5" 2>&1 && { echo "FAIL tidy5 accepted"; cat "$tv/tidy5"; exit 1; }
grep -q 'no required module provides package' "$tv/tidy5" || { echo "FAIL tidy5 text"; cat "$tv/tidy5"; exit 1; }
# restore keeps the floor: a require pinned at v1.0.0 whose MVS
# selection is bumped to v1.2.0 by a dependency's own manifest gets
# re-fetched at the SELECTED version — but the require line is a
# minimum and must not move; only the missing sums are rewritten
mkdir -p "$tv/escdep/util" "$tv/esclib/leaf" "$tv/escapp/escapp"
( cd "$tv/esclib" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/esclib\noxl 0.3\n' "$tv" > oxl.mod
  printf 'package leaf\n\nfn leafval() -> int { return 1 }\n' > leaf/leaf.oxl
  git add -A && git commit -qm v1 && git tag v1.0.0
  printf 'package leaf\n\nfn leafval() -> int { return 2 }\n' > leaf/leaf.oxl
  git add -A && git commit -qm v2 && git tag v1.2.0 )
( cd "$tv/escdep" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/escdep\noxl 0.3\nrequire file://%s/esclib v1.2.0\n' "$tv" "$tv" > oxl.mod
  printf 'package util\nimport "file://%s/esclib/leaf"\n\nfn answer() -> int { return leaf.leafval() + 40 }\n' "$tv" > util/util.oxl
  git add -A && git commit -qm dep && git tag v1.0.0 )
printf 'package main\nimport "file://%s/escdep/util"\nimport "file://%s/esclib/leaf"\n\nfn main() { println("esc=" + itoa(util.answer()) + "/" + itoa(leaf.leafval())) }\n' "$tv" "$tv" > "$tv/escapp/escapp/main.oxl"
( cd "$tv/escapp" && printf 'module file://%s/escapp\noxl 0.3\n\nrequire file://%s/escdep v1.0.0\nrequire file://%s/esclib v1.0.0\n' "$tv" "$tv" "$tv" > oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/escdep@v1.0.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/esclib@v1.0.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy6" 2>&1 || { echo "FAIL tidy6 rc"; cat "$tv/tidy6"; exit 1; }
grep -q "file://$tv/esclib v1.0.0" "$tv/escapp/oxl.mod" || { echo "FAIL floor bumped"; cat "$tv/escapp/oxl.mod"; exit 1; }
grep -q "file://$tv/esclib v1.2.0 h1:" "$tv/escapp/oxl.sum" || { echo "FAIL restore sums"; cat "$tv/escapp/oxl.sum"; exit 1; }
( cd "$tv/escapp" && OXLMODCACHE="$tv/cache" "$OXL" build escapp -o "$tv/escbin" ) || { echo "FAIL esc build"; exit 1; }
[ "$("$tv/escbin")" = "esc=42/2" ] || { echo "FAIL esc run"; "$tv/escbin"; exit 1; }
# a dropped require takes its MVS edges with it: raiser is never
# imported but holds libx at v1.2.0 — the version the walk checked.
# Once raiser drops, keeping the v1.0.0 floor would select an
# unfetched version whose leaf package doesn't even exist, so tidy
# bumps the kept floor to the checked selection
mkdir -p "$tv/raiser/r" "$tv/libx" "$tv/rapp/rapp"
( cd "$tv/libx" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/libx\noxl 0.3\n' "$tv" > oxl.mod
  git add -A && git commit -qm v1 && git tag v1.0.0
  mkdir -p leaf && printf 'package leaf\n\nfn leafval() -> int { return 5 }\n' > leaf/leaf.oxl
  git add -A && git commit -qm v2 && git tag v1.2.0 )
( cd "$tv/raiser" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/raiser\noxl 0.3\nrequire file://%s/libx v1.2.0\n' "$tv" "$tv" > oxl.mod
  printf 'package r\n\nfn rval() -> int { return 0 }\n' > r/r.oxl
  git add -A && git commit -qm raiser && git tag v1.0.0 )
printf 'package main\nimport "file://%s/libx/leaf"\n\nfn main() { println("lv=" + itoa(leaf.leafval())) }\n' "$tv" > "$tv/rapp/rapp/main.oxl"
( cd "$tv/rapp" && printf 'module file://%s/rapp\noxl 0.3\n' "$tv" > oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/raiser@v1.0.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/libx@v1.2.0" >/dev/null
  sed 's|libx v1.2.0|libx v1.0.0|' oxl.mod > oxl.mod.tmp && mv oxl.mod.tmp oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy7" 2>&1 || { echo "FAIL tidy7 rc"; cat "$tv/tidy7"; exit 1; }
grep -q 'dropped file://' "$tv/tidy7" || { echo "FAIL tidy7 drop"; cat "$tv/tidy7"; exit 1; }
! grep -q 'raiser' "$tv/rapp/oxl.mod" || { echo "FAIL raiser kept"; cat "$tv/rapp/oxl.mod"; exit 1; }
grep -q "file://$tv/libx v1.2.0" "$tv/rapp/oxl.mod" || { echo "FAIL floor not raised"; cat "$tv/rapp/oxl.mod"; exit 1; }
( cd "$tv/rapp" && OXLMODCACHE="$tv/cache" "$OXL" build rapp -o "$tv/rbin" ) || { echo "FAIL rapp build"; exit 1; }
[ "$("$tv/rbin")" = "lv=5" ] || { echo "FAIL rapp run"; "$tv/rbin"; exit 1; }
# two-hop floor bump: dropping the unused raiser lowers mida below
# its checked selection so its floor moves — but the raised version
# drops the edge that was holding libb at the checked version. A
# single recheck pass would leave libb's floor untouched and write a
# manifest whose build selects a version lacking the package
mkdir -p "$tv/libb" "$tv/mida/a" "$tv/raiser2/r" "$tv/tapp/tapp"
( cd "$tv/libb" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/libb\noxl 0.3\n' "$tv" > oxl.mod
  git add -A && git commit -qm v1 && git tag v1.0.0
  mkdir b && printf 'package b\n\nfn bval() -> int { return 3 }\n' > b/b.oxl
  git add -A && git commit -qm v2 && git tag v1.2.0 )
( cd "$tv/mida" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/mida\noxl 0.3\nrequire file://%s/libb v1.2.0\n' "$tv" "$tv" > oxl.mod
  printf 'package a\n\nfn aval() -> int { return 10 }\n' > a/a.oxl
  git add -A && git commit -qm v1 && git tag v1.0.0
  printf 'module file://%s/mida\noxl 0.3\n' "$tv" > oxl.mod
  git add -A && git commit -qm v2 && git tag v1.1.0 )
( cd "$tv/raiser2" && git init -q . && git config user.email t@t && git config user.name t
  printf 'module file://%s/raiser2\noxl 0.3\nrequire file://%s/mida v1.1.0\nrequire file://%s/libb v1.2.0\n' "$tv" "$tv" "$tv" > oxl.mod
  printf 'package r\n\nfn rval() -> int { return 0 }\n' > r/r.oxl
  git add -A && git commit -qm r && git tag v1.0.0 )
printf 'package main\nimport "file://%s/mida/a"\nimport "file://%s/libb/b"\n\nfn main() { println("ab=" + itoa(a.aval()) + "/" + itoa(b.bval())) }\n' "$tv" "$tv" > "$tv/tapp/tapp/main.oxl"
( cd "$tv/tapp" && printf 'module file://%s/tapp\noxl 0.3\n' "$tv" > oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/mida@v1.0.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/mida@v1.1.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/libb@v1.0.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/libb@v1.2.0" >/dev/null
  OXLMODCACHE="$tv/cache" "$OXL" get "file://$tv/raiser2@v1.0.0" >/dev/null
  printf 'module file://%s/tapp\noxl 0.3\n\nrequire file://%s/mida v1.0.0\nrequire file://%s/libb v1.0.0\nrequire file://%s/raiser2 v1.0.0\n' "$tv" "$tv" "$tv" "$tv" > oxl.mod
  OXLMODCACHE="$tv/cache" "$OXL" mod tidy ) > "$tv/tidy8" 2>&1 || { echo "FAIL tidy8 rc"; cat "$tv/tidy8"; exit 1; }
grep -q "file://$tv/mida v1.1.0" "$tv/tapp/oxl.mod" || { echo "FAIL tidy8 mida floor"; cat "$tv/tapp/oxl.mod"; exit 1; }
grep -q "file://$tv/libb v1.2.0" "$tv/tapp/oxl.mod" || { echo "FAIL tidy8 libb floor"; cat "$tv/tapp/oxl.mod"; exit 1; }
! grep -q raiser2 "$tv/tapp/oxl.mod" || { echo "FAIL tidy8 raiser kept"; cat "$tv/tapp/oxl.mod"; exit 1; }
( cd "$tv/tapp" && OXLMODCACHE="$tv/cache" "$OXL" build tapp -o "$tv/tbin" ) || { echo "FAIL tidy8 build"; exit 1; }
[ "$("$tv/tbin")" = "ab=10/3" ] || { echo "FAIL tidy8 run"; "$tv/tbin"; exit 1; }
;;
esac

echo "== bootstrap codegen =="
# the self-hosted C emitter must produce working binaries — every
# example plus representative tests, run-and-compared end to end
cgen_dir=$(mktemp -d "$scratch/cgen.XXXXXX")
cgen() {  # cgen <src> <name> — emit C via codegen.oxl, compile it
    "$OXL" run bootstrap/cmd/codegen -- "$1" > "$cgen_dir/$2.c"
    # sqlite only when the emitted C calls it — mirrors emit_and_cc;
    # -l must follow the sources: with --as-needed a library listed
    # before them is dropped and rt.o's sqlite refs go undefined
    def=""; lib=""; net=""
    grep -q 'rt_sqlite_' "$cgen_dir/$2.c" && { def="-DOXL_SQLITE"; lib="$SQLITE_LINK"; }
    # OXL_NET mirrors emit_and_cc's socket scan — without it rt.c's
    # sock section isn't compiled and rt.h hides the prototypes
    grep -qE 'rt_sock_|rt_bind_any|rt_set_(reuseaddr|rcvtimo|sndtimo)|rt_htons|rt_htonl' "$cgen_dir/$2.c" && net="-DOXL_NET"
    "$CC" -O2 $THREAD_FLAG -Iruntime $def $net "$cgen_dir/$2.c" runtime/rt.c $lib $NET_LIBS -o "$cgen_dir/$2$EXE" || { echo "FAIL codegen cc $1"; exit 1; }
}
for f in examples/hello.oxl examples/features.oxl tests/match.oxl tests/chanbuf.oxl tests/methods.oxl tests/pairfwd.oxl; do
    n=$(basename "$f" .oxl)
    cgen "$f" "$n"
    got=$("$cgen_dir/$n$EXE")
    want=$("$OXL" run "$f")
    [ "$got" = "$want" ] || { echo "FAIL codegen $f"; exit 1; }
done
# cli takes argv — the emitted binary gets them directly (no `--`)
cgen examples/cli/main.oxl cli
got=$("$cgen_dir/cli$EXE" Jesse)
want=$("$OXL" run examples/cli/main.oxl -- Jesse)
[ "$got" = "$want" ] || { echo "FAIL codegen cli: $got"; exit 1; }
# --test mode: scope-wrapped runner like `oxl test`
"$OXL" run bootstrap/cmd/codegen -- --test examples/smoke_test.oxl > "$cgen_dir/t.c"
tnet=""
grep -qE 'rt_sock_|rt_bind_any|rt_set_(reuseaddr|rcvtimo|sndtimo)|rt_htons|rt_htonl' "$cgen_dir/t.c" && tnet="-DOXL_NET"
"$CC" -O2 $THREAD_FLAG -Iruntime $tnet "$cgen_dir/t.c" runtime/rt.c -DOXL_SQLITE $SQLITE_LINK $NET_LIBS -o "$cgen_dir/t$EXE" || { echo "FAIL codegen cc --test"; exit 1; }
out=$("$cgen_dir/t$EXE")
grep -q "test_chan ... ok" <<<"$out" || { echo "FAIL codegen --test: $out"; exit 1; }
# http pilot through the self-hosted codegen: the Handler param is an
# iface value, exercising the RtIface/vtab emission end to end
cgen examples/http-crud-sqlite/main.oxl httpsrv_cgen
httpwork=$(mktemp -d "$scratch/httpcgen.XXXXXX")
cp "$cgen_dir/httpsrv_cgen" "$httpwork/"
curl -s -m 1 -o /dev/null localhost:8080/items && { echo "FAIL: :8080 already bound"; exit 1; } || true
(cd "$httpwork"; ./httpsrv_cgen > srv.log 2>&1 & echo $! > srv.pid)
srv_pid=$(cat "$httpwork/srv.pid")
for _ in $(seq 1 50); do
    grep -q "listening" "$httpwork/srv.log" && break
    sleep 0.2
done
grep -q "listening" "$httpwork/srv.log" || { echo "FAIL: cgen server did not bind"; cat "$httpwork/srv.log"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s -d widget localhost:8080/items)
[ "$got" = '{"created":"widget"}' ] || { echo "FAIL cgen post: $got"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s localhost:8080/items)
grep -q '"name":"widget"' <<<"$got" || { echo "FAIL cgen get: $got"; kill $srv_pid 2>/dev/null; exit 1; }
kill $srv_pid 2>/dev/null || true
# gui pilot via codegen: emit + link against the shim whenever gtk4 dev
# files exist (same gate as the gui section below — headless compiles,
# DISPLAY additionally selftests)
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists gtk4 2>/dev/null; then
    "$CC" -O2 $(pkg-config --cflags gtk4) -c examples/gui-todo/gui_shim.c -o "$cgen_dir/gui_shim.o"
    "$OXL" run bootstrap/cmd/codegen -- examples/gui-todo/todo.oxl > "$cgen_dir/todo.c"
    todonet=""
    grep -qE 'rt_sock_|rt_bind_any|rt_set_(reuseaddr|rcvtimo|sndtimo)|rt_htons|rt_htonl' "$cgen_dir/todo.c" && todonet="-DOXL_NET"
    "$CC" -O2 $THREAD_FLAG -Iruntime $todonet "$cgen_dir/todo.c" runtime/rt.c "$cgen_dir/gui_shim.o" $(pkg-config --libs gtk4) $NET_LIBS -o "$cgen_dir/todo$EXE" || { echo "FAIL codegen cc gui"; exit 1; }
    if [ -n "${DISPLAY:-}" ]; then
        out=$(with_timeout 30 "$cgen_dir/todo$EXE" --selftest 2>/dev/null)
        grep -q "selftest rows=2" <<<"$out" || { echo "FAIL codegen gui selftest: $out"; exit 1; }
    fi
fi
# bootstrap chain: all four self-host components compiled by the
# self-hosted codegen must behave identically to `oxl run`
cgen "bootstrap/cmd/lex" "self_lexer"
cgen "bootstrap/cmd/parse" "self_parser"
cgen "bootstrap/cmd/check" "self_check"
cgen "bootstrap/cmd/codegen" "self_codegen"
out=$("$cgen_dir/self_lexer$EXE" examples/hello.oxl)
want=$("$OXL" run bootstrap/cmd/lex -- examples/hello.oxl)
[ "$out" = "$want" ] || { echo "FAIL self lexer: $out"; exit 1; }
grep -q "13 tokens" <<<"$out" || { echo "FAIL self lexer tokens: $out"; exit 1; }
out=$("$cgen_dir/self_parser$EXE" examples/features.oxl)
want=$("$OXL" run bootstrap/cmd/parse -- examples/features.oxl)
[ "$out" = "$want" ] || { echo "FAIL self parser"; exit 1; }
printf '%s' "$out" | $PY -c 'import json,sys
d=json.load(sys.stdin)
assert len(d["items"])==7 and not any("skip" in i for i in d["items"]), d' || { echo "FAIL self parser shape"; exit 1; }
out=$("$cgen_dir/self_check$EXE" examples/features.oxl)
[ "$out" = "ok" ] || { echo "FAIL self check: $out"; exit 1; }
# deeper chain: the self-compiled checker's diagnostics must equal the
# `oxl run` interpretation's on a corpus file — transitively Rust's
out=$("$cgen_dir/self_check$EXE" tests/diag/own_use_moved.oxl || true)
want=$("$OXL" run bootstrap/cmd/check -- tests/diag/own_use_moved.oxl || true)
[ "$out" = "$want" ] || { echo "FAIL self check diag: $out vs $want"; exit 1; }
# codegen self-compile emits byte-identical C to the direct emission
"$cgen_dir/self_codegen$EXE" examples/hello.oxl > "$cgen_dir/chain.c"
"$OXL" run bootstrap/cmd/codegen -- examples/hello.oxl > "$cgen_dir/direct.c"
cmp "$cgen_dir/chain.c" "$cgen_dir/direct.c" || { echo "FAIL bootstrap chain"; exit 1; }

echo "== a1 coroutine scale =="
# spec bar: 10k spawned tasks parked on recv must cost heap frames,
# not OS threads — assert on the per-task delta over this binary's
# own zero-task baseline. An absolute cap can't separate task memory
# from the platform's process baseline (Windows carries ~4 MB of
# CRT/winsock/heap before main runs vs ~2 MB on POSIX).
"$OXL" build tests/conc_scale.oxl -o "$scratch/conc_scale$EXE" || {
    echo "FAIL conc_scale: compile failed"; exit 1; }
"$scratch/conc_scale$EXE" 0 0 2500 > "$scratch/base.out" 2>&1 &
base_pid=$!
base_rss=0
# sample before the liveness gate: a just-spawned native child can lag
# the git-bash process table on Windows, so a leading kill -0 can skip
# the whole window and leave base_rss empty on a healthy run
for try in 1 2; do
    while :; do
        r=$(rss_kb $base_pid || true)
        [ -n "$r" ] && [ "$r" -gt "$base_rss" ] && base_rss=$r
        kill -0 $base_pid 2>/dev/null || break
        sleep 0.2
    done
    wait $base_pid 2>/dev/null || true
    # retry only the observed flake: the run completed (printed done)
    # yet yielded no sample — a spawn-registration race, not a crash;
    # relaunch only while an iteration remains to consume it
    if [ "$base_rss" -gt 0 ] || [ "$(tail -1 "$scratch/base.out")" != "done" ] || [ "$try" -eq 2 ]; then
        break
    fi
    "$scratch/conc_scale$EXE" 0 0 2500 > "$scratch/base.out" 2>&1 &
    base_pid=$!
done
"$scratch/conc_scale$EXE" 10000 1 2500 > "$scratch/scale.out" 2>&1 &
scale_pid=$!
# watchdog: a wedged channel/task regression must not hang the whole
# suite — kill the measured binary at the deadline; the "done" check
# below then reports the failure instead of blocking forever
( sleep 120; kill -KILL $scale_pid 2>/dev/null;   echo "watchdog: killed after 120s" >> "$scratch/scale.out" ) >/dev/null 2>&1 &
scale_dog=$!
# peak matters, not a point sample: poll RSS until exit and take the
# max. `ps -o rss=` is the portable form (/proc is Linux-only).
# The process can exit between kill -0 and ps — under pipefail a failed
# ps makes the substitution nonzero and `set -e` would abort the suite
# before `wait` can report the real result; `|| true` pins it.
rss=0
while :; do
    r=$(rss_kb $scale_pid || true)
    [ -n "$r" ] && [ "$r" -gt "$rss" ] && rss=$r
    kill -0 $scale_pid 2>/dev/null || break
    sleep 0.2
done
kill "$scale_dog" 2>/dev/null || true
wait "$scale_dog" 2>/dev/null || true
scale_rc=0
wait $scale_pid || scale_rc=$?
[ "$scale_rc" -eq 0 ] && [ "$(tail -1 "$scratch/scale.out")" = "done" ] || {
    echo "FAIL scale: rc=$scale_rc: $(cat "$scratch/scale.out")"; exit 1; }
[ -n "$rss" ] && [ "$rss" -gt 0 ] || { echo "FAIL scale: no RSS sample"; exit 1; }
[ -n "$base_rss" ] && [ "$base_rss" -gt 0 ] || {
    echo "FAIL scale: no baseline RSS sample"; cat "$scratch/base.out"; exit 1; }
# ~0.3kB/parked task observed on POSIX (RtTask + frame, one alloc);
# 1kB/task leaves >3x headroom for Windows heap commit granularity
# yet still fails an order of magnitude under any OS-thread-per-task
# regression (a thread costs tens of kB minimum).
delta=$((rss - base_rss))
[ "$delta" -lt 10240 ] || { echo "FAIL scale: ${delta}kB over ${base_rss}kB baseline at 10k parked (spec < 1kB/task)"; exit 1; }
echo "  10k parked tasks: ${delta}kB over ${base_rss}kB baseline"
out=$("$OXL" run tests/conc_ping.oxl -- 5000)
grep -q "bounces=5000" <<<"$out" || { echo "FAIL ping: $out"; exit 1; }

echo "== a2 select =="
out=$("$OXL" run tests/select_fair.oxl)
grep -qE "^select fair ok a=[0-9]+$" <<<"$(tail -1 <<<"$out")" || { echo "FAIL select_fair: $out"; exit 1; }
out=$("$OXL" run tests/select.oxl)
[ "$(tail -1 <<<"$out")" = "select done" ] || { echo "FAIL select: $out"; exit 1; }
# parked select must unwedge on sibling trap — exits nonzero, not a hang.
# 124 = with_timeout expiry = the hang this test exists to catch. Build
# first so a compile error fails the check instead of passing as a
# non-124 nonzero exit.
"$OXL" build tests/select_cancel.oxl -o "$scratch/select_cancel$EXE" || {
    echo "FAIL select_cancel: compile failed"; exit 1; }
rc=0
with_timeout 10 "$scratch/select_cancel$EXE" >/dev/null 2>&1 || rc=$?
if [ "$rc" -eq 0 ] || [ "$rc" -eq 124 ]; then
    echo "FAIL select_cancel: expected trap exit, got rc=$rc"; exit 1
fi

# orphan drain: a frame task returning inside an open scope orphans it;
# its teardown must not lock the joined scope's destroyed mutex. Build
# first so a compile error fails the check, and run the binary directly —
# under `oxl run` the watchdog would kill only the driver while the child
# keeps the capture pipe and $( ) hangs past the deadline.
"$OXL" build tests/orphan_join.oxl -o "$scratch/orphan_join$EXE" || {
    echo "FAIL orphan_join: compile failed"; exit 1; }
rc=0
out=$(with_timeout 30 "$scratch/orphan_join$EXE") || rc=$?
# marker alone isn't enough: a crash/time-out after the print must fail
[ "$rc" -eq 0 ] && [ "$(tail -1 <<<"$out")" = "orphan-join ok" ] || { echo "FAIL orphan_join: rc=$rc: $out"; exit 1; }

# joiner lost-wakeup: a trap's descendant cancel temp-holds a scope
# whose owner is parked joining it; if the temp drop is the pending->0
# transition it must still wake the joiner. Build first so a compile
# error fails instead of passing as a non-124 nonzero.
"$OXL" build tests/cancel_join.oxl -o "$scratch/cancel_join$EXE" || {
    echo "FAIL cancel_join: compile failed"; exit 1; }
rc=0
with_timeout 25 "$scratch/cancel_join$EXE" >/dev/null 2>&1 || rc=$?
if [ "$rc" -eq 0 ] || [ "$rc" -eq 124 ]; then
    echo "FAIL cancel_join: expected trap exit, got rc=$rc"; exit 1
fi

# scope-exit join: break/continue/return inside a loop-nested scope must
# drain its children — a bare goto orphaned them. Build + run the binary
# directly like orphan_join: detached children would hold the capture
# pipe open past the deadline under `oxl run`.
"$OXL" build tests/scope_exit.oxl -o "$scratch/scope_exit$EXE" || {
    echo "FAIL scope_exit: compile failed"; exit 1; }
rc=0
out=$(with_timeout 30 "$scratch/scope_exit$EXE") || rc=$?
[ "$rc" -eq 0 ] && [ "$(tail -1 <<<"$out")" = "scope-exit ok" ] || { echo "FAIL scope_exit: rc=$rc: $out"; exit 1; }

echo "== a3 buffered channels =="
out=$("$OXL" run tests/chanbuf.oxl)
[ "$(tail -1 <<<"$out")" = "chanbuf done" ] || { echo "FAIL chanbuf: $out"; exit 1; }
out=$("$OXL" run tests/chanbuf_close.oxl)
[ "$(tail -1 <<<"$out")" = "chanbuf close ok" ] || { echo "FAIL chanbuf_close: $out"; exit 1; }

echo "== fmt check =="
# the whole .oxl corpus must be canonically formatted
if ! "$OXL" fmt --check $(find std bootstrap tests examples -name '*.oxl' | sort); then
    echo "FAIL fmt: run \`oxl fmt -w\` on the corpus"; exit 1
fi


echo "== lsp hover/definition =="
# a Windows python can't exec an msys path — hand it the native spelling
OXL_WIN=$(cygpath -w "$OXL" 2>/dev/null || printf '%s' "$OXL")
$PY - "$OXL_WIN" <<'PYEOF'
import json, pathlib, subprocess, sys
oxl = sys.argv[1]
p = subprocess.Popen([oxl, "lsp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def send(m):
    b = json.dumps(m).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); p.stdin.flush()
def recv():
    h = {}
    while True:
        l = p.stdout.readline().decode().strip()
        if not l: break
        k, v = l.split(":", 1); h[k.strip()] = v.strip()
    return json.loads(p.stdout.read(int(h["Content-Length"])).decode())
uri = pathlib.Path("bootstrap/cmd/parse/main.oxl").resolve().as_uri()
src = open("bootstrap/cmd/parse/main.oxl").read()
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}); recv()
send({"jsonrpc":"2.0","method":"initialized","params":{}})
send({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"oxl","version":1,"text":src}}}); recv()
lines = src.split("\n")
i = next(i for i,l in enumerate(lines) if "lexlib.lex(src)" in l)
col = lines[i].index("lexlib.lex") + 7
pos = {"textDocument":{"uri":uri},"position":{"line":i,"character":col}}
send({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":pos})
h = recv(); assert "fn lexlib.lex(src: str)" in json.dumps(h), h
send({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":pos})
d = recv(); assert d["result"]["uri"].endswith("lexlib/lexlib.oxl"), d
send({"jsonrpc":"2.0","method":"exit","params":{}})
PYEOF

echo "== http pilot =="
work=$(mktemp -d "$scratch/http.XXXXXX")
"$OXL" build examples/http-crud-sqlite/main.oxl -o "$work/httpsrv$EXE"
curl -s -m 1 -o /dev/null localhost:8080/items && { echo "FAIL: :8080 already bound"; exit 1; } || true
(cd "$work"; ./httpsrv$EXE > srv.log 2>&1 & echo $! > srv.pid)
srv_pid=$(cat "$work/srv.pid")
# readiness can lag on slow runners — poll the log instead of a fixed sleep
for _ in $(seq 1 50); do
    grep -q "listening" "$work/srv.log" && break
    sleep 0.2
done
grep -q "listening" "$work/srv.log" || { echo "FAIL: server did not bind"; cat "$work/srv.log"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s -d widget localhost:8080/items)
[ "$got" = '{"created":"widget"}' ] || { echo "FAIL post: $got"; kill $srv_pid 2>/dev/null; exit 1; }
code=$(curl -s -o /dev/null -w '%{http_code}' -d widget localhost:8080/items)
[ "$code" = "201" ] || { echo "FAIL post status: $code"; kill $srv_pid 2>/dev/null; exit 1; }
# newline in a name would split one stored row into two items on read —
# the server must reject it with a 400 rather than corrupt the list
code=$(curl -s -o /dev/null -w '%{http_code}' -d $'bad\nname' localhost:8080/items)
[ "$code" = "400" ] || { echo "FAIL newline status: $code"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s localhost:8080/items)
grep -q '"name":"widget"' <<<"$got" || { echo "FAIL get: $got"; kill $srv_pid 2>/dev/null; exit 1; }
kill $srv_pid 2>/dev/null || true

echo "== gui pilot =="
# GTK4 FFI pilot: build whenever gtk4 dev files exist; run the
# signal-path selftest only when a display is available (CI macOS /
# headless runners build but don't launch)
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists gtk4 2>/dev/null; then
    (cd examples/gui-todo && OXL="$OXL" ./build.sh >/dev/null)
    if [ -n "${DISPLAY:-}" ]; then
        out=$(with_timeout 30 examples/gui-todo/todo --selftest 2>/dev/null)
        grep -q "selftest rows=2" <<<"$out" || { echo "FAIL gui selftest: $out"; exit 1; }
        grep -q "gui done" <<<"$out" || { echo "FAIL gui run: $out"; exit 1; }
    fi
else
    echo "  (skipped: gtk4 not installed)"
fi

echo "ALL TESTS PASSED"
