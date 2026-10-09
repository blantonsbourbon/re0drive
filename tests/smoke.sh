#!/usr/bin/env bash
# Offline end-to-end smoke test (34 checks).
# Self-contained: starts tests/mock_upstream.py and a fresh re0drive instance on random local ports,
# with EVERY external endpoint (RE0 relay, 115, TMDB) pointed at the mock. Uses only fake credentials
# and never contacts real services. Usage: tests/smoke.sh [path/to/re0drive]  (default target/release/re0drive)
set -u
cd "$(dirname "$0")/.."
BIN=${1:-target/release/re0drive}
[ -x "$BIN" ] || { echo "binary not found: $BIN (run cargo build --release first)"; exit 2; }
free_port(){ python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])'; }
MP=$(free_port); SP=$(free_port); TMP=$(mktemp -d); DATA=$TMP/data
MOCK=http://127.0.0.1:$MP; BASE=http://127.0.0.1:$SP
python3 tests/mock_upstream.py "$MP" mock-test-key >"$TMP/mock.log" 2>&1 & MOCK_PID=$!
# Tripwire: route any non-loopback request into a dead proxy so an accidental real call fails instead of leaving the box.
env HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost \
  http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 all_proxy=http://127.0.0.1:9 no_proxy=127.0.0.1,localhost \
  RE0DRIVE_DATA="$DATA" RE0DRIVE_BIND=127.0.0.1:$SP RE0_GATEWAY_URL="$MOCK" \
  RE0DRIVE_115_BASE_URL="$MOCK" RE0DRIVE_TMDB_BASE_URL="$MOCK" RE0DRIVE_115_QR_BASE_URL="$MOCK" "$BIN" >"$TMP/server.log" 2>&1 & SRV_PID=$!
trap 'kill $SRV_PID $MOCK_PID 2>/dev/null; wait 2>/dev/null; rm -rf "$TMP"' EXIT
for _ in $(seq 1 50); do curl -sf $BASE/health >/dev/null && curl -s $MOCK/_calls >/dev/null && break; sleep 0.2; done
JAR=$(mktemp); PASS=0; FAILN=0
c(){ curl -s -b "$JAR" -c "$JAR" -H 'X-Re0drive-Request: 1' -H 'Content-Type: application/json' "$@"; }
ok(){ if eval "$2"; then echo "PASS $1"; PASS=$((PASS+1)); else echo "FAIL $1"; FAILN=$((FAILN+1)); return 1; fi; }
j(){ python3 -c "import json,sys;d=json.load(sys.stdin);print($1)"; }
waitjobs(){ for _ in $(seq 1 30); do n=$(c $BASE/api/dashboard | j 'd["stats"]["pending"]'); [ "$n" = 0 ] && return; sleep 1; done; }

ok "health" '[ "$(curl -s $BASE/health | j d[\"status\"])" = ok ]'
ok "static index" 'curl -s $BASE/ | grep -q "<title>RE0Drive"'
ok "static js/css non-empty" '[ $(curl -s $BASE/app.js | wc -c) -gt 1000 ] && [ $(curl -s $BASE/style.css | wc -c) -gt 1000 ]'
ok "CSP header" 'curl -sI $BASE/ | grep -qi "content-security-policy"'
ok "unauth api -> 401" '[ $(curl -s -o /dev/null -w %{http_code} $BASE/api/dashboard) = 401 ]'
ok "csrf header required" '[ $(curl -s -o /dev/null -w %{http_code} -X POST -H "Content-Type: application/json" -d "{}" $BASE/api/auth/login) = 403 ]'
ok "setup admin (fresh data dir required)" 'c -X POST -d "{\"username\":\"admin\",\"password\":\"smoke-test-pw\"}" $BASE/api/auth/setup | grep -q ok' || { echo "abort: run against an empty RE0DRIVE_DATA"; exit 1; }
ok "second setup rejected" 'c -X POST -d "{\"username\":\"x\",\"password\":\"another-pw-1\"}" $BASE/api/auth/setup | grep -q error'
ok "bad login rejected" '[ $(curl -s -o /dev/null -w %{http_code} -X POST -H "X-Re0drive-Request: 1" -H "Content-Type: application/json" -d "{\"username\":\"admin\",\"password\":\"wrongwrong\"}" $BASE/api/auth/login) = 401 ]'
ok "auth status" '[ "$(c $BASE/api/auth/status | j d[\"authenticated\"])" = True ]'

# demo mode
ok "demo default" '[ "$(c $BASE/api/settings | j d[\"mode\"])" = demo ]'
ok "demo search" '[ "$(c -X POST -d "{\"query\":\"星际穿越\",\"media_type\":\"movie\"}" $BASE/api/resources/search | j "len(d[\"resources\"])")" = 3 ]'
R=$(c -X POST -d '{"query":"x","media_type":"movie"}' $BASE/api/resources/search | j 'json.dumps(d["resources"][0])')
ok "demo enqueue" 'c -X POST -d "{\"resource\":$R,\"target_cid\":\"0\",\"max_points\":4}" $BASE/api/jobs | grep -q queued'
ok "dedup" '[ "$(c -X POST -d "{\"resource\":$R,\"target_cid\":\"0\",\"max_points\":4}" $BASE/api/jobs | j d[\"duplicate\"])" = True ]'
waitjobs
ok "demo job success" 'c $BASE/api/jobs | j "d[\"jobs\"][0][\"status\"]" | grep -q success'
ok "demo checkin" 'c -X POST $BASE/api/accounts/checkin | grep -q demo'

# switch to gateway mode against the mock
ok "save gateway settings" 'c -X PUT -d "{\"mode\":\"gateway\",\"re0_base_url\":\"$MOCK\",\"install_id\":\"inst_smoke\",\"default_target_cid\":\"123\",\"default_max_points\":4,\"checkin_enabled\":false,\"re0_shared_secret\":\"mock-test-key\",\"cookie_115\":\"UID=fake; CID=fake\"}" $BASE/api/settings | grep -q "\"has_115_cookie\":true"'
ok "secrets not echoed" '! c $BASE/api/settings | grep -q -e mock-test-key -e UID=fake'
ok "secrets encrypted at rest" '! grep -a -q -e mock-test-key -e UID=fake $DATA/re0drive.db* 2>/dev/null'
ok "re0 qr start renders svg qr" 'c -X POST $BASE/api/login/re0/start > $TMP/re0qr.json && j "d[\"qr\"]" < $TMP/re0qr.json | grep -q "^data:image/svg+xml;base64,"'
ok "re0 qr waiting before scan" '[ "$(c $BASE/api/login/re0/status | j d[\"status\"])" = waiting ]'
curl -s "$(j 'd["url"]' < $TMP/re0qr.json)" >/dev/null   # simulate the phone finishing OAuth at the relay
ok "re0 qr confirmed after scan" '[ "$(c $BASE/api/login/re0/status | j d[\"status\"])" = confirmed ]'
ok "re0 status authorized" '[ "$(c $BASE/api/accounts/re0/status | j d[\"user\"][\"nickname\"])" = mock-user ]'
ok "authorize url" 'c -X POST $BASE/api/accounts/re0/authorize | grep -q "auth/start?install_id=inst_smoke"'
ok "gateway checkin" 'c -X POST $BASE/api/accounts/checkin | grep -q mock'
S=$(c -X POST -d '{"query":"tmdb:693134","media_type":"movie"}' $BASE/api/resources/search)
ok "gateway search (115 only, owned first)" '[ "$(echo "$S" | j "\",\".join(r[\"slug\"] for r in d[\"resources\"])")" = "m_owned,m_cheap,m_pricey" ]'
for slug in m_cheap m_pricey; do
  R=$(echo "$S" | j "json.dumps([r for r in d['resources'] if r['slug']=='$slug'][0])")
  c -X POST -d "{\"resource\":$R,\"target_cid\":\"123\",\"max_points\":4}" $BASE/api/jobs >/dev/null
done
waitjobs
J=$(c $BASE/api/jobs)
ok "cheap resource transferred" 'echo "$J" | j "[x for x in d[\"jobs\"] if x[\"resource\"][\"slug\"]==\"m_cheap\"][0][\"status\"]" | grep -q success'
ok "over-cap resource skipped" 'echo "$J" | j "[x for x in d[\"jobs\"] if x[\"resource\"][\"slug\"]==\"m_pricey\"][0][\"status\"]" | grep -q skipped'
CALLS=$(curl -s $MOCK/_calls)
ok "all relay signatures valid" '[ "$(echo "$CALLS" | j "all(x[\"sig_ok\"] for x in d if \"sig_ok\" in x)")" = True ]'
ok "pricey never unlocked" '! echo "$CALLS" | grep -q "m_pricey\\\\\"}"'
ok "115 receive got cookie+ids" 'echo "$CALLS" | j "[x for x in d if x[\"p\"]==\"/share/receive\"][0]" | grep -q "share_code=swmcheap&receive_code=ab12&file_id=111%2C222&cid=123"'

# 115 QR login: waiting -> scanned -> confirmed, cookie replaced and kept write-only
ok "115 qr apps list" 'c $BASE/api/login/115/apps | grep -q alipaymini'
ok "115 qr rejects unknown app" '[ $(c -o /dev/null -w %{http_code} -X POST -d "{\"app\":\"x/y\"}" $BASE/api/login/115/start) = 400 ]'
ok "115 qr start" 'c -X POST -d "{\"app\":\"alipaymini\"}" $BASE/api/login/115/start | j "d[\"uid\"]+\" \"+d[\"qr\"][:26]" | grep -q "^mockqr1 data:image/svg+xml;base64"'
ok "115 qr status sequence" '[ "$(for _ in 1 2 3; do c "$BASE/api/login/115/status?uid=mockqr1" | j d[\"status\"]; done | tr "\n" ,)" = "waiting,scanned,confirmed," ]'
ok "115 qr cookie saved, not echoed" 'S2=$(c $BASE/api/settings); echo "$S2" | grep -q "\"has_115_cookie\":true" && ! echo "$S2" | grep -q mockseid && ! grep -a -q mockseid $DATA/re0drive.db*'

# subscription auto flow
SUB=$(c -X POST -d '{"name":"Mock sub","query":"tmdb:1","media_type":"movie","target_cid":"456","max_points":4,"quality":"1080p","interval_minutes":30,"auto_transfer":true,"enabled":true}' $BASE/api/subscriptions | j 'd["id"]')
for _ in $(seq 1 15); do r=$(c $BASE/api/subscriptions | j 'd[0]["last_result"]'); [[ "$r" == 匹配* ]] && break; sleep 1; done
ok "subscription scanned" 'c $BASE/api/subscriptions | j d[0][\"last_result\"] | grep -q "匹配 1 项 · 新增 1"'
waitjobs
ok "subscription job success" 'c $BASE/api/jobs | j "[x for x in d[\"jobs\"] if x[\"subscription_id\"]==\"$SUB\"][0][\"status\"]" | grep -q success'
ok "disable subscription" 'c -X POST -d "{\"name\":\"Mock sub\",\"query\":\"tmdb:1\",\"media_type\":\"movie\",\"target_cid\":\"456\",\"max_points\":4,\"quality\":\"1080p\",\"interval_minutes\":30,\"auto_transfer\":true,\"enabled\":false}" $BASE/api/subscriptions/$SUB | grep -q "\"enabled\":false"'
ok "invalid rule rejected" '[ $(c -o /dev/null -w %{http_code} -X POST -d "{\"name\":\"x\",\"query\":\"y\",\"target_cid\":\"abc\"}" $BASE/api/subscriptions) = 400 ]'
ok "delete subscription" 'c -X DELETE $BASE/api/subscriptions/$SUB | grep -q ok'
ok "logout" 'c -X POST $BASE/api/auth/logout | grep -q ok && [ $(c -o /dev/null -w %{http_code} $BASE/api/dashboard) = 401 ]'
rm -f "$JAR"
if [ $FAILN != 0 ]; then echo "--- server log"; tail -20 "$TMP/server.log"; fi
echo "== $PASS passed, $FAILN failed"; [ $FAILN = 0 ]
