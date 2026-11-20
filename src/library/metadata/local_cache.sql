ALTER TABLE fetched_releases ADD COLUMN source TEXT NOT NULL DEFAULT 'Wikidata (online)';

-- Previous versions could match an edition from a shipped directory. Keep only
-- editions this user actually matched, respecting any newer online candidates.
INSERT OR IGNORE INTO fetched_releases(id,serial,region,title,date,aliases,work_date,source)
SELECT m.release_id,m.release_serial,m.auto_region,m.auto_title,COALESCE(m.auto_date,''),
       COALESCE(m.aliases,''),COALESCE(m.work_date,''),m.source
FROM game_metadata m
WHERE m.release_id IS NOT NULL AND m.release_serial IS NOT NULL
  AND m.auto_region IS NOT NULL AND m.auto_title IS NOT NULL AND m.source IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM fetched_releases f WHERE f.serial=m.release_serial)
ORDER BY m.refreshed DESC, m.game_id;

CREATE TABLE metadata_responses(
    serial TEXT PRIMARY KEY,
    payload BLOB NOT NULL,
    fetched_at INTEGER NOT NULL
) WITHOUT ROWID;
