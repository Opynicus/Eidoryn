-- A generation invalidates an in-flight request. This flag also suppresses
-- future automatic lookups until the user restores content for this game.
ALTER TABLE game_metadata ADD COLUMN cache_cleared INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version=8;
