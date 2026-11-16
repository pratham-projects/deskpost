# Review: web / Dockerfile / deploy / e2e (R50)

Read-only review. Scope: `apps/web` (API client, auth, XSS), `Dockerfile`, `docker-compose.yml`, `deploy/`, `scripts/e2e.sh`, `.env.example`. Contract: `docs/API.md`. Nothing was executed, so each finding comes from reading code. Server-side crates were out of scope.

## High

### H1. Web container cannot run: `next start` with `output: "export"`
- `Dockerfile:41-44` (`CMD ["bun","run","start"]`), `apps/web/package.json` (`"start": "... next start -p 4000"`), `apps/web/next.config.mjs:6` (`output: "export"`).
- Failure: `next build` produces a static `out/`. `next start` refuses to run with `output: export`. The `web` image exits at boot, and `docker compose up` leaves `web` crash-looping. `deploy/Caddyfile` proxies `/` to `127.0.0.1:4000`, so the UI returns 502.
- The e2e harness serves `out/` through Caddy `file_server`. That path never exercises the Docker `web` target, so CI cannot catch this.
- Fix: serve `out/` with a static server. Either build a `caddy:alpine` stage with `COPY --from=web-build /web/out /srv`, or change `deploy/Caddyfile` to `root * /srv/web; file_server` with `try_files {path} {path}/index.html`. Drop the `bun`/`next start` stage and port 4000. Add a CI smoke step: `docker build --target web` plus `curl /`.

### H2. Build arg name drift: API base URL never reaches the bundle
- `apps/web/lib/api/config.ts:5` reads `NEXT_PUBLIC_API_URL`. `Dockerfile:33-35`, `docker-compose.yml:41` and `.env.example:42` set `NEXT_PUBLIC_API_BASE_URL`.
- Failure: `PUBLIC_URL=https://desk.example.com` has no effect and `API_BASE_URL` stays `""` (same origin). Same-origin behind Caddy works by accident. In the compose topology the UI is on `:4000` and the API on `:3000`, so every fetch goes to `localhost:4000/api/v3/...` and returns 404. The `Dockerfile:2` build example is wrong in the same way. Deployment docs and code disagree.
- Fix: pick one name (suggest `NEXT_PUBLIC_API_URL`) and update the Dockerfile `ARG`/`ENV`, compose `args`, `.env.example` and the comment. Better, document same-origin as the only supported deployment and leave the arg empty.

### H3. Long-lived refresh token stored in localStorage; access token and user object too
- `apps/web/lib/store.ts:62-67, 180-194, 101-112`, `apps/web/lib/api/auth.ts:118-135`.
- Failure: any XSS, or any compromised npm dependency (the `bun.lock` tree is large), reads `refreshToken`/`accessToken`. The refresh token is a 90-day family credential that defeats the HttpOnly cookie design in `docs/API.md` section 1.
- In production the API does not return `refreshToken` (`DP_ENV != production`), so the localStorage path is dead there. In dev or staging (`DP_ENV=development`) it is active. Deploying with `DP_ENV=development` to fix a cookie problem silently downgrades the security model.
- The access token is also persisted (`store.ts:189`) and re-used for 5 minutes after a reload. The comment at `store.ts:26` ("No persist ... don't survive reload") contradicts the code.
- Fix: remove all `localStorage` token handling. Use the cookie plus in-memory access token, and call `/auth/refresh` on load. Gate any dev-only header path behind `process.env.NODE_ENV !== "production"` at build time so it is tree-shaken.

## Medium

### M1. Tokens and full API payloads logged to the console in production
- `apps/web/lib/api/client.ts:109, 171` logs every response body (tickets, messages, user PII). `store.ts:44, 59, 84` and `auth.ts:127, 141` log token prefixes. `auth.ts:141` logs the headers object, which includes the full `X-Refresh-Token`.
- Failure: a screen-share, bug-report paste or browser extension captures the full refresh token and customer data. A 30-char prefix of a JWT also includes the header and most of the claims.
- Fix: delete these logs, or wrap them in a `debug()` helper that is a no-op unless `NODE_ENV !== "production"`. Never log headers or tokens.

### M2. 401 handling hangs requests forever and masks real errors
- `client.ts:57-68, 144-157`: on refresh failure it returns `new Promise(() => {})`. Callers `await` never resolves, so loading spinners hang, `finally` blocks never run, `isLoading` flags stick, and state updates are never reached.
- `client.ts:129-131` matches on message substrings (`"unauthorized"`, `"invalid token"`). Any 4xx whose message contains "unauthorized" (for example a validation message) triggers a refresh-plus-logout cycle. A 403 `FORBIDDEN` is correctly not matched, but an `INVALID_CREDENTIALS` 401 on a change-password call (`POST /users/me/password` wrong `currentPassword` returns 422 per the API doc, which is fine) would log the user out if the server ever returns 401 there.
- `client.ts:57`: `!endpoint.includes('/auth/')` also matches any URL that merely contains `/auth/`, such as search queries embedded in paths.
- Fix: reject with a typed `AuthExpiredError` after calling `handleLogout()`. Match only on `status === 401 && code in {TOKEN_EXPIRED, TOKEN_INVALID, TOKEN_MISSING}`. Drop the substring checks.

### M3. Concurrent refresh race against single-use rotating refresh tokens
- `client.ts:57-60, 134-143` plus `store.ts:41-92`. There is no single-flight guard around `refreshToken()`.
- Failure: the dashboard fires several requests in parallel (tickets, unread, projects). When the token expires, each one calls `authApi.refresh()` at the same time. The server rotates the token on every use (`docs/API.md` section 1). The grace window is 10 s, but a second rotation of an already-rotated token beyond the grace window revokes the whole family (`reuse_detected`). The user is logged out randomly. Cross-tab is worse: two tabs both refresh with the same stored `refreshToken`.
- Fix: memoize the in-flight promise (`let inflight: Promise<boolean> | null`) and clear it in `finally`. For cross-tab, use `navigator.locks.request("dp-refresh", ...)`.

### M4. Missing browser hardening headers on the UI
- `deploy/Caddyfile:20-26, 37-41`: no `Content-Security-Policy` for the app. Only `frame-ancestors 'none'` is set, and `/widget` is excluded from it. HSTS has no `includeSubDomains` or `preload` (acceptable, but note it). No `Permissions-Policy`.
- Failure: with the tokens in JS memory and localStorage (H3), a CSP is the main XSS mitigation, and none is present. The UI renders attacker-controlled text (customer ticket bodies). React escapes it today (`grep` found no `dangerouslySetInnerHTML` or `innerHTML`), but nothing enforces that going forward.
- Fix: add `Content-Security-Policy "default-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"`. Next static export needs inline scripts unless you use hashes. Use `Report-Only` first.

### M5. `/widget` is frameable by any origin and takes the project key from the URL
- `deploy/Caddyfile:22-26`, `apps/web/app/widget/page.tsx:9-15`.
- `?key=` is a public project key by design (`X-Project-Key`), so exposure alone is not a leak. However, `frame-ancestors` is not set at all for `/widget`, so any site can embed it and drive tickets with a harvested key. The API doc adds rate limiting, which is the only control.
- Fix: accept that this is intended, or let the project owner set an allowed-origin list and emit `frame-ancestors` per request. At minimum, state it in `SECURITY.md`. Also add `Referrer-Policy: no-referrer` for `/widget`, because the key sits in the URL query and leaks via `Referer` to third-party resources.

### M6. Attachment type allow-list drifts from the server contract
- `apps/web/lib/api/attachments.ts:15-17` allows images and videos only. `docs/API.md` section 9 says the server accepts images, PDF, text, office documents and zip, and states that "the UI list must be a subset".
- Failure A: videos (`mp4`, `webm`, `quicktime`, `x-msvideo`) are not in the server list, so uploads return `415` while the UI offers them.
- Failure B: PDF, text and office files cannot be attached from the UI even though the server supports them.
- Also: `attachment-viewer.tsx:79-86` treats only `image/` and `video/` as previewable, but the server serves PDFs `inline`. The UI shows "unknown" for them.
- Fix: generate the list from one source (the server `ALLOWED_FILE_TYPES`), or copy it and add a contract test in `e2e/`.

### M7. Blob URL leak in the attachment viewer
- `apps/web/components/attachment-viewer.tsx:66-93`. The effect cleanup closes over `contentUrl`, which is `null` at the time the effect runs, so `URL.revokeObjectURL` is never called. The whole file stays in memory for each preview opened.
- Fix: keep the local `url` variable in the effect scope and revoke that in cleanup. Also guard against setting state after unmount or after `attachment.id` changes (a stale request can overwrite a newer one).

### M8. `Content-Type` of untrusted blobs is used for rendering decisions
- `attachment-viewer.tsx:79-86` trusts `blob.type`. This is acceptable because it is only used to pick `<img>`/`<video>` and blob URLs are same-origin. Do not add an `<iframe>`/`<object>` preview for PDFs from the blob URL: a blob URL inherits the app origin, so an HTML or SVG payload served as `text/html` would run with access to the tokens in localStorage. Note this as a constraint, or sandbox it.

## Low

### L1. e2e downloads and executes a jar without a checksum
- `scripts/e2e.sh:44-49`: `curl` from Maven Central into `.tools/`, then `java -jar`. There is no SHA-256 pin. A compromised mirror or a corrupted partial file (guarded by the `.tmp` + `mv`, good) runs on a developer machine.
- Fix: pin `sha256` and verify with `shasum -a 256 -c` before `mv`.

### L2. e2e drops a database on whatever server `DATABASE_URL` names
- `scripts/e2e.sh:9-11, 39`: `BASE_DB=${DATABASE_URL:-...}` then `DROP DATABASE IF EXISTS dp_e2e`. Only `dp_e2e` is dropped, but `DATABASE_URL` pointing at a shared or production server will create and drop a database there.
- Fix: require an explicit `E2E_DATABASE_URL` or refuse hosts other than `localhost`/`127.0.0.1`.

### L3. e2e leaves the `Set -e` race and orphan processes on failure paths
- `scripts/e2e.sh:24-29`: `cleanup` kills only the PIDs captured. `java`, `caddy` and `api` started through a subshell are fine, but `bunx playwright` child browsers are not in `PIDS` and survive an interrupt. `trap ... EXIT` does not cover `SIGKILL`.
- The port pre-check (`nc -z`) is racy with `wait_port` (another process could grab a port between the check and the bind). Low impact locally.

### L4. Secrets placed in the process environment and in a world-readable run directory
- `scripts/e2e.sh:59-68`: random secrets exported (fine for scratch), but `$RUN/api.log` is created with default umask. Logs may contain request details. Dev only, low.
- `Dockerfile` `api` stage: `HEALTHCHECK` uses `curl`, which adds an attack surface. Use the binary itself (`api healthcheck`) if one exists.

### L5. Docker and compose hygiene
- `Dockerfile:12, 28, 38`: base images are not pinned by digest (`rust:1-bookworm`, `oven/bun:1`). Reproducibility and supply chain.
- `Dockerfile:39`: the `web` runtime stage copies the whole build directory including `node_modules` and sources (`COPY --from=web-build /web ./`). This disappears once H1 is fixed with a static image.
- `docker-compose.yml:13, 22`: `app` and `migrate` use `env_file: .env`, which also injects `POSTGRES_PASSWORD` and `DP_BOOTSTRAP_PASSWORD` into the app container. Use an explicit `environment:` allow-list.
- `docker-compose.yml:2-3, 49-52`: `mailpit` is in the default profile. Anyone who runs the compose file in production gets a catch-all SMTP sink on loopback. Put it in `profiles: ["dev"]`.
- `.dockerignore` does not exclude `apps/web/out`, `e2e`, `.coord` (it does exclude `.coord`), `docs`. Build context bloat only.
- `deploy/deskpost-api.service`: good baseline. Consider adding `ProtectKernelModules=true`, `RestrictNamespaces=true`, `SystemCallFilter=@system-service`, `MemoryDenyWriteExecute=true`, `CapabilityBoundingSet=` and `UMask=0077`.

### L6. Caddy: API paths get no `X-Content-Type-Options` guarantee beyond the API's own
- `deploy/Caddyfile:12-20`: the `header` block with `nosniff` and HSTS applies to every response, which is fine. Note `-Server` removes the Caddy header only.
- `request_body max_size 40MB` applies to all `/api/*` routes, so a 40 MB body reaches the API for routes that expect 64 KiB. The API enforces the real limits (as the comment says), but the body is buffered on the proxy first. Prefer separate `handle` blocks for the upload and webhook paths and a small default.
- Missing `reverse_proxy` timeouts and `health_uri`. A hung upstream ties up connections.

### L7. Dead or misleading code
- `client.ts:12-15`: the `ApiError` type does not match the real envelope (`{success,error:{code,message}}`).
- `client.ts:160-165` handles `errorData.type === "validation"` and `errorData.summary`, which the Rust API never emits. The 422 `details` field is dropped, so field-level validation messages never reach the form.
- `tickets.ts:200`: paths are built from `API_VERSION` directly instead of `API_PREFIX`, so two sources build the same URL. Combined with H2 this is easy to get wrong.

## Checked, no issue found
- XSS: `grep` found no `dangerouslySetInnerHTML`, `innerHTML`, `eval`, `document.cookie` or `window.open` in app code. Message bodies render as React text with `whitespace-pre-wrap`. `htmlBody` is declared in the type (`tickets.ts:22`) but never rendered.
- Tenant isolation: the client sends only the Bearer token and IDs, with no client-supplied tenant claims. Authorization must be (and per the API doc is) enforced server-side, which was not reviewed here.
- Attachment fetch uses `Authorization` header and blob URLs. The `?token=` URL form in the API doc is not used by the UI, so tokens do not end up in URLs or `Referer`.
- CORS: not an issue for the documented same-origin deployment. `credentials: 'include'` is set on all calls, which is harmless same-origin.
- `deploy/deskpost-api.service`: runs as an unprivileged user with `NoNewPrivileges`, `ProtectSystem=strict`, an `EnvironmentFile` outside the repo. Good.
- `.env.example`: contains no real secrets, and `DATABASE_URL` carries a `CHANGE_ME` placeholder that the API rejects at startup per the comment.

## Suggested order
1. H1 and H2 (the shipped Docker web image cannot work as configured).
2. H3 and M1 (token storage and logging).
3. M3 and M2 (auth flow races and hangs).
4. M4 (CSP), then the remaining medium and low items.
