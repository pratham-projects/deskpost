-- Password-reset tokens. Only the SHA-256 of the emailed token is stored; a row is spent once
-- (`used_at`) and is dead after `expires_at`. One live token per user (a new request replaces it).
CREATE TABLE password_resets (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash  text NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX password_resets_user_idx ON password_resets (user_id);

ALTER TABLE refresh_families DROP CONSTRAINT refresh_families_revoked_reason_check;
ALTER TABLE refresh_families ADD CONSTRAINT refresh_families_revoked_reason_check
    CHECK (revoked_reason IN ('logout', 'logout_all', 'reuse_detected', 'expired', 'user_inactive', 'password_reset'));
