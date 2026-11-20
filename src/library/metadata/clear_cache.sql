-- Clean up older source versions even when the active catalog for the serial
-- must remain. Subsequent changes to game_metadata also release their last
-- historical reference through the release_old_response trigger.
DELETE FROM referenced_metadata_responses AS h WHERE h.serial IN clear_serials
AND NOT EXISTS (SELECT 1 FROM game_metadata m WHERE m.release_serial=h.serial
                AND m.source=h.provider AND m.source_revision=h.revision);

-- Other games may use the same disc, including a different disc in a grouped game.
-- Keep the complete candidate set for confirmed editions: retaining only the chosen
-- row could turn an ambiguous serial into a false automatic match on a later import.
DELETE FROM clear_serials WHERE serial IN (
    SELECT disc_serial FROM game_metadata WHERE game_id NOT IN clear_games
    UNION SELECT release_serial FROM game_metadata WHERE game_id NOT IN clear_games OR confirmed
    UNION SELECT serial FROM discs WHERE game_id NOT IN clear_games
);

-- The lease prevents workers from publishing results after cleanup. The per-game
-- generation also invalidates manual requests and cached lookups already in flight.
UPDATE scrape_jobs SET status='cancelled', owner=NULL, lease_until=NULL, updated_at=unixepoch()
WHERE status IN ('pending','running','retry_wait') AND game_id IN clear_games;
DELETE FROM cover_attempts WHERE game_id IN clear_games;
UPDATE game_metadata SET cache_generation=cache_generation+1, cover_revision=cover_revision+1,
    cache_cleared=1 WHERE game_id IN clear_games;
UPDATE game_metadata SET release_id=NULL, release_serial=NULL, auto_title=NULL, auto_region=NULL,
    auto_date=NULL, aliases=NULL, work_date=NULL, source=NULL, refreshed=NULL, source_revision=NULL, pending=0
WHERE game_id IN clear_games AND NOT confirmed;

DELETE FROM fetched_releases WHERE serial IN clear_serials;
DELETE FROM metadata_responses WHERE serial IN clear_serials;
-- Keep request counters monotonic across clears and subsequent re-scrapes. These
-- counters contain neither source responses nor release fields.
UPDATE metadata_requests SET revision=revision+1 WHERE serial IN clear_serials;
DROP TABLE clear_serials;
DROP TABLE clear_games;
