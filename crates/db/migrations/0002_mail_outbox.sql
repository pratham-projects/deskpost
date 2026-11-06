-- R23: persisted retry queue for auto-response mail, and per-project IMAP poll diagnostics.

ALTER TABLE projects
    ADD COLUMN imap_last_error     text,   -- last poll failure, sanitised (never a password)
    ADD COLUMN imap_last_polled_at timestamptz;

-- One row per auto-response to send. The mail itself is rebuilt from the ticket and the project's
-- email response at send time; the email_logs row carries the outcome.
CREATE TABLE mail_outbox (
    id              uuid PRIMARY KEY,
    project_id      uuid NOT NULL,
    ticket_id       uuid NOT NULL,
    email_log_id    uuid NOT NULL REFERENCES email_logs (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind IN ('created', 'resolved')),
    status          text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'dead')),
    attempts        integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_error      text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (project_id, ticket_id) REFERENCES tickets (project_id, id) ON DELETE CASCADE
);
CREATE INDEX mail_outbox_due_idx ON mail_outbox (next_attempt_at) WHERE status = 'pending';
CREATE INDEX mail_outbox_done_idx ON mail_outbox (updated_at) WHERE status <> 'pending';
