-- A mailbox address only routes inbound webhook mail after someone read a code sent to that address.
ALTER TABLE projects
    ADD COLUMN mail_verified_at timestamptz,
    ADD COLUMN mail_verify_hash text,
    ADD COLUMN mail_verify_email text,
    ADD COLUMN mail_verify_expires_at timestamptz;

CREATE FUNCTION projects_reset_mail_verified() RETURNS trigger AS $$
BEGIN
    IF NEW.mail_verified_at IS NOT DISTINCT FROM OLD.mail_verified_at AND (NEW.smtp_email IS DISTINCT FROM OLD.smtp_email
       OR NEW.imap_host IS DISTINCT FROM OLD.imap_host
       OR NEW.imap_username IS DISTINCT FROM OLD.imap_username
       OR NEW.smtp_host IS DISTINCT FROM OLD.smtp_host
       OR NEW.smtp_username IS DISTINCT FROM OLD.smtp_username) THEN
        NEW.mail_verified_at := NULL;
    END IF;
    RETURN NEW;
END $$ LANGUAGE plpgsql;

CREATE TRIGGER projects_reset_mail_verified
    BEFORE UPDATE OF smtp_email, imap_host, imap_username, smtp_host, smtp_username ON projects
    FOR EACH ROW EXECUTE FUNCTION projects_reset_mail_verified();
