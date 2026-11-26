SELECT j.game_id, max(j.mode='full') AS full
FROM scrape_jobs j JOIN games g ON g.id=j.game_id
WHERE NOT g.hidden AND j.dependency IS NULL
  AND (j.status IN ('pending','retry_wait') AND j.next_attempt_at<=unixepoch()
       OR j.status='running' AND j.lease_until<=unixepoch())
GROUP BY j.game_id
ORDER BY full DESC, min(j.id)
LIMIT 512
