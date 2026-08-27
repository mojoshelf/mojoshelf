-- Track when the git URL behind a tin name changes so consumers can be
-- warned for a month afterwards (repo-swap safety).
ALTER TABLE tins ADD COLUMN prev_url TEXT;
ALTER TABLE tins ADD COLUMN url_changed_at TEXT;
