-- Separate verification record for builds against the Mojo nightly channel,
-- so the stable badge and the nightly early-warning signal are independent.
ALTER TABLE tins ADD COLUMN nightly_at TEXT;
ALTER TABLE tins ADD COLUMN nightly_ok INTEGER;
ALTER TABLE tins ADD COLUMN nightly_compiler TEXT;
