# Deskpost

Multi-tenant support-ticket inbox: shared queue across projects, threaded conversations, internal notes,
canned responses, attachments, live viewer presence, and an embeddable customer widget. Customers can
also write in by email.

- **API**: Rust (Axum, SQLx, Postgres 16), `crates/api`. Crates: `core`, `db`, `mail`, `storage`.
- **UI**: Next.js in `apps/web`.
- **Mail**: no AWS. Outbound SMTP, inbound IMAP polling or signed webhook. See the
  [free email guide](docs/free-email.md) for Gmail, Brevo, Resend and Mailgun settings.
- Plan: [docs/PLAN.md](docs/PLAN.md). API reference: [docs/API.md](docs/API.md). Security: [SECURITY.md](SECURITY.md).

## Run with Docker Compose

```sh
cp .env.example .env
# fill POSTGRES_PASSWORD, JWT_SECRET, DP_SECRET_KEY (openssl rand -base64 48), DP_BOOTSTRAP_PASSWORD
docker compose up --build
docker compose run --rm app bootstrap --email admin@example.com --name Admin
```

| Service | URL |
|---------|-----|
| UI | http://localhost:4000 |
| API | http://localhost:3000 (`/health`, `/api/v3/...`) |
| Mailpit (caught mail) | http://localhost:8025 (SMTP `mailpit:1025`, no TLS, no auth) |

To send through mailpit set `DP_ALLOW_PRIVATE_MAIL_HOSTS=true` in `.env` (dev only). Set `FRONTEND_URL=http://localhost:4000` in `.env` so the browser may call the API. `NEXT_PUBLIC_*` values
are baked into the web image at build time (`PUBLIC_URL` in `.env`).

## Run locally (no Docker)

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
export DATABASE_URL=postgres://pratham@localhost:5433/postgres
cargo run -p api -- migrate
DP_ENV=development JWT_SECRET=... DP_SECRET_KEY=... cargo run -p api -- serve
cd apps/web && bun install && bun run dev
make ci          # fmt, clippy, test, deny
```

## Production

1. Build the binary (`cargo build --release -p api`) or the `api` Docker target; run `api migrate`.
2. systemd: [deploy/deskpost-api.service](deploy/deskpost-api.service), secrets in `/etc/deskpost/env`.
3. TLS and routing: [deploy/Caddyfile](deploy/Caddyfile) (`/api` and `/health` to the API, the rest to the UI).
4. Walk through [SECURITY.md](SECURITY.md); run `scripts/smoke.sh https://your.domain` after each deploy.
5. Backups, upgrades, rotation, first admin: [docs/OPERATIONS.md](docs/OPERATIONS.md) (`scripts/backup.sh`).

Every environment variable is listed with defaults in [.env.example](.env.example).

## Layout

```
crates/   api, core, db, mail, storage
apps/web  Next.js UI
deploy/   Caddyfile, systemd unit
scripts/  e2e, smoke, backup, deploy-check
docs/     PLAN, API, OPERATIONS, free-email
brain/    project knowledge bundle (OKF)
```

`UPSTREAM.md` describes the origin of the UI.
