---
title: Install
group: Start
order: 2
description: Binary installer, Docker Compose from release assets, or build from source.
---

# Install

Pick one. Every path needs PostgreSQL 16 (Compose brings its own) and three secrets. Generate secrets with `openssl rand -base64 48`; `JWT_SECRET` and `DP_SECRET_KEY` must differ and placeholders are rejected at startup.

> This is pre-release. No release is tagged yet, so the Binary and Docker Compose sections below work from the first release. Today, use the Source section or clone the repo and run `docker compose up --build`.

## Binary

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/pratham-projects/deskpost/releases/latest/download/deskpost-installer.sh | sh
```

This installs `api` to `~/.cargo/bin`. Tarballs are `api-<target>.tar.xz` for `aarch64` and `x86_64`, on `apple-darwin` and `unknown-linux-gnu`, each with a `.sha256`, plus `sha256.sum` and `source.tar.gz`.

The binary is the API only. Serve the UI from the web image or from a static export of `apps/web` (see [Deploy](#/deploy)). The release also attaches `deskpost-api.service`, `Caddyfile` and `web.Caddyfile`.

## Docker Compose

No source checkout needed. The compose file and `default.env.example` (GitHub renames dotfiles on upload) are release assets.

```bash
V=v0.1.0; B=https://github.com/pratham-projects/deskpost/releases/download/$V
mkdir deskpost && cd deskpost
curl -LO $B/docker-compose.yml && curl -L $B/default.env.example -o .env
# edit .env: POSTGRES_PASSWORD, JWT_SECRET, DP_SECRET_KEY, FRONTEND_URL
docker compose pull && docker compose up -d --no-build
```

Replace `v0.1.0` with a tag that has been published. Images are `ghcr.io/pratham-projects/deskpost-api` and `ghcr.io/pratham-projects/deskpost-web` (amd64 and arm64). Pin them with `DP_API_IMAGE` and `DP_WEB_IMAGE` in `.env`.

The UI listens on `127.0.0.1:4000` and the API on `127.0.0.1:3000`. Compose binds everything to localhost, so put Caddy in front for TLS.

## Source

```bash
git clone https://github.com/pratham-projects/deskpost
cd deskpost
cargo build --release -p deskpost --locked
```

You need a stable Rust toolchain and, for the UI, bun. With a source checkout, `docker compose up --build` builds the images locally instead.

> The container images build in CI, and the Compose stack was run end to end locally (migrate, bootstrap, sign-in). The host layout (release binary, static export, Caddy) was replayed by hand.
