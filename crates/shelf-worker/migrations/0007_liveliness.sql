-- GitHub liveliness indicators, refreshed in batches by the sync cron.
ALTER TABLE tins ADD COLUMN stars INTEGER;
ALTER TABLE tins ADD COLUMN last_push TEXT;
ALTER TABLE tins ADD COLUMN commits_month INTEGER;
ALTER TABLE tins ADD COLUMN commits_year INTEGER;
ALTER TABLE tins ADD COLUMN liveliness_at TEXT;
