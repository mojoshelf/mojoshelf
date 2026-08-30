-- When a publish last asked tin-smoke to verify this tin, so a burst of
-- version publishes triggers at most one verification run a day.
ALTER TABLE tins ADD COLUMN smoke_requested_at TEXT;
