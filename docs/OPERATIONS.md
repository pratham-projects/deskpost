# SendDesk operations

Runbook for a single-host deploy: Postgres 16, the `api` binary (systemd or compose), Caddy in front. Everything here was replayed by hand on a machine without Docker (release build, static export, Caddy, smoke script, backup and restore). The container path itself (`docker build`) is not exercised in CI.

## Layout

| Piece | Where | Notes |
| --- | --- | --- |
| API binary | `/usr/local/bin/api` | `cargo build --release -p api --locked`, copy `target/release/api` |
| Env file | `/etc/senddesk/env` (`chmod 600`, root) | `.env.example` lists every variable. Set `STORAGE_DIR=/var/lib/senddesk/storage` here: systemd lets the env file override the unit's `Environment=`, and the `.env.example` default `./data/storage` is read-only under `ProtectSystem=strict` |
| File storage | `/var/lib/senddesk/storage` (`STORAGE_DIR`) | attachments, `<root>/ab/cd/<key>` plus `.json` sidecars, `.tmp/` for uploads in flight |
| Static UI | `/srv/senddesk/web` | `cd apps/web && NEXT_PUBLIC_API_URL= NEXT_PUBLIC_API_VERSION=v3 bun run build`, copy `out/` |
| Proxy | `deploy/Caddyfile` | one origin: `/api/*`, `/health*` go to `127.0.0.1:3000`, the rest is the static UI |
| Unit | `deploy/senddesk-api.service` | needs the `senddesk` user |

The API binds `0.0.0.0:$PORT` (no bind-address setting). On a host install, block port 3000 from outside with the firewall so clients cannot skip Caddy (and spoof client IPs, see Trusted proxies). Compose already publishes it on `127.0.0.1` only.

Build `NEXT_PUBLIC_API_VERSION` must equal the api's `API_VERSION`, and `NEXT_PUBLIC_API_URL` stays empty (same origin).

## First install (host)

```bash
useradd --system --home /var/lib/senddesk --shell /usr/sbin/nologin senddesk
install -d -o root -g root -m 755 /etc/senddesk
# systemd does not strip trailing `# comments` from values (compose does): strip them when copying
sed -E '/^[[:space:]]*#/d; s/[[:space:]]+#.*$//; /^[[:space:]]*$/d' .env | install -m 600 /dev/stdin /etc/senddesk/env
install -m 755 target/release/api /usr/local/bin/api
install -m 644 deploy/senddesk-api.service /etc/systemd/system/
(set -a; . /etc/senddesk/env; set +a; sudo -E -u senddesk /usr/local/bin/api migrate)
systemctl daemon-reload && systemctl enable --now senddesk-api
install -d /srv/senddesk/web && cp -R apps/web/out/. /srv/senddesk/web/
# edit the domain in deploy/Caddyfile, then:
caddy validate --config deploy/Caddyfile --adapter caddyfile && systemctl reload caddy
scripts/smoke.sh https://desk.example.com
```

Compose: `cp .env.example .env`, fill secrets, `docker compose up --build -d`. `migrate` runs before `app`. The `web` service serves the UI on `127.0.0.1:4000` (plain HTTP, no security headers); put TLS and `deploy/Caddyfile`-style headers in front of it, or use the host layout above.

## First admin

Two ways, pick one:

1. **Bootstrap (recommended).** Pass the password by environment, never as an argument (arguments show up in `ps`):
   ```bash
   SD_BOOTSTRAP_PASSWORD='...' api bootstrap --email you@example.com --name "Your Name"
   # compose
   docker compose run --rm -e SD_BOOTSTRAP_PASSWORD app bootstrap --email you@example.com --name "Your Name"
   ```
   Public signup is off by default (`SD_ALLOW_SIGNUP=false`; `GET /api/v3/auth/config` reports `signupEnabled`), so bootstrap is the only way in until an admin creates users. Remove `SD_BOOTSTRAP_PASSWORD` from the env file afterwards.
2. `SD_FIRST_USER_ADMIN=true` together with `SD_ALLOW_SIGNUP=true`: the first account to sign up becomes global admin. Racy on a public host (whoever signs up first wins); use it only on a closed network and turn it off again.

## Trusted proxies

`SD_TRUSTED_PROXIES` is a comma-separated list of IPs/CIDRs. The api only believes `X-Forwarded-For` from those peers; everything else is the TCP peer address. Rate limits and audit IPs depend on it.

- Caddy on the same host: `127.0.0.1`.
- Caddy in a container, or compose `web` service: the container network address or its CIDR (`docker network inspect <project>_default`), not `127.0.0.1`.
- Empty: the api logs a warning at startup. Behind a proxy every client then shares one rate-limit bucket.
- Never list a range that clients can reach directly.

## Mailbox approval (inbound webhook routing)

Inbound mail pushed to `POST /api/v3/incoming-email` (signed with `SD_WEBHOOK_SECRET`; unset disables the webhook) is routed to the project whose `smtpEmail` equals the recipient. A tenant cannot prove ownership of an address, so a **global admin** approves it:

```
POST /api/v3/projects/{id}/mailbox/approve   {"smtpEmail": "support@customer.com"}
```

- Send the address you reviewed; `409` means it changed since, re-check and retry.
- Changing the project's address or mail servers clears the approval.
- IMAP polling needs no approval (the credentials prove ownership). Poll interval `SD_IMAP_POLL_SECS`, concurrency `SD_IMAP_CONCURRENCY`.
- Local mail servers (mailpit, greenmail) need `SD_ALLOW_PRIVATE_MAIL_HOSTS=true`. Keep it `false` in production.

## Backups

`scripts/backup.sh` writes a custom-format `pg_dump` and a tarball of `STORAGE_DIR` into `BACKUP_DIR` (mode 600, 14-day retention, `.part` files renamed only on success). It refuses to run when `pg_dump` and the server have different major versions: a dump from a newer client does not restore onto an older server.

```bash
DATABASE_URL=postgres://senddesk:...@localhost:5432/senddesk STORAGE_DIR=/var/lib/senddesk/storage \
  BACKUP_DIR=/var/backups/senddesk scripts/backup.sh
# cron (as a user that can read the env file): 15 3 * * * set -a; . /etc/senddesk/env; set +a; BACKUP_DIR=/var/backups/senddesk /opt/senddesk/scripts/backup.sh
```

Compose (volumes `pgdata`, `storage`):

```bash
docker compose exec -T postgres pg_dump -U senddesk --format=custom --no-owner senddesk > senddesk-db.dump
docker compose run --rm --no-deps -v "$PWD:/b" --entrypoint tar app -C /var/lib/senddesk/storage -czf /b/files.tar.gz .
```

Copy backups off the host; a backup on the same disk is not a backup. The dump holds customer data and sealed (encrypted) SMTP/IMAP passwords, so protect it like the database. It is only restorable together with the `SD_SECRET_KEY` it was made under: store that key separately.

### Restore

```bash
systemctl stop senddesk-api
createdb -h localhost -U postgres senddesk_new
pg_restore --no-owner --exit-on-error --dbname=postgres://senddesk:...@localhost/senddesk_new senddesk-db-<ts>.dump
# swap: rename databases (or point DATABASE_URL at senddesk_new), then
tar -C /var/lib/senddesk/storage -xzf senddesk-files-<ts>.tar.gz && chown -R senddesk: /var/lib/senddesk
api migrate            # no-op when the dump is current; applies newer migrations otherwise
systemctl start senddesk-api && scripts/smoke.sh https://desk.example.com
```

Verified: dump taken with PG 16 tools restores into an empty PG 16 database with `--exit-on-error`. Test a restore after the first backup and then quarterly. Files and database drift if restored from different times: attachments with no row are orphaned (harmless), rows with no file return 404 on download.

## Upgrades and migrations

1. Back up (above).
2. Build the new binary and static UI. Read the changelog in `brain/log.md` for env changes; diff `.env.example` against `/etc/senddesk/env`.
3. `api migrate`. Migrations are forward-only sqlx migrations and are idempotent. There are no down migrations: rollback means restoring the backup taken in step 1 and the old binary.
4. Replace `/usr/local/bin/api`, `systemctl restart senddesk-api` (graceful shutdown on SIGTERM), replace `/srv/senddesk/web`, `scripts/smoke.sh`.
5. Run the new binary against the migrated schema only after migrating: an old binary on a newer schema is not supported.

The api and UI are versioned together (`API_VERSION`); deploy both in one step. The service restarts in a second or two; a hard-cached old UI tab gets API errors until reload.

Compose: `docker compose up --build -d` reruns `migrate` before swapping `app`.

## Health, logs, monitoring

- `GET /health`: liveness, `200 {"success":true,"data":{"status":"ok"}}`. Used by the Docker `HEALTHCHECK` and Caddy's active check.
- `GET /health/db`: readiness, runs `SELECT 1` with a 2 s timeout, `503` when Postgres is down. Alert on this one.
- Logs go to stdout/stderr in `tracing` text format: `journalctl -u senddesk-api -f`, `docker compose logs -f app`. Request lines carry the span `http{method=GET path=/api/v3/...}`; query strings are deliberately never logged (download tokens). Secrets and `DATABASE_URL` are redacted from config logging.
- `RUST_LOG`: `info` default; `api=debug,sqlx=warn` for troubleshooting; `debug` globally is noisy.
- Startup `WARN SD_TRUSTED_PROXIES is empty` means rate limits are shared per proxy.
- Watch: `/health/db` from outside, disk space on `STORAGE_DIR` and the Postgres volume, backup age, certificate expiry (Caddy renews on its own; ports 80 and 443 must stay open).
- Run `scripts/smoke.sh https://desk.example.com` after every deploy and from a cron/uptime job. It checks health, the API through the proxy, the static UI, security headers (HSTS, nosniff, X-Frame-Options, CSP), CORS (allowed and denied origins), and body limits. `SMOKE_BIG=1` also sends a 41 MB body to test the proxy cap.

## Secret rotation

All secrets need `openssl rand -base64 48`, are length-checked and placeholders are rejected at startup. `JWT_SECRET` and `SD_SECRET_KEY` must differ. Edit `/etc/senddesk/env`, then `systemctl restart senddesk-api`.

| Secret | Effect of rotating |
| --- | --- |
| `JWT_SECRET` | signs access tokens and derives the attachment download-link key: outstanding access tokens and signed download links stop working. Refresh tokens are random and stored hashed in the database, so sessions survive; clients refresh on the next request. Safe. |
| `SD_SECRET_KEY` | seals stored SMTP/IMAP passwords. There is no re-seal tool: after rotating, every project's mail passwords are unreadable and must be re-entered in project settings (outbound mail and IMAP polling fail until then). Only rotate on suspected compromise. Keep the old key with old backups. |
| `SD_WEBHOOK_SECRET` | update the sender (mail provider) at the same time, otherwise inbound webhooks get `401`. |
| `POSTGRES_PASSWORD` / DB role password | `ALTER ROLE senddesk PASSWORD '...'`, update `DATABASE_URL` (compose: `.env`), restart. |
| `SD_BOOTSTRAP_PASSWORD` | only used by `api bootstrap`; remove after use. |

Rotate after anyone with access to the env file or a backup leaves.

## Security header notes

`deploy/Caddyfile` sends `Content-Security-Policy-Report-Only` for the UI (the Next export needs inline scripts) plus an enforced `frame-ancestors 'none'`. `/widget` stays frameable by design and carries `Referrer-Policy: no-referrer`. HSTS is `max-age=31536000` without `includeSubDomains`; add it only when every subdomain is HTTPS.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `/health/db` 503 | Postgres up, `DATABASE_URL`, `pg_hba.conf`, connection limit (the api opens up to 10). |
| API exits at boot with config errors | the message names the variable; secrets too short, equal, or still placeholders. |
| Browser CORS errors | `FRONTEND_URL` must be the exact origin (scheme, host, port), comma-separated for several. |
| Everyone is rate limited together | `SD_TRUSTED_PROXIES` empty or wrong, see above. |
| Login works but session drops | `COOKIE_SECURE` true needs HTTPS; check the proxy forwards `Set-Cookie`. `SD_ENV=development` also changes token handling: never use it in production. |
| UI shows 404 on deep links | Caddyfile `try_files {path} {path}/ {path}/index.html`; confirm `/srv/senddesk/web` has the export. |
