-- SendDesk schema, Postgres 16. IDs are UUIDs minted in Rust (v7); no PG18 functions.
-- Tenant = project. Every child row carries project_id and is tied to its parent with a composite FK,
-- so a row can never point at another tenant's ticket/client even through a bug in the application.

CREATE TABLE users (
    id            uuid PRIMARY KEY,
    email         text NOT NULL CHECK (length(email) BETWEEN 3 AND 255),
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 255),
    password_hash text NOT NULL,
    role          text NOT NULL DEFAULT 'agent' CHECK (role IN ('admin', 'agent')),
    status        text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'inactive', 'suspended')),
    avatar_url    text,
    phone         text CHECK (length(phone) <= 20),
    timezone      text NOT NULL DEFAULT 'UTC',
    last_login_at timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_email_key ON users (lower(email));

-- One family per sign-in; every refresh rotates the token inside the family.
CREATE TABLE refresh_families (
    id             uuid PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at     timestamptz NOT NULL,  -- absolute cap (90 days)
    revoked_at     timestamptz,
    revoked_reason text CHECK (revoked_reason IN ('logout', 'logout_all', 'reuse_detected', 'expired', 'user_inactive')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    CHECK ((revoked_at IS NULL) = (revoked_reason IS NULL))
);
CREATE INDEX refresh_families_user_idx ON refresh_families (user_id);

CREATE TABLE refresh_tokens (
    id          uuid PRIMARY KEY,
    family_id   uuid NOT NULL REFERENCES refresh_families (id) ON DELETE CASCADE,
    token_hash  text NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    rotated_at  timestamptz,              -- set when exchanged; presenting it again is reuse
    replaced_by uuid REFERENCES refresh_tokens (id) ON DELETE SET NULL,
    ip_address  text,
    user_agent  text,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX refresh_tokens_family_idx ON refresh_tokens (family_id);
CREATE INDEX refresh_tokens_expires_idx ON refresh_tokens (expires_at);

CREATE TABLE projects (
    id                       uuid PRIMARY KEY,
    name                     text NOT NULL CHECK (length(name) BETWEEN 1 AND 255),
    slug                     text NOT NULL CHECK (slug ~ '^[a-z0-9-]{3,255}$'),
    domain                   text CHECK (length(domain) <= 255),
    description              text,
    api_key_hash             text NOT NULL UNIQUE,   -- hash only; the key is shown once
    api_key_prefix           text NOT NULL,
    owner_user_id            uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    status                   text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'inactive', 'suspended')),
    is_archived              boolean NOT NULL DEFAULT false,
    -- outbound mail
    reply_to_email           text,
    smtp_email               text,
    smtp_username            text,
    smtp_password_encrypted  text,
    smtp_host                text,
    smtp_port                integer NOT NULL DEFAULT 587 CHECK (smtp_port BETWEEN 1 AND 65535),
    email_from_name          text NOT NULL DEFAULT 'Support',
    enable_email_threading   boolean NOT NULL DEFAULT true,
    -- inbound mail (IMAP polling)
    imap_host                text,
    imap_port                integer CHECK (imap_port BETWEEN 1 AND 65535),
    imap_username            text,
    imap_password_encrypted  text,
    imap_folder              text NOT NULL DEFAULT 'INBOX',
    imap_uid_validity        bigint,
    imap_last_uid            bigint NOT NULL DEFAULT 0 CHECK (imap_last_uid >= 0),
    -- behaviour and limits
    auto_close_days          integer NOT NULL DEFAULT 7 CHECK (auto_close_days >= 0),  -- 0 = never
    storage_quota_mb         integer NOT NULL DEFAULT 1024 CHECK (storage_quota_mb >= 0),
    next_ticket_number       bigint NOT NULL DEFAULT 1 CHECK (next_ticket_number >= 1),
    created_at               timestamptz NOT NULL DEFAULT now(),
    updated_at               timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX projects_slug_key ON projects (slug);
CREATE UNIQUE INDEX projects_domain_key ON projects (lower(domain)) WHERE domain IS NOT NULL;
-- Inbound mail is routed to a project by its mailbox address, so it must identify exactly one tenant.
CREATE UNIQUE INDEX projects_smtp_email_key ON projects (lower(smtp_email)) WHERE smtp_email IS NOT NULL;
CREATE INDEX projects_owner_idx ON projects (owner_user_id);

CREATE TABLE project_members (
    id                   uuid PRIMARY KEY,
    project_id           uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    user_id              uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role                 text NOT NULL DEFAULT 'agent' CHECK (role IN ('admin', 'agent')),
    should_notify        boolean NOT NULL DEFAULT false,
    can_view_all_tickets boolean NOT NULL DEFAULT true,
    can_manage_templates boolean NOT NULL DEFAULT false,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, user_id)
);
CREATE INDEX project_members_user_idx ON project_members (user_id);

CREATE TABLE clients (
    id         uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    email      text NOT NULL CHECK (length(email) BETWEEN 3 AND 255),
    name       text CHECK (length(name) <= 255),
    phone      text CHECK (length(phone) <= 20),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);
CREATE UNIQUE INDEX clients_project_email_key ON clients (project_id, lower(email));

CREATE TABLE tickets (
    id                  uuid PRIMARY KEY,
    project_id          uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    ticket_number       bigint NOT NULL CHECK (ticket_number >= 1),
    client_id           uuid NOT NULL,
    assigned_to_user_id uuid,
    created_by_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    subject             text NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
    description         text,
    source              text NOT NULL DEFAULT 'web' CHECK (source IN ('web', 'admin', 'email')),
    status              text NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'in_progress', 'on_hold', 'resolved', 'closed')),
    priority            text NOT NULL DEFAULT 'normal' CHECK (priority IN ('low', 'normal', 'high', 'urgent')),
    first_response_at   timestamptz,
    closed_at           timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, ticket_number),
    UNIQUE (project_id, id),
    -- NO ACTION (not RESTRICT) so deleting a project can cascade to clients and tickets in one statement.
    FOREIGN KEY (project_id, client_id) REFERENCES clients (project_id, id),
    -- assignee must be a member; removing the member unassigns the ticket.
    FOREIGN KEY (project_id, assigned_to_user_id) REFERENCES project_members (project_id, user_id)
        ON DELETE SET NULL (assigned_to_user_id)
);
CREATE INDEX tickets_project_status_idx ON tickets (project_id, status, updated_at DESC);
CREATE INDEX tickets_client_idx ON tickets (client_id);
CREATE INDEX tickets_assigned_idx ON tickets (assigned_to_user_id);
CREATE INDEX tickets_auto_close_idx ON tickets (updated_at) WHERE status = 'resolved';

CREATE TABLE messages (
    id                uuid PRIMARY KEY,
    project_id        uuid NOT NULL,
    ticket_id         uuid NOT NULL,
    sender_user_id    uuid REFERENCES users (id) ON DELETE SET NULL,
    sender_type       text NOT NULL CHECK (sender_type IN ('agent', 'client')),
    sender_email      text,
    sender_name       text,
    body              text NOT NULL,
    html_body         text,
    message_type      text NOT NULL DEFAULT 'reply' CHECK (message_type IN ('reply', 'note', 'system')),
    is_internal       boolean NOT NULL DEFAULT false,
    is_published      boolean NOT NULL DEFAULT true,
    email_message_id  text,
    email_in_reply_to text,
    email_references  text,
    email_from        text,
    email_to          text,
    email_subject     text,
    email_received_at timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, id),
    FOREIGN KEY (project_id, ticket_id) REFERENCES tickets (project_id, id) ON DELETE CASCADE,
    CHECK (NOT is_internal OR sender_type = 'agent')
);
CREATE INDEX messages_ticket_idx ON messages (ticket_id, created_at DESC);
-- Email de-duplication: the same Message-ID is stored once per tenant (not globally).
CREATE UNIQUE INDEX messages_email_message_id_key ON messages (project_id, email_message_id)
    WHERE email_message_id IS NOT NULL;

CREATE TABLE attachments (
    id                  uuid PRIMARY KEY,
    project_id          uuid NOT NULL,
    ticket_id           uuid NOT NULL,
    message_id          uuid,
    filename            text NOT NULL CHECK (length(filename) BETWEEN 1 AND 255),
    mime_type           text NOT NULL CHECK (length(mime_type) <= 100),
    size_bytes          bigint NOT NULL CHECK (size_bytes >= 0),
    storage_key         text NOT NULL UNIQUE,   -- opaque key resolved by sd-storage; never a URL
    uploaded_by_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, ticket_id) REFERENCES tickets (project_id, id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, message_id) REFERENCES messages (project_id, id)
        ON DELETE SET NULL (message_id)
);
CREATE INDEX attachments_ticket_idx ON attachments (ticket_id);
CREATE INDEX attachments_message_idx ON attachments (message_id);

CREATE TABLE templates (
    id         uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    title      text NOT NULL CHECK (length(title) BETWEEN 1 AND 255),
    body       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX templates_project_idx ON templates (project_id, title);

CREATE TABLE email_responses (
    id         uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    status     text NOT NULL CHECK (status IN ('created', 'resolved')),
    subject    text NOT NULL DEFAULT '' CHECK (length(subject) <= 255),
    body       text NOT NULL,
    is_enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, status)
);

CREATE TABLE email_logs (
    id                uuid PRIMARY KEY,
    project_id        uuid NOT NULL,
    ticket_id         uuid NOT NULL,
    message_id        uuid,
    to_email          text NOT NULL,
    subject           text,
    sent_by_user_id   uuid REFERENCES users (id) ON DELETE SET NULL,
    smtp_message_id   text,
    status            text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'failed', 'bounced')),
    error             text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, ticket_id) REFERENCES tickets (project_id, id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, message_id) REFERENCES messages (project_id, id)
        ON DELETE SET NULL (message_id)
);
CREATE INDEX email_logs_ticket_idx ON email_logs (ticket_id, created_at DESC);
CREATE INDEX email_logs_smtp_id_idx ON email_logs (smtp_message_id);

-- Unread is per ticket (any member opening it clears it), at most one row.
CREATE TABLE unread_tickets (
    id         uuid PRIMARY KEY,
    project_id uuid NOT NULL,
    ticket_id  uuid NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, ticket_id) REFERENCES tickets (project_id, id) ON DELETE CASCADE
);
CREATE INDEX unread_tickets_project_idx ON unread_tickets (project_id, created_at);

CREATE TABLE active_ticket_viewers (
    ticket_id    uuid NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (ticket_id, user_id)
);
CREATE INDEX active_ticket_viewers_seen_idx ON active_ticket_viewers (last_seen_at);
