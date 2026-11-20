UPDATE scrape_jobs SET status='running',owner=?1,lease_until=unixepoch()+?2,
    attempt_count=attempt_count+1,updated_at=unixepoch()
WHERE id IN (
    SELECT j.id FROM scrape_jobs j INDEXED BY scrape_ready
    WHERE j.job_type=?3 AND j.dependency IS NULL
      AND j.status IN ('pending','retry_wait') AND j.next_attempt_at<=unixepoch()
      AND j.id IN target_jobs
    ORDER BY j.next_attempt_at,j.id LIMIT ?4
)
RETURNING id,game_id,mode,provider,attempt_count,max_attempts
