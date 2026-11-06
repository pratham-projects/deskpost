# SendDesk

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
# fill POSTGRES_PASSWORD, JWT_SECRET, SD_SECRET_KEY (openssl rand -base64 48), SD_BOOTSTRAP_PASSWORD
docker compose up --build
docker compose run --rm app bootstrap --email admin@example.com --name Admin
```

| Service | URL |
|---------|-----|
| UI | http://localhost:4000 |
| API | http://localhost:3000 (`/health`, `/api/v3/...`) |
| Mailpit (caught mail) | http://localhost:8025 (SMTP `mailpit:1025`, no TLS, no auth) |

Set `FRONTEND_URL=http://localhost:4000` in `.env` so the browser may call the API. `NEXT_PUBLIC_*` values
are baked into the web image at build time (`PUBLIC_URL` in `.env`).

## Run locally (no Docker)

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
export DATABASE_URL=postgres://pratham@localhost:5433/postgres
cargo run -p api -- migrate
SD_ENV=development JWT_SECRET=... SD_SECRET_KEY=... cargo run -p api -- serve
cd apps/web && bun install && bun run dev
make ci          # fmt, clippy, test, deny
```

## Production

1. Build the binary (`cargo build --release -p api`) or the `api` Docker target; run `api migrate`.
2. systemd: [deploy/senddesk-api.service](deploy/senddesk-api.service), secrets in `/etc/senddesk/env`.
3. TLS and routing: [deploy/Caddyfile](deploy/Caddyfile) (`/api` and `/health` to the API, the rest to the UI).
4. Walk through [SECURITY.md](SECURITY.md).

Every environment variable is listed with defaults in [.env.example](.env.example).

## Layout

```
crates/   api, core, db, mail, storage
apps/web  Next.js UI
deploy/   Caddyfile, systemd unit
docs/     PLAN, API, free-email
brain/    project knowledge bundle (OKF)
```

`UPSTREAM.md` describes the origin of the UI.
