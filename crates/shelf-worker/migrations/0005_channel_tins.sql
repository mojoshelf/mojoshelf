-- Mirrored modular-community channel packages appear as tins of kind
-- 'channel' (binary conda packages; no git pins, versions, or dependencies
-- rows). Source tins keep kind 'source'.
ALTER TABLE tins ADD COLUMN kind TEXT NOT NULL DEFAULT 'source';
ALTER TABLE tins ADD COLUMN channel_version TEXT;
