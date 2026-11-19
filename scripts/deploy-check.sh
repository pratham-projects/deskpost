#!/usr/bin/env bash
# Static + local checks for the deploy files (no Docker needed): `make deploy-check`.
# Caddyfiles validate, compose/.env.example/systemd agree with config.rs, backup -> restore round-trips.
# Needs caddy, python3 with PyYAML, and (for the backup test) a local postgres via E2E_DATABASE_URL/DATABASE_URL.
set -euo pipefail
cd "$(dirname "$0")/.."
for f in deploy/Caddyfile deploy/web.Caddyfile; do
  caddy validate --config "$f" --adapter caddyfile >/dev/null 2>&1 || { echo "FAIL caddy validate $f" >&2; exit 1; }
  [ -z "$(caddy fmt --diff "$f" 2>/dev/null | grep -E "^[+-]")" ] || { echo "FAIL caddy fmt $f (run: caddy fmt --overwrite $f)" >&2; exit 1; }
  echo "ok caddy $f"
done

python3 - <<'PY'
import re, sys, yaml
cfg = open("crates/api/src/config.rs").read() + open("crates/api/src/mail_config.rs").read()
read = set(re.findall(r'"((?:DP_)?[A-Z][A-Z_]{3,})"', cfg))
fixed_by_image = {"PORT", "STORAGE_DIR"}  # set in the Dockerfile
compose = yaml.safe_load(open("docker-compose.yml"))
errs = []
for svc in ("migrate", "app"):
    env = set(compose["services"][svc]["environment"])
    errs += [f"compose {svc}: unknown var {v}" for v in env - read - {"DATABASE_URL", "RUST_LOG"}]
    errs += [f"compose {svc}: config.rs reads {v} but compose does not pass it" for v in read - env - fixed_by_image]
example = set(re.findall(r"^([A-Z][A-Z_]+)=", open(".env.example").read(), re.M))
errs += [f".env.example lacks {v}" for v in read - example]
errs += [f".env.example has unknown {v}" for v in example - read - {"DP_BOOTSTRAP_PASSWORD", "POSTGRES_PASSWORD", "RUST_LOG", "PUBLIC_URL"}]
unit = open("deploy/deskpost-api.service").read()
for needle in ("User=deskpost", "EnvironmentFile=/etc/deskpost/env", "ExecStart=/usr/local/bin/deskpost serve", "ReadWritePaths=/var/lib/deskpost"):
    if needle not in unit: errs.append(f"systemd unit lacks {needle}")
for sec in ("[Unit]", "[Service]", "[Install]"):
    if sec not in unit: errs.append(f"systemd unit lacks {sec}")
df = open("Dockerfile").read()
if "target/release/deskpost" not in df or 'ENTRYPOINT ["deskpost"]' not in df: errs.append("Dockerfile api binary path/entrypoint")
if "COPY --from=web-build /web/out /srv" not in df: errs.append("Dockerfile web static copy")
if "root * /srv\n" not in open("deploy/web.Caddyfile").read(): errs.append("web.Caddyfile root must match Dockerfile /srv")
if errs:
    print("\n".join("FAIL " + e for e in errs), file=sys.stderr); sys.exit(1)
print("ok compose/.env.example/systemd/Dockerfile agree with config.rs")
PY

BASE=${E2E_DATABASE_URL:-${DATABASE_URL:-postgres://pratham@localhost:5433/postgres}}
case "$BASE" in
  *@localhost[:/]*|*@127.0.0.1[:/]*|*//localhost[:/]*|*//127.0.0.1[:/]*) ;;
  *) echo "skip backup test: non-local database" >&2; exit 0 ;;
esac
W=$(mktemp -d)
trap 'psql "$BASE" -q -c "DROP DATABASE IF EXISTS dp_bk_src" -c "DROP DATABASE IF EXISTS dp_bk_dst" >/dev/null 2>&1; rm -rf "$W"' EXIT
psql "$BASE" -q -c "DROP DATABASE IF EXISTS dp_bk_src" -c "CREATE DATABASE dp_bk_src" -c "DROP DATABASE IF EXISTS dp_bk_dst" -c "CREATE DATABASE dp_bk_dst"
SRC="${BASE%/*}/dp_bk_src"; DST="${BASE%/*}/dp_bk_dst"
psql "$SRC" -q -c "CREATE TABLE t(id int primary key, v text)" -c "INSERT INTO t VALUES (1,'a'),(2,'b')"
mkdir -p "$W/storage/ab/cd" && echo data >"$W/storage/ab/cd/abcdfile"
DATABASE_URL=$SRC STORAGE_DIR=$W/storage BACKUP_DIR=$W/bk scripts/backup.sh >/dev/null
pg_restore --no-owner --exit-on-error --dbname="$DST" "$W"/bk/deskpost-db-*.dump
[ "$(psql "$DST" -tAc 'select count(*) from t')" = 2 ] || { echo "FAIL restore row count" >&2; exit 1; }
tar -tzf "$W"/bk/deskpost-files-*.tar.gz | grep -q abcdfile || { echo "FAIL files tarball" >&2; exit 1; }
[ "$(stat -c %a "$W"/bk/deskpost-db-*.dump 2>/dev/null || stat -f %Lp "$W"/bk/deskpost-db-*.dump)" = 600 ] || { echo "FAIL dump mode" >&2; exit 1; }
echo "ok backup -> restore round-trip"
