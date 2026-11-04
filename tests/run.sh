#!/usr/bin/env bash
# tests/run.sh <path-to-lang-binary> — end-to-end suite run by CI.
set -euo pipefail
LANG="${1:-lang}"
cd "$(dirname "$0")/.."

echo "== hello =="
out=$("$LANG" run examples/hello.lang)
[ "$out" = "hello, world" ] || { echo "FAIL hello: $out"; exit 1; }

echo "== features =="
out=$("$LANG" run examples/features.lang)
[ "$(tail -1 <<<"$out")" = "done" ] || { echo "FAIL features"; exit 1; }

echo "== cli pilot =="
out=$("$LANG" run examples/cli/main.lang -- Jesse)
grep -q "hello, Jesse" <<<"$out" || { echo "FAIL cli"; exit 1; }

echo "== lang test: smoke =="
"$LANG" test examples/smoke_test.lang | tee /dev/stderr | grep -q "test_chan ... ok"

echo "== lang test: stdlib =="
"$LANG" test tests/stdlib_test.lang | grep -q "test_loops ... ok"

echo "== bootstrap lexer =="
out=$("$LANG" run bootstrap/lexer.lang -- examples/hello.lang)
grep -q "11 tokens" <<<"$out" || { echo "FAIL lexer"; exit 1; }

echo "== http pilot =="
work=$(mktemp -d)
"$LANG" build examples/http-crud-sqlite/main.lang -o "$work/httpsrv"
pkill -f "$work/httpsrv" 2>/dev/null || true
curl -s -m 1 -o /dev/null localhost:8080/items && { echo "FAIL: :8080 already bound"; exit 1; } || true
(cd "$work"; ./httpsrv > srv.log 2>&1 & echo $! > srv.pid)
sleep 1
srv_pid=$(cat "$work/srv.pid")
grep -q "listening" "$work/srv.log" || { echo "FAIL: server did not bind"; cat "$work/srv.log"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s -d widget localhost:8080/items)
[ "$got" = "created: widget" ] || { echo "FAIL post: $got"; kill $srv_pid 2>/dev/null; exit 1; }
got=$(curl -s localhost:8080/items)
grep -q "1|widget" <<<"$got" || { echo "FAIL get: $got"; kill $srv_pid 2>/dev/null; exit 1; }
kill $srv_pid 2>/dev/null || true

echo "ALL TESTS PASSED"
