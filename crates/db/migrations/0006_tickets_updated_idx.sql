-- Inbox list orders a project's tickets by recency; without this it sorts the whole project on every page.
CREATE INDEX tickets_project_updated_idx ON tickets (project_id, updated_at DESC, id DESC);
