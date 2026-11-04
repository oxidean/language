#!/usr/bin/env bash
# tests/run.sh <path-to-oxl-binary> — end-to-end suite run by CI.
set -euo pipefail
OXL="${1:-oxl}"
cd "$(dirname "$0")/.."

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
"$OXL" test examples/smoke_test.oxl | tee /dev/stderr | grep -q "test_chan ... ok"

echo "== oxl test: stdlib =="
"$OXL" test tests/stdlib_test.oxl | grep -q "test_loops ... ok"

echo "== stdmods (json/sync/time) =="
out=$("$OXL" run tests/stdmods_test.oxl)
grep -q '"name":"widget"' <<<"$out" || { echo "FAIL stdmods json: $out"; exit 1; }
grep -q "^4000$" <<<"$out" || { echo "FAIL stdmods sync counter: $out"; exit 1; }
[ "$(tail -1 <<<"$out")" = "done" ] || { echo "FAIL stdmods time: $out"; exit 1; }

echo "== float (d1) =="
"$OXL" test tests/float_test.oxl | grep -q "test_json_float ... ok"
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

echo "== defer (b5) =="
out=$("$OXL" run tests/defer_test.oxl)
[ "$(head -3 <<<"$out")" = $'3\n2\n1' ] || { echo "FAIL defer LIFO: $out"; exit 1; }
grep -q "^9$" <<<"$out" || { echo "FAIL defer error-return path: $out"; exit 1; }

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
if timeout 10 "$OXL" run tests/select_cancel.oxl >/dev/null 2>&1; then
    echo "FAIL select_cancel: expected nonzero exit"; exit 1
fi

echo "== a3 buffered channels =="
out=$("$OXL" run tests/chanbuf.oxl)
[ "$(tail -1 <<<"$out")" = "chanbuf done" ] || { echo "FAIL chanbuf: $out"; exit 1; }

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
got=$(curl -s localhost:8080/items)
grep -q '"name":"widget"' <<<"$got" || { echo "FAIL get: $got"; kill $srv_pid 2>/dev/null; exit 1; }
kill $srv_pid 2>/dev/null || true

echo "ALL TESTS PASSED"
