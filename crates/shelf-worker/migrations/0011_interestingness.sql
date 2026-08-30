-- Forks complete the liveliness signals, and `score` caches the
-- interestingness ranking so the tin list can ORDER BY it directly. Both are
-- refreshed together by the sync cron, in refresh_liveliness().
ALTER TABLE tins ADD COLUMN forks INTEGER;
ALTER TABLE tins ADD COLUMN score REAL;
