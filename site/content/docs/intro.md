---
title: Introduction
group: Start
order: 1
description: What Deskpost is, what runs where, and what it leaves out.
---

# Deskpost

A multi-tenant support inbox you host yourself: a shared queue across projects, threaded conversations, internal notes, canned responses, attachments, live viewer presence and an embeddable customer widget. Customers can also write in by email.

> Names to look for: the repository is `pratham-projects/deskpost`, the server binary is `api`, environment variables start with `DP_` and the systemd unit is `deskpost-api`.

> Deskpost is pre-release. There is no tagged release yet. Read the [FAQ](#/faq) and the accepted risks in [Security](#/security) before you rely on it.

## What runs

| Part | Default port | Role |
| --- | --- | --- |
| `api` | 3000 | Axum HTTP API under `/api/v3`, plus the background workers (IMAP poll, mail outbox, housekeeping). |
| Web UI | 4000 | Next.js app in `apps/web`, shipped as the `deskpost-web` image or a static export. |
| PostgreSQL 16 | 5432 | Tickets, messages, users, sessions, the mail outbox. |
| Caddy | 80, 443 | TLS and one origin: `/api/*` and `/health*` to the API, the rest to the UI. |
| Mailpit | 8025, 1025 | Optional dev inbox that catches outbound mail (compose profile `dev`). |

## Model

A project is the tenant. It owns its members, clients, tickets, templates, auto-replies, mail settings and API key. Users have a global role (`admin` or `agent`); what they can see is decided by project membership. A non-member gets `404` for project resources.

## Crates

`core` (status machine, threading, reply stripping, secret box), `db` (SQLx and migrations), `storage` (attachment store), `mail` (SMTP, IMAP, MIME) and `api` (the binary).

## Non-goals

Deskpost has no paid or proprietary services, SaaS billing or mobile apps. Attachments live on the local filesystem, and there is no S3 driver.
