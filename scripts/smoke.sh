#!/usr/bin/env bash
# Post-deploy smoke test. Run it against the public URL (through the reverse proxy) on a real host,
# or against a local Caddy + api.  Read-only apart from one rejected oversize POST.
#
#   scripts/smoke.sh https://desk.example.com
#   BASE_URL=https://localhost:8443 INSECURE=1 scripts/smoke.sh      # local Caddy with `tls internal`
#
# Env: BASE_URL (or $1)   public origin, default https://localhost:8443
#      API_VERSION        default v3
#      ORIGIN             allowed CORS origin, default = BASE_URL (must match FRONTEND_URL)
#      INSECURE=1         curl -k (self-signed / internal CA)
#      BODY_LIMIT_BYTES   api DP_BODY_LIMIT_BYTES, default 65536 (a bigger JSON body must give 413)
#      PROXY_LIMIT_BYTES  proxy cap, default 41943040 (40MB); set SMOKE_BIG=1 to send one oversize body
#      EXPECT_HSTS        default auto (on for https); set 0/1 to force
# Exit code is the number of failed checks (0 = healthy).
set -uo pipefail

BASE_URL=${1:-${BASE_URL:-https://localhost:8443}}
BASE_URL=${BASE_URL%/}
API_VERSION=${API_VERSION:-v3}
ORIGIN=${ORIGIN:-$BASE_URL}
BODY_LIMIT_BYTES=${BODY_LIMIT_BYTES:-65536}
PROXY_LIMIT_BYTES=${PROXY_LIMIT_BYTES:-41943040}
CURL=(curl -sS --max-time 15)
[ -n "${INSECURE:-}" ] && CURL+=(-k)
case "$BASE_URL" in https://*) HSTS_DEFAULT=1 ;; *) HSTS_DEFAULT=0 ;; esac
EXPECT_HSTS=${EXPECT_HSTS:-$HSTS_DEFAULT}

FAIL=0
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
ok() { printf 'ok    %s\n' "$1"; }
bad() { printf 'FAIL  %s\n' "$1"; FAIL=$((FAIL + 1)); }

# status <url> [curl args...] -> prints http code; headers -> $TMP/h, body -> $TMP/b
status() {
  local url=$1
  shift
  "${CURL[@]}" -o "$TMP/b" -D "$TMP/h" -w '%{http_code}' "$@" "$url" 2>"$TMP/err" || echo "000"
}
# header <name> -> value of the last response header (case-insensitive), trimmed
header() { tr -d '\r' <"$TMP/h" | awk -v n="$(printf '%s' "$1" | tr 'A-Z' 'a-z')" -F': ' 'tolower($1)==n {sub(/^[^:]*: */,""); v=$0} END {print v}'; }
expect_status() { # name want url [curl args]
  local name=$1 want=$2 url=$3
  shift 3
  local got
  got=$(status "$url" "$@")
  if [ "$got" = "$want" ]; then ok "$name ($got)"; else bad "$name: want $want got $got $(head -c 200 "$TMP/err")"; fi
}
expect_header() { # name header regex
  local v
  v=$(header "$2")
  if printf '%s' "$v" | grep -Eiq -- "$3"; then ok "$1"; else bad "$1: header '$2' = '${v:-<absent>}'"; fi
}
expect_no_header() { # name header
  local v
  v=$(header "$2")
  if [ -z "$v" ]; then ok "$1"; else bad "$1: header '$2' present: $v"; fi
}

echo "smoke: $BASE_URL (api $API_VERSION, origin $ORIGIN)"

# --- health
expect_status "GET /health" 200 "$BASE_URL/health"
expect_status "GET /health/db" 200 "$BASE_URL/health/db"

# --- api through the proxy
expect_status "GET /api/$API_VERSION/auth/config" 200 "$BASE_URL/api/$API_VERSION/auth/config"
if head -c1 "$TMP/b" | grep -q '{'; then ok "auth/config is JSON"; else bad "auth/config is not JSON"; fi
expect_header "api content-type json" content-type 'application/json'
expect_status "unknown api route is not the SPA" 404 "$BASE_URL/api/$API_VERSION/does-not-exist"
if grep -qi '<html' "$TMP/b"; then bad "unknown api route returned HTML"; else ok "unknown api route not HTML"; fi
expect_status "protected route without token" 401 "$BASE_URL/api/$API_VERSION/users/me"

# --- static UI
expect_status "GET / (static index)" 200 "$BASE_URL/"
expect_header "index is html" content-type 'text/html'
if grep -q '_next/' "$TMP/b"; then ok "index references _next assets"; else bad "index does not look like the Next export"; fi
expect_status "GET /settings/ (client route)" 200 "$BASE_URL/settings/"
expect_status "GET /widget/ (embeddable)" 200 "$BASE_URL/widget/"
expect_no_header "widget frameable (no X-Frame-Options)" x-frame-options
expect_header "widget Referrer-Policy no-referrer" referrer-policy '^no-referrer$'
# first _next asset the index links to must be served with a js/css type
asset=$(status "$BASE_URL/" >/dev/null; grep -oE '/_next/static/[^"\\]+\.(js|css)' "$TMP/b" | head -1)
if [ -n "$asset" ]; then expect_status "static asset $asset" 200 "$BASE_URL$asset"; else bad "no _next asset found in index"; fi

# --- security headers on the UI
status "$BASE_URL/" >/dev/null
expect_header "X-Frame-Options DENY" x-frame-options '^DENY$'
expect_header "X-Content-Type-Options nosniff" x-content-type-options '^nosniff$'
expect_header "CSP frame-ancestors none" content-security-policy "frame-ancestors 'none'"
expect_header "CSP report-only default-src self" content-security-policy-report-only "default-src 'self'"
expect_header "Referrer-Policy" referrer-policy '.'
expect_header "Permissions-Policy" permissions-policy 'camera=\(\)'
expect_no_header "no Server banner" server
if [ "$EXPECT_HSTS" = 1 ]; then expect_header "HSTS" strict-transport-security 'max-age=[0-9]{7,}'; else ok "HSTS skipped (plain http)"; fi
# nosniff must also cover api responses
status "$BASE_URL/api/$API_VERSION/auth/config" >/dev/null
expect_header "nosniff on api" x-content-type-options '^nosniff$'

# --- CORS: configured origin allowed, others not
got=$(status "$BASE_URL/api/$API_VERSION/auth/config" -X OPTIONS -H "Origin: $ORIGIN" \
  -H 'Access-Control-Request-Method: POST' -H 'Access-Control-Request-Headers: content-type,authorization')
if [ "$(header access-control-allow-origin)" = "$ORIGIN" ]; then ok "CORS allows $ORIGIN (preflight $got)"; else bad "CORS preflight for $ORIGIN: acao='$(header access-control-allow-origin)' ($got)"; fi
status "$BASE_URL/api/$API_VERSION/auth/config" -X OPTIONS -H 'Origin: https://evil.example' \
  -H 'Access-Control-Request-Method: POST' >/dev/null
if [ -z "$(header access-control-allow-origin)" ]; then ok "CORS denies https://evil.example"; else bad "CORS leaks to evil origin: '$(header access-control-allow-origin)'"; fi
status "$BASE_URL/api/$API_VERSION/auth/config" -H 'Origin: https://evil.example' >/dev/null
if [ "$(header access-control-allow-origin)" = "*" ]; then bad "wildcard CORS on simple request"; else ok "no wildcard CORS"; fi

# --- body limits
head -c $((BODY_LIMIT_BYTES + 1024)) /dev/zero | tr '\0' 'a' >"$TMP/big"
printf '{"email":"a@b.co","password":"' >"$TMP/login"
cat "$TMP/big" >>"$TMP/login"
printf '"}' >>"$TMP/login"
expect_status "oversize JSON body rejected" 413 "$BASE_URL/api/$API_VERSION/auth/signin" \
  -X POST -H 'content-type: application/json' --data-binary @"$TMP/login"
if [ -n "${SMOKE_BIG:-}" ]; then
  head -c $((PROXY_LIMIT_BYTES + 1048576)) /dev/zero >"$TMP/huge"
  expect_status "body over proxy cap rejected" 413 "$BASE_URL/api/$API_VERSION/auth/signin" \
    -X POST -H 'content-type: application/json' --data-binary @"$TMP/huge"
else
  ok "proxy cap check skipped (SMOKE_BIG=1 to run)"
fi

echo
if [ "$FAIL" -eq 0 ]; then echo "smoke: all checks passed"; else echo "smoke: $FAIL check(s) failed"; fi
exit "$FAIL"
