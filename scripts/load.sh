#!/usr/bin/env bash
# Load test: scratch db + release api, seed data over the API, then oha against the hot endpoints.
# Needs: postgres on :5433, oha, jq, openssl. Results: $RUN/load.md (copy numbers to docs/PERFORMANCE.md).
# Knobs: LOAD_SECS (10), LOAD_CONC (32), LOAD_LOGIN_CONC (8), LOAD_TICKETS (500).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH

BASE_DB=${LOAD_DATABASE_URL:-${DATABASE_URL:-postgres://pratham@localhost:5433/postgres}}
case "$BASE_DB" in
  *@localhost[:/]*|*@127.0.0.1[:/]*|*//localhost[:/]*|*//127.0.0.1[:/]*) ;;
  *) echo "refusing non-local database host" >&2; exit 1 ;;
esac
DB_NAME=dp_load
DB_URL="${BASE_DB%/*}/$DB_NAME"
PORT=${LOAD_API_PORT:-3200}
SECS=${LOAD_SECS:-10}
CONC=${LOAD_CONC:-32}
LOGIN_CONC=${LOAD_LOGIN_CONC:-8}
TICKETS=${LOAD_TICKETS:-500}
RUN=e2e/.run
ADMIN_EMAIL=admin@load.test
ADMIN_PASS=load-admin-pass-1
BASE=http://127.0.0.1:$PORT
API_URL=$BASE/api/v3

umask 077
mkdir -p "$RUN"
API_PID=
cleanup() { [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT
trap 'exit 130' INT TERM
if nc -z 127.0.0.1 "$PORT" 2>/dev/null; then echo "port $PORT busy" >&2; exit 1; fi

echo "== scratch database $DB_NAME"
psql "$BASE_DB" -q -c "DROP DATABASE IF EXISTS $DB_NAME" -c "CREATE DATABASE $DB_NAME"

echo "== build api (release)"
cargo build -q --release -p deskpost
API=$ROOT/target/release/deskpost

export DATABASE_URL=$DB_URL
export JWT_SECRET=$(openssl rand -hex 32) DP_SECRET_KEY=$(openssl rand -hex 32)
export DP_ENV=development PORT=$PORT FRONTEND_URL=$BASE DP_TRUSTED_PROXIES=127.0.0.1
export STORAGE_DIR=$ROOT/$RUN/load-storage DP_WEBHOOK_SECRET=$(openssl rand -hex 32)
# every request comes from one address: lift the per-address quotas so the server is measured, not the limiter
export DP_RATE_LIMIT_SCALE=100000
rm -rf "$STORAGE_DIR"
"$API" migrate >/dev/null
DP_BOOTSTRAP_PASSWORD=$ADMIN_PASS "$API" bootstrap --email $ADMIN_EMAIL --name "Load Admin" >/dev/null
"$API" serve >"$RUN/load-api.log" 2>&1 &
API_PID=$!
for _ in $(seq 1 60); do nc -z 127.0.0.1 "$PORT" 2>/dev/null && break; sleep 0.5; done
nc -z 127.0.0.1 "$PORT" || { echo "api did not start; see $RUN/load-api.log" >&2; exit 1; }

echo "== seed ($TICKETS tickets)"
TOKEN=$(curl -fsS -H 'content-type: application/json' -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" "$API_URL/auth/signin" | jq -r .data.accessToken)
AUTH="Authorization: Bearer $TOKEN"
PROJ=$(curl -fsS -H "$AUTH" -H 'content-type: application/json' -d '{"name":"Load","slug":"load-test"}' "$API_URL/projects")
PID=$(jq -r .data.id <<<"$PROJ")
KEY=$(jq -r .data.apiKey <<<"$PROJ")
TID=
for i in $(seq 1 "$TICKETS"); do
  TID=$(curl -fsS -H "X-Project-Key: $KEY" -F projectId="$PID" -F clientEmail="c$((i % 50))@load.test" \
    -F subject="Seed ticket $i" -F description="Seeded body $i" "$API_URL/tickets" | jq -r .data.id)
  if [ $((i % 5)) -eq 0 ]; then
    curl -fsS -o /dev/null -H "$AUTH" -H 'content-type: application/json' -d '{"body":"seed reply","sendEmail":false}' "$API_URL/tickets/$TID/messages"
  fi
done

rss() { ps -o rss= -p "$API_PID" | awk '{printf "%.0f", $1/1024}'; }
OUT=$RUN/load.md
{
  echo "# Load results $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "host: $(sysctl -n machdep.cpu.brand_string 2>/dev/null) ($(sysctl -n hw.ncpu) cpu); ${SECS}s per scenario, conc $CONC (login $LOGIN_CONC), $TICKETS seeded tickets, release build"
  echo "api rss before: $(rss) MB"
  echo
  echo "| scenario | rps | p50 ms | p99 ms | 2xx | non-2xx/errors |"
  echo "|---|---|---|---|---|---|"
} >"$OUT"

run() { # name conc oha-args...
  local name=$1 c=$2; shift 2
  local j
  j=$(oha -z "${SECS}s" -w -c "$c" --no-tui --output-format json "$@")
  jq -r --arg n "$name" '[ $n,
      (.summary.requestsPerSec|floor),
      ((.latencyPercentiles.p50*1000)|.*10|floor/10),
      ((.latencyPercentiles.p99*1000)|.*10|floor/10),
      ([.statusCodeDistribution|to_entries[]|select(.key|startswith("2"))|.value]|add // 0),
      (([.statusCodeDistribution|to_entries[]|select(.key|startswith("2")|not)|.value]|add // 0) + ([.errorDistribution|to_entries[]|.value]|add // 0))
    ] | "| " + (map(tostring)|join(" | ")) + " |"' <<<"$j" >>"$OUT"
  echo "done: $name"
}

H_AUTH=(-H "$AUTH")
run login "$LOGIN_CONC" -m POST -H 'content-type: application/json' \
  -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" "$API_URL/auth/signin"
run "ticket list (50)" "$CONC" "${H_AUTH[@]}" "$API_URL/tickets?projectId=$PID&limit=50"
run "ticket search" "$CONC" "${H_AUTH[@]}" "$API_URL/tickets/search?q=seed&projectId=$PID"
run "ticket counts" "$CONC" "${H_AUTH[@]}" "$API_URL/tickets/counts"
run "ticket get" "$CONC" "${H_AUTH[@]}" "$API_URL/tickets/$TID"
run "ticket messages" "$CONC" "${H_AUTH[@]}" "$API_URL/tickets/$TID/messages?limit=50"
run "widget ticket create" "$CONC" -m POST -H "X-Project-Key: $KEY" -F projectId="$PID" \
  -F clientEmail=load@load.test -F subject="Load ticket" -F description="Load body" "$API_URL/tickets"
# spread over the seeded tickets (one hot row would only measure that row's lock); no SMTP here, so skip mail
psql "$DB_URL" -Atc "select '$API_URL/tickets/' || id || '/messages' from tickets order by created_at limit $TICKETS" >"$RUN/load-urls.txt"
run "message post (spread)" "$CONC" -m POST "${H_AUTH[@]}" -H 'content-type: application/json' \
  -d '{"body":"load reply","sendEmail":false}' --urls-from-file "$RUN/load-urls.txt"
run "message post (one ticket)" "$CONC" -m POST "${H_AUTH[@]}" -H 'content-type: application/json' \
  -d '{"body":"load reply","sendEmail":false}' "$API_URL/tickets/$TID/messages"
run "health" "$CONC" "$BASE/health"

echo >>"$OUT"
echo "api rss after: $(rss) MB" >>"$OUT"
echo "api log errors: $(grep -ciE 'error|panic' "$RUN/load-api.log" || true)" >>"$OUT"
cat "$OUT"
