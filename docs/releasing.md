# Releasing

## Cut a release
Branch flow: `feat/*` -> `staging` -> `main` (prod). Merge with `--no-ff` only.
1. On `staging`: update `CHANGELOG.md` (move `Unreleased` items under the new version), bump `version` in root `Cargo.toml`, run `make ci` + `make e2e`, commit.
2. Merge `staging` into `main` (`--no-ff`).
3. First release goes out as a candidate: set `version = "0.1.0-rc.N"` in root `Cargo.toml` (the tag must equal the crate version or `dist` refuses to plan), then `git tag v0.1.0-rc.N && git push origin v0.1.0-rc.N`. cargo-dist marks tags with a prerelease suffix as GitHub pre-releases. Smoke-test the artifacts and images.
4. If good, tag the final: `git tag v0.1.0 && git push origin v0.1.0`. Fix problems with `rc.2`, `rc.3`, ...
5. Each tag fires 3 workflows:
   - `release` (cargo-dist): builds the `deskpost` binary, creates the GitHub Release.
   - `docker`: pushes `ghcr.io/<owner>/deskpost-api` and `-web` (amd64+arm64), tags `X.Y.Z`, `X.Y`, `latest`.
   - `release-assets`: attaches deploy files after `release` succeeds. Manual: Actions > release-assets > Run, tag `v0.1.0`.

PRs and pushes to `main`/`staging` also run the `docker-build` and `deploy-check` CI jobs, so broken images or deploy files surface before a tag.

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
Installs `deskpost` to `~/.cargo/bin`. Needs Postgres 16 and env from `.env.example`; see `deskpost-api.service` for systemd.

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
