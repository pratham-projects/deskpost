# Releasing

## Cut a release
1. Merge to `prod`; bump `version` in root `Cargo.toml`, commit.
2. `git tag v0.1.0 && git push origin v0.1.0`
3. Tag fires 3 workflows:
   - `release` (cargo-dist): builds `api`, creates GitHub Release.
   - `docker`: pushes `ghcr.io/<owner>/deskpost-api` and `-web` (amd64+arm64), tags `X.Y.Z`, `X.Y`, `latest`.
   - `release-assets`: attaches deploy files after `release` succeeds. Manual: Actions > release-assets > Run, tag `v0.1.0`.

Change targets/installers: edit `dist-workspace.toml`, run `dist generate`, commit `release.yml`. Check: `dist plan`.

## Release assets
- `deskpost-{aarch64,x86_64}-{apple-darwin,unknown-linux-gnu}.tar.xz` (+ `.sha256`), `sha256.sum`, `source.tar.gz`
- `deskpost-installer.sh`
- `docker-compose.yml`, `.env.example`
- `deploy/Caddyfile` -> `Caddyfile`, `deploy/web.Caddyfile` -> `web.Caddyfile`
- `deskpost-api.service`

## Install API binary
```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pratham-projects/deskpost/releases/latest/download/deskpost-installer.sh | sh
```
Installs `api` to `~/.cargo/bin`. Needs Postgres 16 and env from `.env.example`; see `deskpost-api.service` for systemd.

## Compose quickstart (no source checkout)
```sh
V=v0.1.0; B=https://github.com/pratham-projects/deskpost/releases/download/$V
mkdir deskpost && cd deskpost
curl -LO $B/docker-compose.yml && curl -L $B/.env.example -o .env
# edit .env: POSTGRES_PASSWORD, JWT_SECRET, DP_SECRET_KEY (openssl rand -base64 48), FRONTEND_URL
docker compose pull && docker compose up -d --no-build
```
Pin versions: `DP_API_IMAGE=ghcr.io/pratham-projects/deskpost-api:0.1.0`, `DP_WEB_IMAGE=...-web:0.1.0` in `.env`.
Web UI on `127.0.0.1:4000`, API on `127.0.0.1:3000`. Put `Caddyfile` in front for TLS.
