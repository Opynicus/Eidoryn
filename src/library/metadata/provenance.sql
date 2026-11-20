ALTER TABLE metadata_responses ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE metadata_responses ADD COLUMN provider TEXT NOT NULL DEFAULT 'Wikidata (online)';
ALTER TABLE metadata_responses ADD COLUMN payload_version INTEGER NOT NULL DEFAULT 1;

ALTER TABLE game_metadata ADD COLUMN source_revision INTEGER;
CREATE INDEX game_metadata_by_response ON game_metadata(release_serial,source,source_revision)
    WHERE source_revision IS NOT NULL;

-- A v3 library kept just the newest response per serial. Attribute that response
-- only when the user's stored edition still agrees with the parsed source fields.
UPDATE game_metadata AS g SET source_revision=0
WHERE EXISTS (
    SELECT 1 FROM fetched_releases f JOIN metadata_responses r ON r.serial=f.serial
    WHERE f.id=g.release_id AND f.serial=g.release_serial AND r.provider=g.source
      AND f.source=g.source AND f.title=g.auto_title AND f.region=g.auto_region
      AND NULLIF(f.date,'') IS g.auto_date AND NULLIF(f.work_date,'') IS g.work_date
      AND COALESCE(g.aliases,'')=f.aliases
);

CREATE TABLE referenced_metadata_responses (
    serial TEXT NOT NULL, provider TEXT NOT NULL, revision INTEGER NOT NULL,
    payload_version INTEGER NOT NULL, fetched_at INTEGER NOT NULL, payload BLOB NOT NULL,
    PRIMARY KEY(serial,provider,revision)
) WITHOUT ROWID;

-- Several game records may share a disc serial but keep different confirmed
-- editions. A refresh on one game must not change another game's source evidence.
CREATE TRIGGER retain_referenced_response BEFORE UPDATE OF revision ON metadata_responses
WHEN OLD.revision<>NEW.revision AND EXISTS (
    SELECT 1 FROM game_metadata WHERE release_serial=OLD.serial AND source=OLD.provider
      AND source_revision=OLD.revision
)
BEGIN
    INSERT INTO referenced_metadata_responses(serial,provider,revision,payload_version,fetched_at,payload)
    VALUES (OLD.serial,OLD.provider,OLD.revision,OLD.payload_version,OLD.fetched_at,OLD.payload);
END;

CREATE TRIGGER release_old_response AFTER UPDATE OF release_serial,source,source_revision ON game_metadata
WHEN OLD.source_revision IS NOT NULL AND (OLD.release_serial,OLD.source,OLD.source_revision)
      IS NOT (NEW.release_serial,NEW.source,NEW.source_revision)
BEGIN
    DELETE FROM referenced_metadata_responses
    WHERE serial=OLD.release_serial AND provider=OLD.source AND revision=OLD.source_revision
      AND NOT EXISTS(SELECT 1 FROM game_metadata WHERE release_serial=OLD.release_serial
          AND source=OLD.source AND source_revision=OLD.source_revision);
END;

CREATE TRIGGER release_deleted_response AFTER DELETE ON game_metadata
WHEN OLD.source_revision IS NOT NULL
BEGIN
    DELETE FROM referenced_metadata_responses
    WHERE serial=OLD.release_serial AND provider=OLD.source AND revision=OLD.source_revision
      AND NOT EXISTS(SELECT 1 FROM game_metadata WHERE release_serial=OLD.release_serial
          AND source=OLD.source AND source_revision=OLD.source_revision);
END;

CREATE VIEW metadata_sources AS
SELECT g.game_id,g.source AS provider,g.release_id AS external_id,
       COALESCE(r.fetched_at,h.fetched_at,g.refreshed) AS fetched_at,
       COALESCE(r.payload_version,h.payload_version) AS source_payload_version,
       COALESCE(r.payload,h.payload) AS raw_payload,CASE WHEN g.confirmed THEN 2 ELSE 1 END AS priority
FROM game_metadata g
LEFT JOIN metadata_responses r ON g.release_serial=r.serial AND g.source=r.provider
    AND g.source_revision=r.revision
LEFT JOIN referenced_metadata_responses h ON g.release_serial=h.serial AND g.source=h.provider
    AND g.source_revision=h.revision
WHERE g.release_id IS NOT NULL;

CREATE TABLE release_names (
    name TEXT NOT NULL,
    release_id TEXT NOT NULL REFERENCES fetched_releases(id) ON DELETE CASCADE,
    PRIMARY KEY (name, release_id)
) WITHOUT ROWID;
CREATE INDEX release_names_by_release ON release_names(release_id);
