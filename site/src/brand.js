// Deskpost: all site copy and config. Every claim here is traceable to the repo (README, SECURITY.md, docs/API.md,
// docs/OPERATIONS.md, docs/free-email.md, docs/PERFORMANCE.md, crates/*). Deskpost is the site name only: the repository
// is `sendesk`, the binary is `api`, env vars start with `SD_`.
export default {
  name: "Deskpost",
  wordmark: "desk<b>post</b>",
  repo: "pratham-projects/sendesk",
  scene: "inbox",
  license: "AGPL-3.0-only",
  stack: "Rust · Postgres 16 · Next.js",
  status: "pre-release",
  headline: "Support inbox you host <em>yourself.</em>",
  sub: "A shared queue across projects, threaded replies, internal notes and an embeddable widget. Customers can just email. <strong>A Rust API, a web UI and one Postgres.</strong> No paid services.",
  heroCmdPre: "git clone https://github.com/pratham-projects/sendesk && cd sendesk",
  heroCmd: "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pratham-projects/sendesk/releases/latest/download/api-installer.sh | sh",

  liveTitle: "Built in the open.",
  liveNote: "Counts appear when GitHub answers; the rest comes from files in the repo. The repository is named `sendesk` and the binary is `api`: Deskpost is the name of this site. Load figures come from one laptop, see `docs/PERFORMANCE.md` before quoting them.",
  facts: [
    ["License", "AGPL-3.0-only"],
    ["Backend", "Rust, Axum, SQLx, Postgres 16"],
    ["Workspace", "5 crates, 1 binary (`api`)"],
    ["API prefix", "`/api/v3`"],
    ["Ticket list p50, 500 tickets", "13.4 ms · 2.4k req/s · Apple M4"],
    ["API memory, idle", "32 MB RSS"],
  ],

  featuresTitle: "Everything it does, listed.",
  featuresLede: "No seats, no tiers, no hosted dependency. The pieces below are in the repo today; known limits are listed in `SECURITY.md`.",
  features: [
    { title: "The inbox", blurb: "One queue for every project an agent belongs to.", items: [
      ["inbox", "Shared queue across projects", "Tickets from every project you are a member of in one list, with per-status counts, filters, unread-first sorting and literal text search over subjects, descriptions and message bodies."],
      ["chat", "Threaded conversations", "Every ticket is a thread of client and agent messages, newest first, paginated. Per-project ticket numbers are gap-free."],
      ["notebook", "Internal notes", "A reply flagged internal is stored as a note and is never emailed."],
      ["message", "Canned responses", "Per-project templates, searchable by title and body. Editing is gated by a per-member `canManageTemplates` flag or the project admin role."],
      ["eye", "Live viewer presence", "The UI sends a heartbeat while a ticket is open and shows who else is looking. Presence rows expire after two minutes."],
      ["checklist", "Ticket status machine", "`pending`, `in_progress`, `on_hold`, `resolved`, `closed`. Open states move freely. A closed ticket can only be reopened to `pending`."],
      ["clock", "Auto-close worker", "Resolved tickets idle past the project's `auto_close_days` become closed. The column defaults to 7 and `0` means never. An hourly job runs it."],
    ]},
    { title: "Email in and out", blurb: "Plain SMTP out, IMAP or a signed webhook in. No AWS.", items: [
      ["send", "Outbound over SMTP", "Replies are queued with the message in one transaction and sent by a worker over the project's own SMTP server. The request itself never waits on SMTP and always answers `201`."],
      ["mailbox", "Inbound by IMAP poll", "Per-project mailbox polled every 60 s by default (`SD_IMAP_POLL_SECS`), up to 50 messages per fetch with `BODY.PEEK`. A message that cannot be parsed is consumed, so it cannot wedge the poller."],
      ["webhook", "Inbound by signed webhook", "`POST /api/v3/incoming-email` for providers that push mail. HMAC-SHA256 over `timestamp.body`, five minute tolerance, constant-time compare. Unset `SD_WEBHOOK_SECRET` and it is off."],
      ["branch", "Threading and reply stripping", "Replies find their ticket by the `[Ticket #N]` subject tag, `In-Reply-To` and `References`. Quoted replies, \"On … wrote:\" blocks and signatures are cut, in several languages."],
      ["support", "Auto-replies", "Per-project confirmation and resolved emails, one entry per event, each switchable. Auto-responders such as `mailer-daemon` are ignored on the way in."],
      ["plug", "Free email, documented", "Settings for Gmail (app password), Brevo, Resend and Mailgun, plus Mailpit as a dev inbox that shows everything the app would send."],
    ]},
    { title: "Customers", blurb: "They can use a form, or just write an email.", items: [
      ["code", "Embeddable widget", "A `/widget?project=<id>&key=<key>` page you can frame on your own site. Creating a ticket needs the project API key in `X-Project-Key` and is rate limited."],
      ["attach", "Attachments on the local filesystem", "Stored under `STORAGE_DIR`, never in the database. Caps default to 10 MB per file, 25 MB per request, 20 files and a 1024 MB project quota, all configurable."],
      ["shield2", "Attachments, hardened", "Filenames are sanitised, no path is built from user input, and uploads that start like a PE, ELF, Mach-O or script are refused. Only safe types display inline; the rest download with `nosniff`."],
    ]},
    { title: "Teams and tenants", blurb: "A project is the tenant. Membership decides everything.", items: [
      ["layers", "Multi-tenant projects", "Each project has its own members, clients, templates, mail settings and API key. A non-member gets `404`, not `403`, so existence never leaks."],
      ["users", "Roles and per-member flags", "Global `admin` or `agent`, plus a project role. Members can be notified of new tickets, limited to tickets assigned to or created by them, or allowed to manage templates."],
      ["lock", "Keys you can rotate", "The project API key is stored hashed and shown once. Regenerate it and the old key stops working at once."],
    ]},
    { title: "Security", blurb: "Written down before the first route existed.", items: [
      ["finger", "Argon2id passwords", "Hashing runs behind a concurrency semaphore, with sign-in limited per address and per account."],
      ["shield", "Rotating refresh tokens", "Five minute access token plus an opaque, hashed, rotated refresh cookie. Reusing a rotated token revokes the whole family, with a 10 s grace for parallel tabs."],
      ["lock", "Secrets sealed at rest", "SMTP and IMAP passwords are encrypted with AES-256-GCM, bound per field, and never returned by the API."],
      ["globe", "SSRF-safe mail hosts", "Loopback, private and link-local targets are rejected for SMTP and IMAP hosts unless you opt in for local testing."],
      ["alert", "Bounded everything", "Rate limits on sign-in, sign-up, refresh, the widget and the webhook. Request bodies, uploads, pages and IMAP batches are all capped."],
    ]},
    { title: "Operate it", blurb: "One host is enough.", items: [
      ["server", "Docker Compose or systemd", "Compose with Postgres, API and web; or the `api` binary under a hardened unit. Caddy in front for TLS and one origin."],
      ["disk", "Backup and smoke scripts", "`scripts/backup.sh` writes a `pg_dump` plus the attachment tarball with 14-day retention. `scripts/smoke.sh` checks health, headers, CORS and body limits after every deploy."],
      ["code", "HTTP API, v3", "Every route the UI uses is documented in `docs/API.md`, with a consistent envelope and typed error codes."],
      ["package", "No paid services", "No hosted queue, search, storage or email vendor is required. Postgres and an SMTP server are the only dependencies."],
    ]},
  ],

  howTitle: "One email, followed all the way through.",
  howLede: "From a customer's inbox to a closed ticket, with every hop a real job in the worker.",
  steps: [
    ["A customer writes in", "Through the widget, by emailing the project's address, or an agent opens the ticket on their behalf."],
    ["Mail is pulled or pushed", "The API polls the project's IMAP mailbox, or your provider posts a signed webhook. Repeats are dropped by `Message-ID`."],
    ["It finds its thread", "The `[Ticket #N]` tag or the reply headers match an existing ticket, but only when the sender is that ticket's client. Anything else opens a new ticket. Quoted text is stripped, and a reply reopens a resolved or closed ticket."],
    ["Agents work the queue", "Unread tickets sort first, presence shows who is already on it, notes stay internal, canned responses fill the reply box."],
    ["The reply goes out", "The message and its mail job commit together. The outbox worker sends over the project's SMTP with `In-Reply-To` and `References`, so the customer's reply threads back."],
    ["Resolved becomes closed", "After the project's idle window, the hourly job closes it. A later customer reply reopens it."],
  ],

  installTitle: "Run it your way.",
  installLedePending: "Run it with Docker Compose from a clone, or build from source. Binaries, images and checksums arrive with the first tagged release.",
  installLede: "Download the binary, pull the images, or build from source. SHA-256 checksums are attached to every release.",
  install: [
    { id: "docker", label: "Docker", cmds: [
      "git clone https://github.com/pratham-projects/sendesk && cd sendesk",
      "cp .env.example .env   # fill POSTGRES_PASSWORD, JWT_SECRET, SD_SECRET_KEY, SD_BOOTSTRAP_PASSWORD",
      "docker compose up --build",
      "docker compose run --rm app bootstrap --email admin@example.com --name Admin",
    ], note: "Builds the API and web images from the clone. UI on `localhost:4000`, API on `localhost:3000`, caught mail on `localhost:8025`." },
    { id: "source", label: "Source", cmds: [
      "git clone https://github.com/pratham-projects/sendesk && cd sendesk",
      "cargo build --release -p api --locked",
      "# export DATABASE_URL, JWT_SECRET and SD_SECRET_KEY first, see .env.example",
      "./target/release/api migrate && ./target/release/api serve",
      "cd apps/web && bun install && bun run dev",
    ], note: "Rust stable and PostgreSQL 16, plus bun for the UI. The first admin comes from `api bootstrap`, see the quickstart." },
    { id: "images", label: "Images", cmds: [
      "V=v0.1.0; B=https://github.com/pratham-projects/sendesk/releases/download/$V",
      "mkdir deskpost && cd deskpost",
      "curl -LO $B/docker-compose.yml && curl -L $B/.env.example -o .env",
      "# edit .env: POSTGRES_PASSWORD, JWT_SECRET, SD_SECRET_KEY, FRONTEND_URL",
      "docker compose pull && docker compose up -d --no-build",
      "docker compose run --rm -e SD_BOOTSTRAP_PASSWORD app bootstrap --email you@example.com --name \"You\"",
    ], note: "Images are `ghcr.io/pratham-projects/sendesk-api` and `sendesk-web`. Set `V` to a tag that exists. UI on `127.0.0.1:4000`, API on `127.0.0.1:3000`: put Caddy in front.", pending: "No images are published yet: they arrive with the first release. Use the Docker tab today." },
    { id: "binary", label: "Binary", cmds: [
      "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/pratham-projects/sendesk/releases/latest/download/api-installer.sh | sh",
      "# export DATABASE_URL, JWT_SECRET and SD_SECRET_KEY first, see .env.example",
      "api migrate",
      "SD_BOOTSTRAP_PASSWORD='choose-a-long-one' api bootstrap --email you@example.com --name \"You\"",
      "api serve",
    ], note: "Installs `api` to `~/.cargo/bin` for macOS and Linux, x86_64 and arm64. Needs PostgreSQL 16. It is the API only: serve the UI from the web image or a static export.", pending: "No release is tagged yet: this URL works from the first release." },
  ],

  faq: [
    ["Is it production ready?", "Not yet. It is pre-release and there is no tagged release yet. It has unit, property, integration and browser tests and a reviewed hardening pass, but the container build is not exercised in CI. `SECURITY.md` lists the accepted risks."],
    ["Why Deskpost, when the repo says sendesk?", "Deskpost is the name of this site. The project is still `sendesk` on GitHub: the binary is `api`, environment variables start with `SD_`, and the systemd unit is `senddesk-api`."],
    ["Do I need AWS or a paid email service?", "No. Outbound mail is any SMTP server and inbound is IMAP polling or a signed webhook. Gmail, Brevo, Resend and Mailgun settings are in the free email guide, and each project can use a different provider."],
    ["Can one install serve several clients?", "Yes. A project is the tenant: its own members, clients, templates, mail settings and API key. Queries are scoped by project membership and a non-member gets `404`."],
    ["Where do attachments live?", "On the server's filesystem under `STORAGE_DIR`, with only a storage key in Postgres. There is no S3 driver. Back up the database and the directory together."],
    ["Which license?", "AGPL-3.0-only, as declared in `Cargo.toml`."],
  ],

  footBlurb: "A self-hosted support inbox. Repository `sendesk`, binary `api`.",
  footNote: "Pre-release. Figures are measured on one laptop, not promised.",
};
