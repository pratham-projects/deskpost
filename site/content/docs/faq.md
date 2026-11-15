---
title: FAQ
group: Help
order: 10
description: Short answers about status, names, mail, storage and licensing.
---

# FAQ

## Is it production ready?

Not yet. It is pre-release with no tagged release. It has unit, property, integration and browser tests, a review pass and a local final gate, and the container images build in CI and were run end to end with Docker Compose before release. Read the accepted risks in [Security](#/security).

## What are the repo, binary and unit called?

The repository is `deskpost`, the binary is `api`, variables start with `DP_` and the systemd unit is `deskpost-api`.

## Do I need AWS or a paid email service?

No. Outbound is any SMTP server, inbound is IMAP polling or a signed webhook. See [Email](#/email) for Gmail, Brevo, Resend and Mailgun.

## Can one install serve several clients?

Yes. A project is the tenant, with its own members, clients, templates, mail settings and API key.

## Where do attachments live?

On the server's filesystem under `STORAGE_DIR`. There is no S3 driver. Back them up with the database.

## How do I get the first admin in?

`deskpost bootstrap --email ... --name ...` with `DP_BOOTSTRAP_PASSWORD` in the environment. Public sign-up is off by default.

## How fast is it?

On one Apple M4 with Postgres on the same machine, 32 connections, listing 50 of 500 tickets measured p50 13.4 ms at 2356 req/s, and fetching one ticket 1.7 ms at 18236 req/s. Real SMTP latency, attachments and IMAP polling were not measured. See `docs/PERFORMANCE.md` in the repo.

## Which license?

AGPL-3.0-only, as declared in `Cargo.toml`.
