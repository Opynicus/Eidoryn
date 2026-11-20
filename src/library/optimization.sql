ALTER TABLE cover_assets ADD COLUMN content_hash TEXT;
ALTER TABLE cover_assets ADD COLUMN mime_type TEXT;
ALTER TABLE cover_assets ADD COLUMN width INTEGER;
ALTER TABLE cover_assets ADD COLUMN height INTEGER;
ALTER TABLE cover_assets ADD COLUMN downloaded_at INTEGER;
CREATE INDEX cover_original_refs ON cover_assets(original);

ALTER TABLE games ADD COLUMN platform TEXT NOT NULL DEFAULT 'PS1';
ALTER TABLE games ADD COLUMN sort_title TEXT;
ALTER TABLE games ADD COLUMN description TEXT;
ALTER TABLE games ADD COLUMN developer TEXT;
ALTER TABLE games ADD COLUMN publisher TEXT;
ALTER TABLE games ADD COLUMN created_at INTEGER;
ALTER TABLE games ADD COLUMN updated_at INTEGER;
CREATE TRIGGER game_created AFTER INSERT ON games BEGIN
    UPDATE games SET created_at=unixepoch(),updated_at=unixepoch() WHERE id=NEW.id;
END;
CREATE TRIGGER release_changed AFTER UPDATE OF release_id,auto_title,auto_region,auto_date,user_title,user_region,user_date ON game_metadata
WHEN (OLD.release_id,OLD.auto_title,OLD.auto_region,OLD.auto_date,OLD.user_title,OLD.user_region,OLD.user_date)
  IS NOT (NEW.release_id,NEW.auto_title,NEW.auto_region,NEW.auto_date,NEW.user_title,NEW.user_region,NEW.user_date)
BEGIN
    UPDATE games SET updated_at=unixepoch() WHERE id=NEW.game_id;
END;
CREATE VIEW game_works AS
SELECT g.id,g.platform,COALESCE(m.user_title,m.auto_title,g.fallback_title) AS canonical_title,
       g.sort_title,COALESCE(m.user_date,m.auto_date) AS release_date,
       g.description,g.developer,g.publisher,COALESCE(u.original,c.original) AS cover_asset_id,
       g.created_at,g.updated_at
FROM games g LEFT JOIN game_metadata m ON m.game_id=g.id
LEFT JOIN cover_assets u ON u.game_id=g.id AND u.kind='user'
LEFT JOIN cover_assets c ON c.game_id=g.id AND c.kind='download' AND c.serial=COALESCE(m.release_serial,m.disc_serial);

ALTER TABLE sessions ADD COLUMN started_at INTEGER;
ALTER TABLE sessions ADD COLUMN ended_at INTEGER;
ALTER TABLE sessions ADD COLUMN exit_reason TEXT;
UPDATE sessions SET ended_at=last_played, exit_reason='legacy';
CREATE INDEX open_play_sessions ON sessions(session_id) WHERE ended_at IS NULL;
CREATE VIEW play_sessions AS
SELECT session_id,game_id,started_at,ended_at,seconds AS duration_seconds,last_played,exit_reason
FROM sessions;

-- Aliases, relocated files and individual discs keep their existing identities.
ALTER TABLE locations ADD COLUMN last_seen_at INTEGER;
ALTER TABLE locations ADD COLUMN availability_check TEXT;
CREATE TRIGGER file_seen AFTER INSERT ON locations BEGIN
    UPDATE locations SET last_seen_at=unixepoch() WHERE id=NEW.id;
END;
CREATE VIEW game_files AS
SELECT l.id,d.game_id,l.path,d.platform,p.size AS file_size,p.mtime AS modified_at,
       d.strong_id AS content_hash,d.serial,d.region,d.number AS disc_number,l.available AS availability,l.last_seen_at
FROM locations l JOIN discs d ON d.id=l.disc_id
LEFT JOIN parts p ON p.location_id=l.id AND p.path=l.path;
