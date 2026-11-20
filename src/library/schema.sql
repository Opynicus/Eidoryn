CREATE TABLE games(id INTEGER PRIMARY KEY, fallback_title TEXT NOT NULL, hidden INTEGER NOT NULL DEFAULT 0);
CREATE TABLE discs(
    id INTEGER PRIMARY KEY, home_game_id INTEGER NOT NULL UNIQUE REFERENCES games(id),
    game_id INTEGER NOT NULL REFERENCES games(id), platform TEXT NOT NULL DEFAULT 'PS1',
    number INTEGER NOT NULL DEFAULT 1, serial TEXT, region TEXT, boot TEXT, sample TEXT NOT NULL,
    strong_id TEXT, copy_of INTEGER REFERENCES discs(id)
);
CREATE INDEX discs_by_game ON discs(game_id);
CREATE INDEX discs_by_sample ON discs(sample);
CREATE INDEX discs_by_identity ON discs(strong_id) WHERE strong_id IS NOT NULL;
CREATE TABLE locations(
    id INTEGER PRIMARY KEY, disc_id INTEGER NOT NULL REFERENCES discs(id), path BLOB NOT NULL,
    current INTEGER NOT NULL DEFAULT 1, available INTEGER NOT NULL DEFAULT 1, fast_boot INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX current_path ON locations(path) WHERE current=1;
CREATE TABLE parts(location_id INTEGER NOT NULL REFERENCES locations(id), path BLOB NOT NULL,
    size INTEGER NOT NULL, mtime INTEGER NOT NULL, PRIMARY KEY(location_id,path));
CREATE INDEX parts_by_path ON parts(path);
CREATE TABLE roots(path BLOB PRIMARY KEY,recursive INTEGER NOT NULL,available INTEGER NOT NULL DEFAULT 1,
    added INTEGER,existing INTEGER,pending INTEGER,failed INTEGER);
CREATE TABLE ignored(path BLOB PRIMARY KEY);
CREATE TABLE migrated_recent(path BLOB PRIMARY KEY);
CREATE TABLE sessions(session_id TEXT PRIMARY KEY, game_id INTEGER NOT NULL REFERENCES games(id),
    seconds INTEGER NOT NULL CHECK(seconds>=0), last_played INTEGER NOT NULL,
    cleared_seconds INTEGER NOT NULL DEFAULT 0, cleared_at INTEGER);
CREATE INDEX sessions_by_home_game ON sessions(game_id);

-- A track indexed before its CUE appeared retains its own history. The card
-- becomes visible again when no available CUE owns that path. Obsolete paths
-- remain available for relocation unless already represented on the same card.
CREATE VIEW listed_locations AS
SELECT l.* FROM locations l WHERE NOT EXISTS (
    SELECT 1 FROM parts p JOIN locations cue ON cue.id=p.location_id
    WHERE p.path=l.path AND cue.path<>p.path AND cue.current AND cue.available
) AND (l.current OR NOT EXISTS (
    SELECT 1 FROM locations dest JOIN discs d ON d.id=dest.disc_id
    JOIN discs old ON old.id=l.disc_id
    WHERE dest.current AND dest.path=l.path AND old.game_id=d.game_id
));
