-- The only catalog identifiers kept for an explicit edition choice are the
-- preferred ID and its serial. Every downloaded field must be fetched again.
UPDATE game_metadata SET release_id=NULL,
    release_serial=CASE WHEN preferred_release_id IS NOT NULL THEN release_serial END,
    auto_title=NULL, auto_region=NULL, auto_date=NULL, aliases=NULL, work_date=NULL,
    source=NULL, refreshed=NULL, source_revision=NULL, confirmed=0, pending=0
WHERE game_id IN clear_games;

-- The source trigger releases history when the last game detaches. Also sweep
-- unreferenced rows left by older libraries.
DELETE FROM referenced_metadata_responses AS h WHERE h.serial IN clear_serials
AND NOT EXISTS (SELECT 1 FROM game_metadata m WHERE m.release_serial=h.serial
                AND m.source=h.provider AND m.source_revision=h.revision);

-- A game outside this cleanup can still use the same catalog, including an
-- unresolved candidate or another disc belonging to a grouped game.
DELETE FROM clear_serials WHERE serial IN (
    SELECT disc_serial FROM game_metadata WHERE game_id NOT IN clear_games AND (NOT cache_cleared OR pending)
    UNION SELECT release_serial FROM game_metadata WHERE game_id NOT IN clear_games AND (NOT cache_cleared OR pending)
    UNION SELECT d.serial FROM discs d LEFT JOIN game_metadata m ON m.game_id=d.game_id
          WHERE d.game_id NOT IN clear_games AND (NOT COALESCE(m.cache_cleared,0) OR m.pending)
);

DELETE FROM fetched_releases WHERE serial IN clear_serials;
DELETE FROM metadata_responses WHERE serial IN clear_serials;
-- Keep request counters monotonic across clears and subsequent re-scrapes. These
-- counters contain neither source responses nor release fields.
UPDATE metadata_requests SET revision=revision+1 WHERE serial IN clear_serials;
DROP TABLE clear_serials;
DROP TABLE clear_games;
