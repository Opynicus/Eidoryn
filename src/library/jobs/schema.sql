CREATE TABLE scrape_jobs(
    id INTEGER PRIMARY KEY,
    game_id INTEGER NOT NULL REFERENCES games(id),
    provider TEXT NOT NULL,
    job_type TEXT NOT NULL CHECK(job_type IN ('metadata','cover')),
    mode TEXT NOT NULL CHECK(mode IN ('full','incremental')),
    dependency INTEGER REFERENCES scrape_jobs(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN
        ('pending','running','succeeded','retry_wait','failed','cancelled')),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    lease_until INTEGER,
    owner TEXT,
    last_error_code TEXT,
    last_error_message TEXT,
    outcome TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE UNIQUE INDEX scrape_active ON scrape_jobs(game_id,provider,job_type)
    WHERE status IN ('pending','retry_wait','running');
CREATE INDEX scrape_due ON scrape_jobs(status,next_attempt_at,lease_until);
CREATE INDEX scrape_game ON scrape_jobs(game_id,id);
CREATE INDEX scrape_dependency ON scrape_jobs(dependency);

-- The publisher makes the dependent cover eligible (or skips it) at the same
-- commit boundary. Claiming the next slot can then seek directly into ready work.
CREATE TRIGGER scrape_release_cover AFTER UPDATE OF status ON scrape_jobs
WHEN NEW.job_type='metadata' AND NEW.status IN ('succeeded','failed','cancelled')
BEGIN
    UPDATE scrape_jobs SET dependency=NULL,
        outcome=CASE WHEN NEW.outcome='skip_cover' THEN 'skipped' ELSE outcome END,
        status=CASE WHEN NEW.status='cancelled' THEN 'cancelled'
                    WHEN NEW.outcome='skip_cover' THEN 'succeeded' ELSE status END,
        updated_at=unixepoch()
    WHERE dependency=NEW.id AND status IN ('pending','retry_wait');
END;
CREATE INDEX scrape_ready ON scrape_jobs(job_type,next_attempt_at,id)
    WHERE dependency IS NULL AND status IN ('pending','retry_wait');
CREATE INDEX scrape_leases ON scrape_jobs(lease_until) WHERE status='running';

-- Keep the full job history, but offer retries only for the latest source for a stage.
CREATE INDEX scrape_stage_history ON scrape_jobs(game_id,job_type,id);
CREATE VIEW latest_scrape_failures AS
SELECT j.* FROM scrape_jobs j JOIN games g ON g.id=j.game_id
WHERE NOT g.hidden AND j.status IN ('failed','retry_wait') AND NOT EXISTS(
    SELECT 1 FROM scrape_jobs n WHERE n.game_id=j.game_id AND n.job_type=j.job_type AND n.id>j.id
);
