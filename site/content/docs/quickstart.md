---
title: Quickstart
group: Start
order: 3
description: From a checkout to a first ticket, with a dev inbox that catches the mail.
---

# Quickstart

The fastest local loop is Docker Compose with Mailpit.

## 1. Configure

```bash
git clone https://github.com/pratham-projects/deskpost
cd deskpost && cp .env.example .env
```

Fill these in `.env`. The last one is dev only: it lets a project use Mailpit.

```dotenv
POSTGRES_PASSWORD=<openssl rand -base64 24>
JWT_SECRET=<openssl rand -base64 48>
DP_SECRET_KEY=<openssl rand -base64 48>
FRONTEND_URL=http://localhost:4000
DP_ALLOW_PRIVATE_MAIL_HOSTS=true
```

## 2. Start

```bash
docker compose --profile dev up --build
```

| Service | URL |
| --- | --- |
| UI | http://localhost:4000 |
| API | http://localhost:3000 (`/health`, `/api/v3/...`) |
| Mailpit | http://localhost:8025 (SMTP `mailpit:1025`, no TLS, no auth) |

## 3. First admin

Public sign-up is off by default, so create the first admin with `bootstrap`. Pass the password through the environment, never as an argument (arguments show up in `ps`):

```bash
export DP_BOOTSTRAP_PASSWORD='a-long-password'
docker compose run --rm -e DP_BOOTSTRAP_PASSWORD \
  app bootstrap --email admin@example.com --name Admin
```

## 4. First ticket

Sign in, create a project, and in its settings point SMTP at `mailpit` port `1025`. Create a ticket from the UI and send a reply: the mail appears in Mailpit. To let customers file tickets from your site, see the [widget](#/widget).

## Without Docker

```bash
export DATABASE_URL=postgres://user@localhost:5432/postgres
cargo run -p api -- migrate
DP_ENV=development JWT_SECRET=... DP_SECRET_KEY=... cargo run -p api -- serve
cd apps/web && bun install && bun run dev
```

`DP_ENV=development` returns the refresh token in the JSON body and relaxes cookies. Never set it on a real deployment.
