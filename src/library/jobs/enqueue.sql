INSERT INTO scrape_jobs(game_id,provider,job_type,mode,dependency)
SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM games WHERE id=?1 AND NOT hidden)
ON CONFLICT(game_id,provider,job_type) WHERE status IN ('pending','retry_wait','running')
DO UPDATE SET
  mode=CASE WHEN excluded.mode='full' THEN 'full' ELSE scrape_jobs.mode END,
  dependency=CASE WHEN scrape_jobs.status='running' THEN scrape_jobs.dependency ELSE excluded.dependency END,
  next_attempt_at=CASE WHEN excluded.mode='full' AND scrape_jobs.status='retry_wait'
     AND scrape_jobs.last_error_code NOT IN ('limited','rate_limited') THEN 0 ELSE scrape_jobs.next_attempt_at END
RETURNING id AS "id!: i64"
