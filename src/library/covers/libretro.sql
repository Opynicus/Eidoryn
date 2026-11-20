-- Populated only when the user requests art. The serial index comes from Libretro.
CREATE TABLE libretro_boxart_names(
    serial TEXT NOT NULL,
    region TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY(serial,region,name)
) WITHOUT ROWID;
CREATE TABLE libretro_catalog(
    id INTEGER PRIMARY KEY CHECK(id = 1),
    sync_time INTEGER,
    retry_after INTEGER NOT NULL DEFAULT 0
);
INSERT INTO libretro_catalog(id) VALUES(1);
