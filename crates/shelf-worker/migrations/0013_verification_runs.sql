-- The tin-smoke run behind each verification, so a "failing" badge can lead
-- to the logs that explain it instead of being a dead end.
ALTER TABLE tins ADD COLUMN verified_run_url TEXT;
ALTER TABLE tins ADD COLUMN nightly_run_url TEXT;
