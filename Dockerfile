# syntax=docker/dockerfile:1
# Targets: `api` (Rust server) and `web` (Next.js UI).
#   docker build --target api -t senddesk-api .
#   docker build --target web --build-arg NEXT_PUBLIC_API_BASE_URL=https://desk.example.com -t senddesk-web .

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
ARG NEXT_PUBLIC_API_BASE_URL=http://localhost:3000
ARG NEXT_PUBLIC_API_VERSION=v3
ENV NEXT_PUBLIC_API_BASE_URL=$NEXT_PUBLIC_API_BASE_URL NEXT_PUBLIC_API_VERSION=$NEXT_PUBLIC_API_VERSION
RUN bun run build

FROM oven/bun:1-slim AS web
WORKDIR /web
ENV NODE_ENV=production
COPY --from=web-build /web ./
USER bun
EXPOSE 4000
CMD ["bun", "run", "start"]
