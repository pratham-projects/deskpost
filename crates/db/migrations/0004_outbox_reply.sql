-- Agent replies go through the outbox too: one durable, retried send path for every outgoing mail.
-- A reply job points at its message; body and attachments are read at send time.
ALTER TABLE mail_outbox
    ADD COLUMN message_id        uuid REFERENCES messages (id) ON DELETE CASCADE,
    ADD COLUMN sent_by_user_id   uuid REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE mail_outbox DROP CONSTRAINT mail_outbox_kind_check;
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_kind_check CHECK (kind IN ('created', 'resolved', 'reply'));
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_reply_has_message CHECK ((kind = 'reply') = (message_id IS NOT NULL));
