-- A running incremental lookup may have already accepted a cache hit. A fresh
-- request must also supersede outstanding artwork jobs for a previous source.
UPDATE scrape_jobs SET status='cancelled',owner=NULL,lease_until=NULL,updated_at=unixepoch()
WHERE game_id=?1 AND (
    job_type='cover' AND provider<>CASE WHEN (SELECT platform FROM games WHERE id=?1) IN ('GBA','NDS') THEN 'libretro' ELSE ?2 END AND status IN ('pending','running','retry_wait')
    OR mode='incremental' AND status='running' AND (job_type='metadata' OR provider=CASE WHEN (SELECT platform FROM games WHERE id=?1) IN ('GBA','NDS') THEN 'libretro' ELSE ?2 END)
)
