-- "New ticket" notice to members with should_notify: one outbox job per recipient, rebuilt from the
-- ticket at send time. The email_logs row carries the recipient and the outcome.
ALTER TABLE mail_outbox DROP CONSTRAINT mail_outbox_kind_check;
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_kind_check CHECK (kind IN ('created', 'resolved', 'reply', 'new_ticket'));
