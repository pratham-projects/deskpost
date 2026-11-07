#!/usr/bin/env bash
# Backup: one pg_dump (custom format) + a tarball of the file storage dir, with 14-day retention.
#   DATABASE_URL=postgres://senddesk:...@localhost:5432/senddesk   (password is moved to PGPASSWORD, not passed in argv) STORAGE_DIR=/var/lib/senddesk/storage \
#     BACKUP_DIR=/var/backups/senddesk scripts/backup.sh
# Restore steps: docs/OPERATIONS.md. Cron example: 15 3 * * * /opt/senddesk/scripts/backup.sh
# Env: DATABASE_URL (required), STORAGE_DIR (default /var/lib/senddesk/storage), BACKUP_DIR (default ./backups),
#      KEEP_DAYS (default 14; 0 disables pruning). Docker compose: see OPERATIONS.md for the exec form.
set -euo pipefail
: "${DATABASE_URL:?set DATABASE_URL}"
STORAGE_DIR=${STORAGE_DIR:-/var/lib/senddesk/storage}
BACKUP_DIR=${BACKUP_DIR:-./backups}
KEEP_DAYS=${KEEP_DAYS:-14}
case "$KEEP_DAYS" in '' | *[!0-9]*) echo "KEEP_DAYS must be a number" >&2; exit 2 ;; esac

# Keep the password out of argv (visible in `ps`): move it from the URL into PGPASSWORD.
# A password-less URL works with ~/.pgpass or an already exported PGPASSWORD.
if [ -z "${PGPASSWORD:-}" ]; then
  PGPASSWORD=$(python3 -I -c 'import sys,urllib.parse as u; print(u.unquote(u.urlsplit(sys.argv[1]).password or ""), end="")' "$DATABASE_URL")
fi
[ -n "$PGPASSWORD" ] && export PGPASSWORD || unset PGPASSWORD
DATABASE_URL=$(python3 -I -c 'import sys,urllib.parse as u
p=u.urlsplit(sys.argv[1]); n=p.netloc.rpartition("@")[2]
n=(p.username and u.quote(u.unquote(p.username), safe="")+"@" or "")+n
print(u.urlunsplit((p.scheme,n,p.path,p.query,p.fragment)), end="")' "$DATABASE_URL")

umask 077 # dumps contain customer data and sealed credentials
mkdir -p "$BACKUP_DIR"
ts=$(date -u +%Y%m%dT%H%M%SZ)
db_out="$BACKUP_DIR/senddesk-db-$ts.dump"
files_out="$BACKUP_DIR/senddesk-files-$ts.tar.gz"

# A dump from a newer pg_dump than the server cannot be restored onto that server (e.g. pg_dump 18 -> PG 16).
client_major=$(pg_dump --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')
server_major=$(psql "$DATABASE_URL" -tAc 'SHOW server_version_num' | cut -c1-2)
if [ "$client_major" != "$server_major" ]; then
  echo "pg_dump is v$client_major but the server is v$server_major: use matching client tools" >&2
  exit 3
fi

# Write to .part and rename, so a failed run never leaves something that looks like a good backup.
trap 'rm -f "$db_out.part" "$files_out.part"' EXIT
pg_dump --format=custom --no-owner --dbname="$DATABASE_URL" --file="$db_out.part"
pg_restore --list "$db_out.part" >/dev/null # the dump must be readable
mv "$db_out.part" "$db_out"

if [ -d "$STORAGE_DIR" ]; then
  tar -C "$STORAGE_DIR" --exclude=./.tmp -czf "$files_out.part" .
  mv "$files_out.part" "$files_out"
else
  echo "warning: STORAGE_DIR $STORAGE_DIR not found, skipping files" >&2
  files_out=
fi

if [ "$KEEP_DAYS" -gt 0 ]; then
  find "$BACKUP_DIR" -maxdepth 1 -type f \( -name 'senddesk-db-*.dump' -o -name 'senddesk-files-*.tar.gz' \) \
    -mtime +"$KEEP_DAYS" -delete
fi
echo "backup ok: $db_out${files_out:+ $files_out}"
