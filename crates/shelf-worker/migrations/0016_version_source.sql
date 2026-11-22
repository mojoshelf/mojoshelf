-- Where each version was published. `tins.url` and `tins.subdirectory` say
-- where a tin lives now; a version's commit stays where it was published, so
-- a tin can move (into a monorepo subdirectory, say) without breaking the
-- versions from before the move.
--
-- NULL `url` means the tin's url and subdirectory. With `url` set,
-- `subdirectory` is the version's own, NULL meaning the repository root.
ALTER TABLE versions ADD COLUMN url TEXT;
ALTER TABLE versions ADD COLUMN subdirectory TEXT;

-- Versions published before a tin's url change came from `prev_url`, at its
-- root: subdirectories arrived in 0015, after the url changes on record.
UPDATE versions SET url = (SELECT prev_url FROM tins WHERE tins.id = versions.tin_id)
WHERE EXISTS (
    SELECT 1 FROM tins
    WHERE tins.id = versions.tin_id
      AND tins.prev_url IS NOT NULL
      AND versions.published_at < tins.url_changed_at
);
