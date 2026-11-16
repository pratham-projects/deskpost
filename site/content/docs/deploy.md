---
title: Deploy
group: Operate
order: 8
description: One host with Compose or systemd, Caddy, backups and smoke checks.
---

# Deploy on one host

Postgres 16, the `api` binary or the compose stack, and Caddy in front. Everything below except the container build was replayed by hand on a machine without Docker.

## Layout (host install)

| Piece | Where |
| --- | --- |
| API binary | `/usr/local/bin/deskpost` |
| Env file | `/etc/deskpost/env`, `chmod 600`. Set `STORAGE_DIR=/var/lib/deskpost/storage` here. |
| Attachments | `/var/lib/deskpost/storage` (`<root>/ab/cd/<key>` plus `.json` sidecars) |
| Static UI | `/srv/deskpost/web`, from `NEXT_PUBLIC_API_URL= NEXT_PUBLIC_API_VERSION=v3 bun run build` in `apps/web` |
| Proxy | `deploy/Caddyfile`: `/api/*` and `/health*` to `127.0.0.1:3000`, the rest is the UI |
| Unit | `deploy/deskpost-api.service`, runs as the `deskpost` user |

The API binds `0.0.0.0:$PORT` with no bind-address setting. Block port 3000 from outside so clients cannot skip Caddy and spoof client IPs.

## Steps

```bash
useradd --system --home /var/lib/deskpost --shell /usr/sbin/nologin deskpost
install -d -m 755 /etc/deskpost
sed -E '/^[[:space:]]*#/d; s/[[:space:]]+#.*$//; /^[[:space:]]*$/d' .env | install -m 600 /dev/stdin /etc/deskpost/env
install -m 755 target/release/deskpost /usr/local/bin/deskpost
install -m 644 deploy/deskpost-api.service /etc/systemd/system/
(set -a; . /etc/deskpost/env; set +a; sudo -E -u deskpost /usr/local/bin/deskpost migrate)
systemctl daemon-reload && systemctl enable --now deskpost-api
install -d /srv/deskpost/web && cp -R apps/web/out/. /srv/deskpost/web/
caddy validate --config deploy/Caddyfile --adapter caddyfile && systemctl reload caddy
scripts/smoke.sh https://desk.example.com
```

The `sed` strips trailing comments because systemd, unlike compose, keeps them in values. Edit the domain in `deploy/Caddyfile` first.

## Compose

`docker compose up -d` runs `migrate` before `app`. The `web` service serves the UI on `127.0.0.1:4000` over plain HTTP without security headers: put TLS and Caddy-style headers in front of it.

## Trusted proxies

Set `DP_TRUSTED_PROXIES` to the proxy only: `127.0.0.1` for Caddy on the same host, the container network address for Caddy in a container. Left empty, every client shares one rate-limit bucket behind the proxy and the API logs a warning.

## Backups

```bash
DATABASE_URL=postgres://deskpost:...@localhost:5432/deskpost \
STORAGE_DIR=/var/lib/deskpost/storage BACKUP_DIR=/var/backups/deskpost scripts/backup.sh
```

It writes a custom-format `pg_dump` and a tarball of `STORAGE_DIR` (mode 600, 14-day retention). It refuses to run when `pg_dump` and the server have different major versions. Copy backups off the host. A dump is only restorable with the `DP_SECRET_KEY` it was made under, so store that key separately.

## Upgrades

1. Back up.
2. Build the new binary and UI, diff `.env.example` against your env file.
3. `deskpost migrate`. Migrations are forward-only; rollback means restoring the backup and the old binary.
4. Restart the service, replace the UI, run `scripts/smoke.sh`.

The API and UI are versioned together. Deploy both in one step.

## Smoke and health

`scripts/smoke.sh https://desk.example.com` checks health, the API through the proxy, the static UI, security headers (HSTS, nosniff, X-Frame-Options, CSP), CORS allowed and denied origins, and body limits. Run it after every deploy and from a cron or uptime job. Logs go to stdout: `journalctl -u deskpost-api -f` or `docker compose logs -f app`. Query strings are never logged, because download tokens travel in them.
