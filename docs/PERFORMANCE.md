# Performance

`scripts/load.sh` boots a release API on a scratch db (`sd_load`), seeds 500 tickets, then runs
[oha](https://github.com/hatoo/oha) for 10 s per scenario at 32 connections (login: 8, it is Argon2).
It sets `SD_RATE_LIMIT_SCALE=100000` because every request comes from one address; production keeps 1.
Raw output lands in `e2e/.run/load.md`. Re-run after any query or pool change.

Host: Apple M4 (10 cpu), Postgres 16 on localhost, pool size 10, same box for load generator and server.

| scenario | rps | p50 ms | p99 ms | errors |
|---|---|---|---|---|
| login (Argon2id) | 348 | 22.8 | 31 | 0 |
| ticket list (50) | 2356 | 13.4 | 16.6 | 0 |
| ticket search | 3794 | 8.3 | 10.2 | 0 |
| ticket counts | 16863 | 1.8 | 2.2 | 0 |
| ticket get | 18236 | 1.7 | 2.6 | 0 |
| ticket messages | 13112 | 2.3 | 4.5 | 0 |
| widget ticket create | 3237 | 8.7 | 23.9 | 0 |
| message post, spread over 500 tickets | 9735 | 3.2 | 3.7 | 0 |
| message post, one hot ticket | 1352 | 22.1 | 58.8 | 0 |
| health | 162562 | 0.1 | 0.4 | 0 |

Memory: API RSS 32 MB idle, 170 MB after about 32k extra tickets and 1.4M requests (no growth trend inside a
scenario; not a soak test). API log: no errors or panics.

## Findings

- **Fixed: missing recency index.** The inbox list ordered a project's tickets by `updated_at` with a seq scan
  and sort of the whole project (4.4 ms at 31k tickets). Migration `0004` adds
  `(project_id, updated_at DESC, id DESC)`; list went 1856 -> 2356 rps and p99 44 -> 17 ms at 500 tickets,
  and the gap grows with project size. Test: `crates/api/tests/migrations.rs`.
- **Expected, not a bug:** posting many replies to one ticket serialises on that ticket row (1.4k rps vs 9.7k
  spread). One conversation does not see that rate.
- **Login** is capped by Argon2 cost and the hashing semaphore on purpose; the per-address login quota (10/min)
  and per-account throttle keep it from being a flood target.
- **Not measured:** real SMTP latency (replies with `sendEmail:true` and no SMTP just log a failure; the load
  run sends `sendEmail:false`), attachments, IMAP polling, multi-host Postgres.
