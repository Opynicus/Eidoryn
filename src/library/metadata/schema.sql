CREATE TABLE game_metadata(
    game_id INTEGER PRIMARY KEY REFERENCES games(id),
    disc_serial TEXT, disc_region TEXT, filename TEXT NOT NULL,
    release_id TEXT, release_serial TEXT, auto_title TEXT, auto_region TEXT, auto_date TEXT, aliases TEXT, work_date TEXT,
    source TEXT, refreshed INTEGER, confirmed INTEGER NOT NULL DEFAULT 0, pending INTEGER NOT NULL DEFAULT 0,
    user_title TEXT, user_region TEXT, user_date TEXT, cover_revision INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE cover_assets(
    game_id INTEGER NOT NULL REFERENCES games(id), kind TEXT NOT NULL CHECK(kind IN ('user','download')),
    serial TEXT, source TEXT NOT NULL, original TEXT NOT NULL, thumb TEXT NOT NULL, PRIMARY KEY(game_id,kind)
);
CREATE TABLE cover_attempts(
    game_id INTEGER NOT NULL REFERENCES games(id), serial TEXT NOT NULL, url TEXT NOT NULL,
    state TEXT NOT NULL, retry_after INTEGER NOT NULL DEFAULT 0, updated INTEGER NOT NULL,
    PRIMARY KEY(game_id,serial,url)
);
CREATE TABLE cover_rate_limits(host TEXT PRIMARY KEY, retry_after INTEGER NOT NULL);
