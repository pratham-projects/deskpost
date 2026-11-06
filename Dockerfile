# syntax=docker/dockerfile:1
# Targets: `api` (Rust server) and `web` (static UI).
#   docker build --target api -t senddesk-api .
#   docker build --target web -t senddesk-web .
# The UI is a static export and expects the API on the same origin (leave NEXT_PUBLIC_API_URL empty).

FROM rust:1-bookworm AS api-build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p api --locked && cp target/release/api /api

FROM debian:bookworm-slim AS api
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/senddesk senddesk \
    && mkdir -p /var/lib/senddesk/storage && chown -R senddesk /var/lib/senddesk
COPY --from=api-build /api /usr/local/bin/api
USER senddesk
ENV PORT=3000 STORAGE_DIR=/var/lib/senddesk/storage
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=3s CMD curl -fsS http://127.0.0.1:${PORT}/health || exit 1
ENTRYPOINT ["api"]
CMD ["serve"]

FROM oven/bun:1 AS web-build
WORKDIR /web
COPY apps/web/package.json apps/web/bun.lock ./
RUN bun install --frozen-lockfile
COPY apps/web ./
# NEXT_PUBLIC_* values are inlined at build time.
ARG NEXT_PUBLIC_API_URL=
ARG NEXT_PUBLIC_API_VERSION=v3
ENV NEXT_PUBLIC_API_URL=$NEXT_PUBLIC_API_URL NEXT_PUBLIC_API_VERSION=$NEXT_PUBLIC_API_VERSION
RUN bun run build

FROM caddy:2-alpine AS web
# Static export served by Caddy; `next start` does not work with `output: "export"`.
COPY --from=web-build /web/out /srv
COPY deploy/web.Caddyfile /etc/caddy/Caddyfile
EXPOSE 4000
