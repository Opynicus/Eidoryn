INSERT INTO scrape_jobs(game_id,provider,job_type,mode,dependency)
SELECT ?1,?2,?3,?4,
    CASE WHEN ?4='full' AND ?2 IN ('libretro','screenscraper','custom')
              AND EXISTS(SELECT 1 FROM scrape_jobs WHERE id=?5 AND status='retry_wait')
              AND EXISTS(SELECT 1 FROM game_metadata WHERE game_id=?1
                         AND COALESCE(release_serial,disc_serial,'')<>'')
         THEN NULL ELSE ?5 END
WHERE EXISTS(SELECT 1 FROM games WHERE id=?1 AND NOT hidden)
  AND (?4='full' OR NOT EXISTS(SELECT 1 FROM game_metadata WHERE game_id=?1 AND cache_cleared))
ON CONFLICT(game_id,provider,job_type) WHERE status IN ('pending','retry_wait','running')
DO UPDATE SET
  mode=CASE WHEN excluded.mode='full' THEN 'full' ELSE scrape_jobs.mode END,
  dependency=CASE WHEN scrape_jobs.status='running' OR (scrape_jobs.mode='full' AND excluded.mode='incremental')
    THEN scrape_jobs.dependency ELSE excluded.dependency END,
  next_attempt_at=CASE WHEN excluded.mode='full' AND scrape_jobs.status='retry_wait'
     AND scrape_jobs.last_error_code NOT IN ('limited','rate_limited') THEN 0 ELSE scrape_jobs.next_attempt_at END
RETURNING id AS "id!: i64"
