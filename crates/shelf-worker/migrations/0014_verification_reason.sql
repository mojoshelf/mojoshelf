-- The error a failing verification actually hit, so the tin page can say what
-- is wrong instead of only that something is.
ALTER TABLE tins ADD COLUMN verified_reason TEXT;
ALTER TABLE tins ADD COLUMN nightly_reason TEXT;
