#!/usr/bin/env bash
# tests/run.sh <path-to-oxl-binary> — end-to-end suite run by CI.
set -euo pipefail
OXL="${1:-oxl}"
cd "$(dirname "$0")/.."
# absolutize — subshell gates (gui pilot) invoke $OXL from other dirs
case "$OXL" in /*) ;; *) OXL="$PWD/$OXL";; esac

# GNU timeout doesn't exist on stock macOS; fall back to gtimeout
# (brew coreutils), else run unbounded.
with_timeout() {
    if command -v timeout >/dev/null 2>&1; then timeout "$@"
    elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$@"
    else shift; "$@"
    fi
}

echo "== hello =="
out=$("$OXL" run examples/hello.oxl)
[ "$out" = "hello, world" ] || { echo "FAIL hello: $out"; exit 1; }

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

echo "== stdmods (json/sync/time) =="
out=$("$OXL" run tests/stdmods_test.oxl)
grep -q '"name":"widget"' <<<"$out" || { echo "FAIL stdmods json: $out"; exit 1; }
grep -q "^4000$" <<<"$out" || { echo "FAIL stdmods sync counter: $out"; exit 1; }
[ "$(tail -1 <<<"$out")" = "done" ] || { echo "FAIL stdmods time: $out"; exit 1; }

echo "== float (d1) =="
out=$("$OXL" test tests/float_test.oxl)
grep -q "test_json_float ... ok" <<<"$out" || { echo "FAIL float: $out"; exit 1; }
# int/float mixing must be a compile error, not silent widening
cat > /tmp/floatmix.oxl <<'EOF'
fn main() { x := 1 + 2.5 }
EOF
if "$OXL" build /tmp/floatmix.oxl -o /tmp/floatmix 2>/dev/null; then
    echo "FAIL: int+float compiled (should be a type error)"; exit 1
fi

echo "== enum (b1) =="
out=$("$OXL" run tests/enum_test.oxl)
[ "$(tail -1 <<<"$out")" = "enum fns ok" ] || { echo "FAIL enum: $out"; exit 1; }

echo "== match (b2) =="
out=$("$OXL" run tests/match.oxl)
[ "$(tail -1 <<<"$out")" = "match ok" ] || { echo "FAIL match: $out"; exit 1; }
# non-exhaustive match must name the missing variant
cat > /tmp/matchnex.oxl <<'EOF'
enum E { A(int), B, }
fn f(e: E) -> int { return match e { E.A(v) => v } }
fn main() { }
EOF
if "$OXL" build /tmp/matchnex.oxl -o /tmp/matchnex 2>/dev/null; then
    echo "FAIL: non-exhaustive match compiled"; exit 1
fi
diag=$("$OXL" build /tmp/matchnex.oxl -o /tmp/matchnex 2>&1 || true)
grep -q "missing B" <<<"$diag" || { echo "FAIL: missing-variant diagnostic: $diag"; exit 1; }

echo "== slices (b3) =="
out=$("$OXL" run tests/slices.oxl)
[ "$(tail -1 <<<"$out")" = "slices ok" ] || { echo "FAIL slices: $out"; exit 1; }
# out-of-range index must trap, not corrupt
if "$OXL" run tests/slice_oob.oxl >/dev/null 2>&1; then
    echo "FAIL slice_oob: expected nonzero exit"; exit 1
fi
# inverted view range must trap, not corrupt
if "$OXL" run tests/slice_range.oxl >/dev/null 2>&1; then
    echo "FAIL slice_range: expected nonzero exit"; exit 1
fi

echo "== fn values (b4) =="
out=$("$OXL" run tests/fnval.oxl)
[ "$(tail -1 <<<"$out")" = "fnval ok" ] || { echo "FAIL fnval: $out"; exit 1; }
# fn values can't suspend, capture susp, compare, or wrap generics/pairs
for bad in susp clsusp ctrans gen pair eq; do
    cat > /tmp/fnv_$bad.oxl <<EOF
$(case $bad in
  susp)   printf 'fn s() { sleep(1) }\nfn main() { f := s }' ;;
  clsusp) printf 'fn main() { f := { |x: int| sleep(1); return x } }' ;;
  ctrans) printf 'fn w() { sleep(1) }\nfn main() { f := { |x: int| w(); return x } }' ;;
  gen)    printf 'fn id[T any](x: T) -> T { return x }\nfn main() { f := id[int] }' ;;
  pair)   printf 'fn p() -> [int, err] { return 1, nil }\nfn main() { f := p }' ;;
  eq)     printf 'fn a() {}\nfn main() { f := a; if f == a { println(1) } }' ;;
esac)
EOF
    if "$OXL" build /tmp/fnv_$bad.oxl -o /tmp/fnv_$bad 2>/dev/null; then
        echo "FAIL: fnval/$bad compiled (should be rejected)"; exit 1
    fi
done

echo "== collections (b6) =="
out=$("$OXL" run tests/collections_test.oxl)
[ "$(tail -1 <<<"$out")" = "collections ok" ] || { echo "FAIL collections: $out"; exit 1; }

echo "== defer (b5) =="
out=$("$OXL" run tests/defer_test.oxl)
[ "$(head -3 <<<"$out")" = $'3\n2\n1' ] || { echo "FAIL defer LIFO: $out"; exit 1; }
grep -q "^9$" <<<"$out" || { echo "FAIL defer error-return path: $out"; exit 1; }
# loop registers per-iteration (drains 2 1 0); untaken branch registers nothing (7 only)
grep -q $'2\n1\n0' <<<"$out" || { echo "FAIL defer loop/capture: $out"; exit 1; }
if grep -q "^99$" <<<"$out"; then echo "FAIL defer untaken branch: $out"; exit 1; fi
# trap unwind still drains the task's defer stack before aborting
cat > /tmp/d5trap.oxl <<'EOF'
fn dmark(x: int) { println(itoa(x)) }
fn willtrap() {
    defer dmark(6)
    defer dmark(5)
    s := mkslice(int, 2)
    println(s[9])
}
fn main() { willtrap() }
EOF
trapout=$(with_timeout 10 "$OXL" run /tmp/d5trap.oxl 2>&1 || true)
grep -q "^5$" <<<"$trapout" || { echo "FAIL defer trap drain: $trapout"; exit 1; }
grep -q "^6$" <<<"$trapout" || { echo "FAIL defer trap drain order: $trapout"; exit 1; }

echo "== methods + interfaces (d3) =="
out=$("$OXL" run tests/methods.oxl)
[ "$out" = "methods ok" ] || { echo "FAIL methods: $out"; exit 1; }
# a method-less type must not satisfy a method interface
cat > /tmp/d3sat.oxl <<'D3EOF'
interface Writer { write(s: str) -> int }
struct Bare { n: int }
fn emit(w: Writer) -> int { return w.write("x") }
fn main() { emit(Bare{n: 1}) }
D3EOF
if "$OXL" build /tmp/d3sat.oxl -o /tmp/d3sat 2>/dev/null; then
    echo "FAIL: unsatisfied iface compiled"; exit 1
fi
# methods can't suspend — vtable calls carry no task frame
cat > /tmp/d3susp.oxl <<'D3EOF'
struct S { n: int }
fn (s S) wait() { sleep(1) }
fn main() { s := S{n: 0}; s.wait() }
D3EOF
if "$OXL" build /tmp/d3susp.oxl -o /tmp/d3susp 2>/dev/null; then
    echo "FAIL: suspending method compiled"; exit 1
fi

echo "== move tracking (d2) =="
# use-after-move must be a compile error naming the binding
cat > /tmp/d2move.oxl <<'EOF'
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); take(p); take(p) }
EOF
if "$OXL" build /tmp/d2move.oxl -o /tmp/d2move 2>/dev/null; then
    echo "FAIL: use-after-move compiled"; exit 1
fi
diag=$("$OXL" build /tmp/d2move.oxl -o /tmp/d2move 2>&1 || true)
grep -q 'use of moved value `p`' <<<"$diag" || { echo "FAIL moved diag: $diag"; exit 1; }
# the headline bug class — one own cell into two spawned workers
cat > /tmp/d2spawn.oxl <<'EOF'
struct P { v: int }
fn w(p: own P) { }
fn main() { p := new(P); spawn w(p); spawn w(p) }
EOF
if "$OXL" build /tmp/d2spawn.oxl -o /tmp/d2spawn 2>/dev/null; then
    echo "FAIL: double-spawn of own compiled"; exit 1
fi
# moved inside a loop body — the next iteration would read a moved value
cat > /tmp/d2for.oxl <<'EOF'
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); for true { take(p) } }
EOF
if "$OXL" build /tmp/d2for.oxl -o /tmp/d2for 2>/dev/null; then
    echo "FAIL: move-in-loop compiled"; exit 1
fi
# re-init clears the mark
cat > /tmp/d2ok.oxl <<'EOF'
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
out=$("$OXL" run /tmp/d2ok.oxl)
[ "$out" = "ok" ] || { echo "FAIL d2 reinit: $out"; exit 1; }
# moved in one branch: the join can't prove which path ran — reject the
# later read unless EVERY path re-initializes the binding
cat > /tmp/d2branch.oxl <<'EOF'
struct P { v: int }
fn take(p: own P) { }
fn main() { p := new(P); if len("x") > 0 { take(p) }; take(p) }
EOF
if "$OXL" build /tmp/d2branch.oxl -o /tmp/d2branch 2>/dev/null; then
    echo "FAIL: one-branch move compiled"; exit 1
fi
# shared cells are copy/shareable — same cell to four workers is intended
out=$("$OXL" run tests/stdmods_test.oxl)
grep -q "^4000$" <<<"$out" || { echo "FAIL d2 shared workers: $out"; exit 1; }

echo "== bootstrap lexer =="
out=$("$OXL" run bootstrap/lexer.oxl -- examples/hello.oxl)
grep -q "11 tokens" <<<"$out" || { echo "FAIL lexer"; exit 1; }

echo "== bootstrap parser =="
out=$("$OXL" run bootstrap/parser.oxl -- examples/features.oxl)
python3 - "$out" <<'PY' || { echo "FAIL parser"; exit 1; }
import json,sys
d=json.loads(sys.argv[1])
assert len(d['items'])==6 and not any('skip' in i for i in d['items']), d
PY
out=$("$OXL" run bootstrap/parser.oxl -- bootstrap/parser.oxl)
python3 - "$out" <<'PY' || { echo "FAIL parser self-parse"; exit 1; }
import json,sys
d=json.loads(sys.argv[1])
assert len(d['items'])>20 and not any('skip' in i for i in d['items']), d
PY

echo "== bootstrap checker =="
# the self-hosted typechecker must check itself and representative sources
for f in bootstrap/check.oxl examples/features.oxl tests/match.oxl stdlib/collections.oxl; do
    out=$("$OXL" run bootstrap/check.oxl -- "$f")
    [ "$out" = "ok" ] || { echo "FAIL checker $f: $out"; exit 1; }
done

echo "== bootstrap codegen =="
# the self-hosted C emitter must produce working binaries
cgen_dir=$(mktemp -d)
for f in examples/hello.oxl examples/features.oxl tests/match.oxl tests/chanbuf.oxl; do
    "$OXL" run bootstrap/codegen.oxl -- "$f" > "$cgen_dir/out.c"
    "${LANG_CC:-cc}" -O2 -pthread -Iruntime "$cgen_dir/out.c" runtime/rt.c -lsqlite3 -o "$cgen_dir/out" || { echo "FAIL codegen cc $f"; exit 1; }
    got=$("$cgen_dir/out")
    want=$("$OXL" run "$f")
    [ "$got" = "$want" ] || { echo "FAIL codegen $f"; exit 1; }
done
# --test mode: scope-wrapped runner like `oxl test`
"$OXL" run bootstrap/codegen.oxl -- --test examples/smoke_test.oxl > "$cgen_dir/t.c"
"${LANG_CC:-cc}" -O2 -pthread -Iruntime "$cgen_dir/t.c" runtime/rt.c -lsqlite3 -o "$cgen_dir/t" || { echo "FAIL codegen cc --test"; exit 1; }
out=$("$cgen_dir/t")
grep -q "test_chan ... ok" <<<"$out" || { echo "FAIL codegen --test: $out"; exit 1; }
# bootstrap chain: codegen compiled by itself emits identical C
"$OXL" run bootstrap/codegen.oxl -- bootstrap/codegen.oxl > "$cgen_dir/self.c"
"${LANG_CC:-cc}" -O2 -pthread -Iruntime "$cgen_dir/self.c" runtime/rt.c -lsqlite3 -o "$cgen_dir/self" || { echo "FAIL codegen self-cc"; exit 1; }
"$cgen_dir/self" examples/hello.oxl > "$cgen_dir/chain.c"
"$OXL" run bootstrap/codegen.oxl -- examples/hello.oxl > "$cgen_dir/direct.c"
cmp "$cgen_dir/chain.c" "$cgen_dir/direct.c" || { echo "FAIL bootstrap chain"; exit 1; }

echo "== a1 coroutine scale =="
# 2k frame tasks parked on recv, then two drain rounds — a thread-per-task
# executor cannot do this within pool limits; frames must
out=$("$OXL" run tests/conc_scale.oxl -- 2000 2 50)
[ "$out" = "done" ] || { echo "FAIL scale: $out"; exit 1; }
out=$("$OXL" run tests/conc_ping.oxl -- 5000)
grep -q "bounces=5000" <<<"$out" || { echo "FAIL ping: $out"; exit 1; }

echo "== a2 select =="
out=$("$OXL" run tests/select.oxl)
[ "$(tail -1 <<<"$out")" = "select done" ] || { echo "FAIL select: $out"; exit 1; }
# parked select must unwedge on sibling trap — exits nonzero, not a hang
if with_timeout 10 "$OXL" run tests/select_cancel.oxl >/dev/null 2>&1; then
    echo "FAIL select_cancel: expected nonzero exit"; exit 1
fi

echo "== a3 buffered channels =="
out=$("$OXL" run tests/chanbuf.oxl)
[ "$(tail -1 <<<"$out")" = "chanbuf done" ] || { echo "FAIL chanbuf: $out"; exit 1; }

echo "== fmt check =="
# the whole .oxl corpus must be canonically formatted
if ! "$OXL" fmt --check $(find stdlib bootstrap tests examples -name '*.oxl' | sort); then
    echo "FAIL fmt: run \`oxl fmt -w\` on the corpus"; exit 1
fi


echo "== lsp hover/definition =="
python3 - "$OXL" <<'PYEOF'
import json, subprocess, sys, os
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
uri = "file://" + os.path.abspath("bootstrap/parser.oxl")
src = open("bootstrap/parser.oxl").read()
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
d = recv(); assert d["result"]["uri"].endswith("bootstrap/lexlib.oxl"), d
send({"jsonrpc":"2.0","method":"exit","params":{}})
PYEOF

echo "== http pilot =="
work=$(mktemp -d)
"$OXL" build examples/http-crud-sqlite/main.oxl -o "$work/httpsrv"
pkill -f "$work/httpsrv" 2>/dev/null || true
curl -s -m 1 -o /dev/null localhost:8080/items && { echo "FAIL: :8080 already bound"; exit 1; } || true
(cd "$work"; ./httpsrv > srv.log 2>&1 & echo $! > srv.pid)
sleep 1
srv_pid=$(cat "$work/srv.pid")
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
