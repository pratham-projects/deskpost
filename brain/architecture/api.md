---
type: Architecture
title: API crate (R20)
description: crates/api layout, config, envelope, auth flow, limits, and how to add a route with its tests.
tags: [api, auth, security, testing]
timestamp: 2026-10-27
---

# API crate

Contract: `docs/API.md`. DB side: [DB crate](/architecture/db-contract.md). Library crate `api` plus binary `api` (`serve`, `migrate`, `bootstrap --email --password --name`; the password may come from `DP_BOOTSTRAP_PASSWORD`, which keeps it out of `ps`).

`build_app(state)` returns the whole `Router`; `build_app_with(state, extra)` mounts `extra` under `/api/{version}`. Tests use `tower::ServiceExt::oneshot`, no socket.

| module | contract |
|--------|----------|
| `config` | `Config::from_lookup` collects every problem. `JWT_SECRET`, `DP_SECRET_KEY` >= 32 bytes, no placeholder substrings, >= 8 distinct bytes, must differ. `DP_ENV` defaults to **production** (Strict + Secure cookie, no refresh token in the body). Also `PORT`, `API_VERSION` (charset-checked, goes into paths), `DP_TRUSTED_PROXIES`, `COOKIE_SECURE`, `FRONTEND_URL` (plain origins), `STORAGE_DIR`, `DP_BODY_LIMIT_BYTES` (64 KiB), `DP_UPLOAD_LIMIT_BYTES`, `DP_WEBHOOK_LIMIT_BYTES`, `DP_ALLOW_SIGNUP`. `Debug` hides secrets. |
| `error` | `ApiError` -> envelope `{success:false,error:{code,message,details?}}`; 401s add `WWW-Authenticate`, 429 adds `Retry-After`. `ApiError::internal(e)` logs and answers a generic 500. `Fields` collects 422 details. `Envelope::{ok,created}.paginated().stats()` is the success shape. |
| `extract` | `ApiJson` (400/413/415/422 as envelope), `ApiPath` (bad UUID -> 400), `AuthUser` (Bearer, loads the user, inactive -> `TOKEN_INVALID`), `GlobalAdmin`, `project_role` (404 for non-members), `project_admin` (403 for agents). |
| `jwt` | HS256, header must equal ours byte for byte (no `alg` negotiation), HMAC verified in constant time before parsing, `exp`/`nbf` with no leeway, claims `sub jti iat nbf exp`, 5 min. |
| `password` | Argon2id (19 MiB, t=2), semaphore around `spawn_blocking`, dummy hash for unknown emails. |
| `ratelimit` | Sliding-window log per `(Rule, client)`; Login 10/min, Register 5/h, Refresh 30/min, Public 20/min, Webhook 120/min. IPv6 keyed by /64. At most 10 000 keys; expired go first, then least recent. Rejected requests are not recorded. |
| `client_ip` | `X-Forwarded-For` only from a peer in `DP_TRUSTED_PROXIES`, read right to left to the first untrusted hop. |
| `routes::auth` | signup, signin, refresh, signout, signout-all, me, config; `create_admin` for the CLI. |
| `routes::health` | `/health` liveness, `/health/db` readiness (2 s timeout, no details). |

Layers (outer to inner): CORS (credentials, only `FRONTEND_URL` origins), trace (path only, no query), security headers (nosniff, DENY, no-referrer, CSP `default-src 'none'`, `no-store`, HSTS when `COOKIE_SECURE`), `DefaultBodyLimit`. Unknown path -> 404 envelope, wrong method -> 405 envelope.

## Auth behaviour

- Refresh token: 256 random bits, only SHA-256 stored. Cookie `refresh_token`, `HttpOnly; Path=/api/{v}/auth; Max-Age=7d`; also accepted in `X-Refresh-Token`.
- Reuse after the 10 s grace revokes the family (`reuse_detected`). Inside the grace the answer is 401 `TOKEN_EXPIRED` **without** a clearing cookie. Family cap 90 days; a token never outlives its family.
- Sign-in: same `INVALID_CREDENTIALS` body for unknown email, wrong password and inactive/suspended accounts; the dummy hash equalises timing. Sign-up returns 409 for a taken email (contract) but hashes first.
- Signout-all only revokes refresh families; an access token already issued works until it expires (<= 5 min).

## Fixed on purpose (vs TS)

Per-IP limiter plus /64 grouping, `X-Forwarded-For` only via trusted proxies, JWT header pinning, and `alg=none` impossible; secret placeholder check; no refresh token in prod bodies.

## Adding a route (and its tests)

1. Create `routes/<name>.rs` with `pub fn router() -> Router<AppState>` and register it in `routes/mod.rs` and `build_app_with` (`.nest("/<name>", ...)`).
2. Signature: `State(s): State<AppState>`, `AuthUser(user): AuthUser` (or `GlobalAdmin`), `ApiPath(id): ApiPath<Uuid>`, `ApiJson(req): ApiJson<Req>`. For a project resource call `project_role(&s, user.id, project_id)` or `project_admin(...)` before touching data; pass `user.id` as `actor` to the `dp_db` query. Return `Result<Envelope<T>, ApiError>`; map `dp_db::Error` with `?` (logged, generic 500) and `is_unique_violation` to `Conflict`.
3. Validate with `Fields`; request structs use `#[serde(default)]` so missing fields give 422, not 400.
4. Public/widget/webhook routes call `s.limiter.check(Rule::Public, ip)?` with `ClientIp`. Upload/webhook routes add `.layer(DefaultBodyLimit::max(cfg.upload_limit))` on the route only.
5. Tests in `crates/api/tests/<name>.rs`: `mod common; use common::*;` then `#[sqlx::test(migrations = "../db/migrations")] async fn x(pool: PgPool) { let t = TestApp::new(pool).await; ... }`.
   - `t.user(email, Role)` creates a user with password `PASSWORD`; `t.signin(email)` returns `Session { access_token, refresh_token, cookie }`; `t.project(owner, slug)` and `t.add_member(admin, project, user, role)` build tenants.
   - `t.get(path)/post(path)/req(method, path)` take paths under `/api/{version}`; `.bearer(token)`, `.cookie(pair)`, `.header(k, v)`, `.peer(ip)`; send with `t.send(req)` or `t.send_json(req, json!(..))`; `Resp::{status, json(), error_code(), set_cookie(), header()}`.
   - The tenant pattern is `tests/tenant.rs::isolated_project_route`: owner (admin), agent member, outsider (even a global admin) must get 200/200/404, the outsider's 404 must equal the body for a missing id, and admin-only routes give agent 403.
   - Every route needs: happy path, 401 without token, 404 for a foreign tenant, 403 for a too-low role, 422 for each bad field.
   - Extra fixture routes can be mounted with `build_app_with` (see `fixture_routes` in `tests/common/mod.rs`).
