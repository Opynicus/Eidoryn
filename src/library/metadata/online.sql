CREATE TABLE fetched_releases (
    id TEXT PRIMARY KEY,
    serial TEXT NOT NULL,
    region TEXT NOT NULL,
    title TEXT NOT NULL,
    date TEXT NOT NULL,
    aliases TEXT NOT NULL,
    work_date TEXT NOT NULL
);
CREATE INDEX fetched_releases_by_serial ON fetched_releases(serial);

-- Reserve a sequence before leaving SQLite to query the source. A slower
-- response must not replace a newer response for the same serial.
CREATE TABLE metadata_requests (
    serial TEXT PRIMARY KEY,
    revision INTEGER NOT NULL
);
