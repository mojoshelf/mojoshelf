-- Agent-facing precomputed tin cards (markdown, refreshed by the sync cron)
-- and consumer smoke-test results reported by the tin-smoke workflow.
ALTER TABLE tins ADD COLUMN card TEXT;
ALTER TABLE tins ADD COLUMN card_at TEXT;
ALTER TABLE tins ADD COLUMN verified_at TEXT;
ALTER TABLE tins ADD COLUMN verified_ok INTEGER;
ALTER TABLE tins ADD COLUMN verified_compiler TEXT;
