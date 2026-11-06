#!/usr/bin/env bash
# Full-stack e2e: postgres (scratch db) + GreenMail (SMTP/IMAP) + api + static web behind Caddy + Playwright.
# Needs: postgres on :5433, java, caddy, bun, node, Google Chrome (or set E2E_CHROME_CHANNEL).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH

BASE_DB=${E2E_DATABASE_URL:-${DATABASE_URL:-postgres://pratham@localhost:5433/postgres}}
# Creates and drops a scratch db: refuse anything that is not a local server.
case "$BASE_DB" in
  *@localhost[:/]*|*@127.0.0.1[:/]*|*//localhost[:/]*|*//127.0.0.1[:/]*) ;;
  *) echo "refusing non-local database host in E2E_DATABASE_URL/DATABASE_URL" >&2; exit 1 ;;
esac
DB_NAME=sd_e2e
DB_URL="${BASE_DB%/*}/$DB_NAME"
API_PORT=${E2E_API_PORT:-3100}
WEB_PORT=${E2E_WEB_PORT:-4100}
SMTP_PORT=3025
IMAP_PORT=3143
GREENMAIL_VERSION=2.1.15
GREENMAIL_SHA256=91d58798698cac357f015365240383b936ced2f2a1a9f7db37d9e3e1a31391b1
JAR=.tools/greenmail-standalone-$GREENMAIL_VERSION.jar
RUN=e2e/.run
ADMIN_EMAIL=admin@e2e.test
ADMIN_PASS=e2e-admin-pass-1

umask 077
mkdir -p .tools "$RUN"
PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  pkill -P $$ 2>/dev/null || true # playwright/browser children
  wait 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

for port in $API_PORT $WEB_PORT $SMTP_PORT $IMAP_PORT; do
  if nc -z 127.0.0.1 "$port" 2>/dev/null; then echo "port $port busy" >&2; exit 1; fi
done
wait_port() { # name port
  for _ in $(seq 1 60); do nc -z 127.0.0.1 "$2" 2>/dev/null && return 0; sleep 0.5; done
  echo "$1 did not start (port $2); see $RUN/$1.log" >&2; exit 1
}

echo "== scratch database $DB_NAME"
psql "$BASE_DB" -q -c "DROP DATABASE IF EXISTS $DB_NAME" -c "CREATE DATABASE $DB_NAME"

echo "== greenmail"
if [ ! -f "$JAR" ]; then
  curl -fsSL -o "$JAR.tmp" "https://repo1.maven.org/maven2/com/icegreen/greenmail-standalone/$GREENMAIL_VERSION/greenmail-standalone-$GREENMAIL_VERSION.jar"
  echo "$GREENMAIL_SHA256  $JAR.tmp" | shasum -a 256 -c - >/dev/null || { rm -f "$JAR.tmp"; echo "greenmail checksum mismatch" >&2; exit 1; }
  mv "$JAR.tmp" "$JAR"
fi
echo "$GREENMAIL_SHA256  $JAR" | shasum -a 256 -c - >/dev/null || { echo "cached greenmail jar checksum mismatch: delete $JAR" >&2; exit 1; }
java -Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap -Dgreenmail.hostname=127.0.0.1 \
  -Dgreenmail.auth.disabled -jar "$JAR" >"$RUN/greenmail.log" 2>&1 &
PIDS+=($!)
wait_port greenmail $SMTP_PORT
wait_port greenmail $IMAP_PORT

echo "== build api + web"
cargo build -q -p api
API=$ROOT/target/debug/api
if [ ! -f apps/web/out/index.html ] || [ -n "${E2E_REBUILD_WEB:-}" ]; then
  (cd apps/web && bun install --frozen-lockfile >/dev/null && bun run build >/dev/null)
fi

echo "== api"
export DATABASE_URL=$DB_URL
export JWT_SECRET=$(openssl rand -hex 32) SD_SECRET_KEY=$(openssl rand -hex 32)
export SD_ENV=development PORT=$API_PORT FRONTEND_URL=http://127.0.0.1:$WEB_PORT
export STORAGE_DIR=$ROOT/$RUN/storage SD_ALLOW_PRIVATE_MAIL_HOSTS=true SD_MAIL_PLAINTEXT=true
export SD_IMAP_POLL_SECS=5 SD_TRUSTED_PROXIES=127.0.0.1
export SD_WEBHOOK_SECRET=$(openssl rand -hex 32)
rm -rf "$STORAGE_DIR"
"$API" migrate >/dev/null
SD_BOOTSTRAP_PASSWORD=$ADMIN_PASS "$API" bootstrap --email $ADMIN_EMAIL --name "E2E Admin" >/dev/null
"$API" serve >"$RUN/api.log" 2>&1 &
PIDS+=($!)
wait_port api $API_PORT

echo "== web (caddy)"
cat >"$RUN/Caddyfile" <<CADDY
{
	admin off
	auto_https off
	# keep the test-supplied X-Forwarded-For so each spec gets its own rate-limit bucket
	servers {
		trusted_proxies static 127.0.0.1/8
	}
}
http://127.0.0.1:$WEB_PORT {
	@api path /api/* /health /health/*
	handle @api {
		reverse_proxy 127.0.0.1:$API_PORT
	}
	handle {
		root * $ROOT/apps/web/out
		file_server
	}
}
CADDY
caddy run --config "$RUN/Caddyfile" --adapter caddyfile >"$RUN/caddy.log" 2>&1 &
PIDS+=($!)
wait_port caddy $WEB_PORT

echo "== playwright"
(cd e2e && bun install --frozen-lockfile >/dev/null 2>&1 || bun install >/dev/null)
export E2E_BASE_URL=http://127.0.0.1:$WEB_PORT E2E_ADMIN_EMAIL=$ADMIN_EMAIL E2E_ADMIN_PASS=$ADMIN_PASS
export E2E_SMTP_PORT=$SMTP_PORT E2E_IMAP_PORT=$IMAP_PORT E2E_WEBHOOK_SECRET=$SD_WEBHOOK_SECRET
set +e
(cd e2e && bunx playwright test)
rc=$?
if [ $rc -ne 0 ]; then echo "--- api log tail"; tail -30 "$RUN/api.log"; fi
exit $rc
