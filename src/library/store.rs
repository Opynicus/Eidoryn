use super::{Game, IndexResult, Library, Location, key_path, metadata, path_key};
use crate::preferences::RecentDisc;
use eidoryn_ps1::disc::{DiscImage, DiscProbe, DiscRegion};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    size: u64,
    mtime: i64,
}

impl Stamp {
    fn read(path: &Path) -> IndexResult<Self> {
        let meta = fs::metadata(path)?;
        let time = meta.modified()?;
        let mtime = match time.duration_since(UNIX_EPOCH) {
            Ok(value) => i64::try_from(value.as_nanos())?,
            Err(error) => -i64::try_from(error.duration().as_nanos())?,
        };
        Ok(Self {
            path: path.to_owned(),
            size: meta.len(),
            mtime,
        })
    }

    fn unchanged(&self) -> bool {
        Self::read(&self.path).is_ok_and(|now| now == *self)
    }
}

#[derive(PartialEq, Eq)]
struct Indexed {
    location: i64,
    disc: i64,
    home: i64,
    game: i64,
    sample: String,
    strong: Option<String>,
}
fn current(db: &Connection, key: &[u8]) -> rusqlite::Result<Option<Indexed>> {
    db.query_row(
        "SELECT l.id,d.id,d.home_game_id,d.game_id,d.sample,d.strong_id
         FROM locations l JOIN discs d ON d.id=l.disc_id WHERE l.path=? AND l.current",
        [key],
        |r| {
            Ok(Indexed {
                location: r.get(0)?,
                disc: r.get(1)?,
                home: r.get(2)?,
                game: r.get(3)?,
                sample: r.get(4)?,
                strong: r.get(5)?,
            })
        },
    )
    .optional()
}
fn parts(db: &Connection, location: i64) -> rusqlite::Result<Vec<Stamp>> {
    db.prepare("SELECT path,size,mtime FROM parts WHERE location_id=? ORDER BY path")?
        .query_map([location], |r| {
            Ok(Stamp {
                path: key_path(r.get(0)?),
                size: r.get(1)?,
                mtime: r.get(2)?,
            })
        })?
        .collect()
}
fn write_parts(tx: &Transaction<'_>, location: i64, stamps: &[Stamp]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM parts WHERE location_id=?", [location])?;
    let mut insert =
        tx.prepare_cached("INSERT INTO parts(location_id,path,size,mtime) VALUES(?,?,?,?)")?;
    for stamp in stamps {
        insert.execute(params![
            location,
            path_key(&stamp.path),
            stamp.size,
            stamp.mtime
        ])?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Change {
    Added,
    Existing,
    Ignored,
}
pub(super) struct IndexedFile {
    pub change: Change,
    pub home: i64,
    pub dependencies: Vec<PathBuf>,
    pub pending: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Directory,
    Explicit,
}
fn restore(
    tx: &Transaction<'_>,
    entry: &Indexed,
    key: &[u8],
    mode: Mode,
    fast_boot: Option<bool>,
) -> rusqlite::Result<()> {
    if mode == Mode::Explicit {
        tx.execute("DELETE FROM ignored WHERE path=?", [key])?;
        tx.execute(
            "UPDATE games SET hidden=0 WHERE id=? AND hidden",
            [entry.game],
        )?;
    }
    tx.execute(
        "UPDATE locations SET available=1,fast_boot=COALESCE(?1,fast_boot)
        WHERE id=?2 AND (available=0 OR ( ?1 IS NOT NULL AND fast_boot<>?1))",
        params![fast_boot, entry.location],
    )?;
    Ok(())
}

fn needs_confirmation(db: &Connection, home: i64, game: i64) -> rusqlite::Result<bool> {
    if metadata::needs_match(db, game)? {
        return Ok(true);
    }
    db.prepare_cached(
        "SELECT 1
         FROM discs d
         JOIN discs candidate ON candidate.sample = d.sample AND candidate.game_id <> d.game_id
         JOIN games g ON g.id = candidate.game_id
         WHERE d.home_game_id = ? AND NOT g.hidden
               AND (d.strong_id IS NULL OR candidate.strong_id IS NULL)",
    )?
    .exists([home])
}

fn is_ignored(db: &Connection, key: &[u8]) -> rusqlite::Result<bool> {
    db.prepare_cached("SELECT 1 FROM ignored WHERE path=?")?
        .exists([key])
}

fn dependency_paths(stamps: Vec<Stamp>, path: &Path) -> Vec<PathBuf> {
    stamps
        .into_iter()
        .map(|stamp| stamp.path)
        .filter(|p| p != path)
        .collect()
}

impl Library {
    // One SQLite transaction per completed entry. Long fingerprints are calculated
    // before taking the writer lock, only for explicit verification/association.
    pub(super) fn index(
        &self,
        db: &mut Connection,
        path: &Path,
        mode: Mode,
        verified: Option<(&str, bool)>,
    ) -> IndexResult<IndexedFile> {
        let path = path.canonicalize()?;
        let key = path_key(&path);
        loop {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let entry = current(&tx, &key)?;
            let saved = entry
                .as_ref()
                .map(|v| parts(&tx, v.location))
                .transpose()?
                .unwrap_or_default();
            if mode == Mode::Directory && is_ignored(&tx, &key)? {
                return Ok(IndexedFile {
                    change: Change::Ignored,
                    home: 0,
                    dependencies: dependency_paths(saved, &path),
                    pending: false,
                });
            }
            let cached = !saved.is_empty() && saved.iter().all(Stamp::unchanged);
            if let Some(existing) = &entry
                && cached
                && verified.is_none_or(|(id, _)| existing.strong.as_deref() == Some(id))
            {
                restore(&tx, existing, &key, mode, verified.map(|(_, fast)| fast))?;
                let pending = needs_confirmation(&tx, existing.home, existing.game)?;
                tx.commit()?;
                return Ok(IndexedFile {
                    change: Change::Existing,
                    home: existing.home,
                    dependencies: dependency_paths(saved, &path),
                    pending,
                });
            }
            let initial = Stamp::read(&path)?;
            let probe = DiscImage::probe(&path)?;
            let mut stamps = vec![initial];
            for p in &probe.dependencies {
                if !stamps.iter().any(|s| s.path == *p) {
                    stamps.push(Stamp::read(p)?);
                }
            }
            // A known strong identity with matching samples can resolve a changed
            // timestamp or a possible move. A new, unrelated entry never hashes.
            let needs_identity = verified.is_some()
                || tx
                    .prepare_cached("SELECT 1 FROM discs WHERE sample=? AND strong_id IS NOT NULL")?
                    .exists([&probe.sample])?;
            let (tx, identity) = if needs_identity {
                drop(tx);
                let hash = DiscImage::open(&path)?.identity().to_owned();
                if verified.is_some_and(|(booted, _)| booted != hash) {
                    return Err("Disc changed between loading and library association".into());
                }
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if current(&tx, &key)? != entry
                    || (mode == Mode::Directory && is_ignored(&tx, &key)?)
                {
                    continue;
                }
                (tx, Some(hash))
            } else {
                (tx, None)
            };
            if !stamps.iter().all(Stamp::unchanged) {
                return Err("Disc changed during indexing; rescan it".into());
            }
            if let Some(existing) = &entry
                && existing.sample == probe.sample
                && identity.as_ref().is_some_and(|id| {
                    existing.strong.as_ref() == Some(id) || (existing.strong.is_none() && cached)
                })
            {
                write_parts(&tx, existing.location, &stamps)?;
                restore(&tx, existing, &key, mode, verified.map(|(_, v)| v))?;
                tx.execute(
                    "UPDATE discs SET strong_id=? WHERE id=?",
                    params![identity, existing.disc],
                )?;
                let pending = needs_confirmation(&tx, existing.home, existing.game)?;
                tx.commit()?;
                return Ok(IndexedFile {
                    change: Change::Existing,
                    home: existing.home,
                    dependencies: probe.dependencies,
                    pending,
                });
            }
            if let Some(existing) = &entry {
                tx.execute(
                    "UPDATE locations SET current=0,available=0 WHERE id=?",
                    [existing.location],
                )?;
            }
            let verified = identity
                .as_deref()
                .map(|id| (id, verified.is_some_and(|(_, fast)| fast)));
            return self.insert_revision(tx, &path, probe, &stamps, mode, verified);
        }
    }

    fn insert_revision(
        &self,
        tx: Transaction<'_>,
        path: &Path,
        probe: DiscProbe,
        stamps: &[Stamp],
        mode: Mode,
        verified: Option<(&str, bool)>,
    ) -> IndexResult<IndexedFile> {
        let key = path_key(path);
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        let region = probe.region.map(|r| match r {
            DiscRegion::America => "USA",
            DiscRegion::Europe => "Europe",
            DiscRegion::Japan => "Japan",
        });
        tx.execute(
            "INSERT INTO games(fallback_title) VALUES(?)",
            [title.as_ref()],
        )?;
        let home = tx.last_insert_rowid();
        let identical = if let Some((id, _)) = verified {
            tx.query_row(
                "SELECT game_id,COALESCE(copy_of,id),number
                 FROM discs WHERE strong_id=? ORDER BY id LIMIT 1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
        } else {
            None
        };
        let (game, copy_of, number) =
            identical.map_or((home, None, 1), |(g, d, n)| (g, Some(d), n));
        tx.execute(
            "INSERT INTO discs(home_game_id,game_id,serial,region,boot,sample,strong_id,number,copy_of)
             VALUES(?,?,?,?,?,?,?,?,?)",
            params![
                home,
                game,
                probe.serial,
                region,
                probe.boot_value,
                probe.sample,
                verified.map(|(identity, _)| identity),
                number,
                copy_of,
            ],
        )?;
        let disc = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO locations(disc_id,path,fast_boot) VALUES(?,?,?)",
            params![disc, key, verified.is_some_and(|(_, b)| b)],
        )?;
        write_parts(&tx, tx.last_insert_rowid(), stamps)?;
        if mode == Mode::Explicit {
            tx.execute("DELETE FROM ignored WHERE path=?", [&key])?;
            tx.execute("UPDATE games SET hidden=0 WHERE id=? AND hidden", [game])?;
        }
        metadata::identify(&tx, home, probe.serial.as_deref(), region, title.as_ref())?;
        let pending = needs_confirmation(&tx, home, game)?;
        tx.commit()?;
        // A verified copy gets a home ID for its own sessions, but shares the existing game card.
        if game == home
            && let Some(additions) = &self.additions
        {
            let _ = additions.send(game);
        }
        Ok(IndexedFile {
            change: Change::Added,
            home,
            dependencies: probe.dependencies,
            pending,
        })
    }

    pub fn associate_opened(
        &self,
        path: &Path,
        identity: &str,
        fast_boot: bool,
    ) -> Result<i64, String> {
        self.index(
            &mut self.connect()?,
            path,
            Mode::Explicit,
            Some((identity, fast_boot)),
        )
        .map(|r| r.home)
        .map_err(|e| e.to_string())
    }

    pub fn games(&self) -> Result<Vec<Game>, String> {
        self.read_games().map_err(|e| e.to_string())
    }

    fn read_games(&self) -> rusqlite::Result<Vec<Game>> {
        let mut db = self.connection()?;
        let tx = db.transaction()?;
        let mut query = tx.prepare(
            "SELECT g.id, g.fallback_title, COALESCE(stats.seconds, 0), stats.last_played
             FROM games g
             LEFT JOIN (
                 SELECT d.game_id, SUM(s.seconds - s.cleared_seconds) AS seconds,
                        MAX(CASE WHEN s.cleared_at IS NULL OR s.last_played > s.cleared_at
                                 THEN s.last_played END) AS last_played
                 FROM discs d
                 JOIN sessions s ON s.game_id = d.home_game_id
                 GROUP BY d.game_id
             ) stats ON stats.game_id = g.id
             WHERE NOT g.hidden AND EXISTS (
                 SELECT 1 FROM discs d JOIN listed_locations l ON l.disc_id = d.id
                 WHERE d.game_id = g.id
             )
             ORDER BY g.id",
        )?;
        let mut games = query
            .query_map([], |row| {
                Ok(Game {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    seconds: row.get(2)?,
                    last_played: row.get(3)?,
                    serial: None,
                    region: None,
                    locations: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let index: HashMap<_, _> = games.iter().enumerate().map(|(i, g)| (g.id, i)).collect();
        let mut query = tx.prepare(
            "SELECT d.game_id, d.serial, d.region, l.id, d.id, d.number, l.path, l.available, l.fast_boot
             FROM discs d
             JOIN listed_locations l ON l.disc_id = d.id
             JOIN games g ON g.id = d.game_id
             WHERE NOT g.hidden
             ORDER BY d.game_id, d.number, l.available DESC, l.id",
        )?;
        let mut rows = query.query([])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            if let Some(i) = index.get(&id) {
                let g = &mut games[*i];
                let serial: Option<String> = r.get(1)?;
                if g.locations.is_empty() {
                    g.serial.clone_from(&serial);
                    g.region = r.get(2)?;
                }
                g.locations.push(Location {
                    id: r.get(3)?,
                    disc_id: r.get(4)?,
                    disc_number: r.get(5)?,
                    path: key_path(r.get(6)?),
                    available: r.get(7)?,
                    fast_boot: r.get(8)?,
                    serial,
                });
            }
        }
        Ok(games)
    }

    pub fn remove(&self, game: i64) -> Result<(), String> {
        self.hide(game).map_err(|e| e.to_string())
    }

    fn hide(&self, game: i64) -> rusqlite::Result<()> {
        let mut db = self.connection()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO ignored
             SELECT l.path FROM locations l JOIN discs d ON d.id = l.disc_id
             WHERE d.game_id = ? AND l.current",
            [game],
        )?;
        tx.execute("UPDATE games SET hidden=1 WHERE id=?", [game])?;
        tx.commit()
    }

    pub fn clear_statistics(&self, game: i64) -> Result<(), String> {
        self.connect()?
            .execute(
                "UPDATE sessions SET cleared_seconds=seconds,cleared_at=unixepoch()
                 WHERE game_id IN (SELECT home_game_id FROM discs WHERE game_id = ?)",
                [game],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn group_disc(&self, disc: i64, game: i64) -> Result<(), String> {
        self.connect()?
            .execute(
                "UPDATE discs SET game_id = ?2, copy_of = NULL,
                     number = (SELECT COALESCE(MAX(number), 0) + 1 FROM discs WHERE game_id = ?2)
                 WHERE id = ?1 AND game_id <> ?2
                     AND EXISTS (SELECT 1 FROM games WHERE id = ?2 AND NOT hidden)",
                params![disc, game],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn separate_disc(&self, disc: i64) -> Result<(), String> {
        self.connect()?
            .execute(
                "UPDATE discs SET game_id=home_game_id,number=1,copy_of=NULL WHERE id=?",
                [disc],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn migrate_recents(&self, records: &[RecentDisc]) -> Result<(), String> {
        self.import_recents(records).map_err(|e| e.to_string())
    }

    fn import_recents(&self, records: &[RecentDisc]) -> IndexResult<()> {
        let mut db = self.connection()?;
        for recent in records {
            let Ok(path) = recent.path.canonicalize() else {
                continue;
            };
            let key = path_key(&path);
            if db
                .prepare_cached("SELECT 1 FROM migrated_recent WHERE path=?")?
                .exists([&key])?
            {
                continue;
            }
            // Respect removal even after a crash between indexing and migration bookkeeping.
            if self.index(&mut db, &path, Mode::Directory, None).is_err() {
                continue;
            }
            let tx = db.transaction()?;
            tx.execute(
                "UPDATE locations SET fast_boot=? WHERE current AND path=?",
                params![recent.fast_boot, key],
            )?;
            tx.execute("INSERT OR IGNORE INTO migrated_recent VALUES(?)", [key])?;
            tx.commit()?;
        }
        Ok(())
    }
}

impl Library {
    /// Offered after the user selects the new location. A known full fingerprint
    /// is binding. For a never-booted, missing original only the stored layout
    /// and samples can be compared; the user's explicit choice resolves that case.
    pub fn relocate(&self, location: i64, new_path: &Path) -> Result<(), String> {
        self.relocate_verified(location, new_path)
            .map_err(|e| e.to_string())
    }

    fn relocate_verified(&self, location: i64, new_path: &Path) -> IndexResult<()> {
        let path = new_path.canonicalize()?;
        let mut db = self.connection()?;
        let (disc, prior_path, strong, sample): (i64, Vec<u8>, Option<String>, String) = db
            .query_row(
                "SELECT d.id, l.path, d.strong_id, d.sample
                 FROM locations l JOIN discs d ON d.id = l.disc_id
                 WHERE l.id = ?",
                [location],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let old_path = key_path(prior_path.clone());
        let probe = DiscImage::probe(&path)?;
        if sample != probe.sample {
            return Err(
                "Selected image differs from the indexed disc layout or sample sectors".into(),
            );
        }
        let stamps = std::iter::once(&path)
            .chain(&probe.dependencies)
            .map(|p| Stamp::read(p))
            .collect::<IndexResult<Vec<_>>>()?;
        let verified = DiscImage::open(&path)?;
        let old_stamps = parts(&db, location)?;
        let original = if strong.is_none()
            && old_path != path
            && !old_stamps.is_empty()
            && old_stamps.iter().all(Stamp::unchanged)
        {
            Some(DiscImage::open(&old_path)?)
        } else {
            None
        };
        let expected = strong
            .as_deref()
            .or_else(|| original.as_ref().map(DiscImage::identity));
        if expected.is_some_and(|id| id != verified.identity()) {
            return Err("Selected image does not match the original full disc identity".into());
        }
        let key = path_key(&path);
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !stamps.iter().all(Stamp::unchanged) {
            return Err("Selected image changed during verification".into());
        }
        if let Some(other) = current(&tx, &key)?
            && other.location != location
        {
            // A rescan may already have indexed the destination. Keep its local history,
            // and associate it as a verified copy of the relocated disc.
            if other
                .strong
                .as_deref()
                .is_some_and(|id| id != verified.identity())
                || other.sample != sample
                || !parts(&tx, other.location)?.iter().all(Stamp::unchanged)
            {
                return Err("Another revision already occupies the destination".into());
            }
            tx.execute(
                "UPDATE locations SET current=0,available=0 WHERE id=?",
                [other.location],
            )?;
            tx.execute(
                "UPDATE discs SET game_id = (SELECT game_id FROM discs WHERE id = ?1),
                                  number = (SELECT number FROM discs WHERE id = ?1),
                                  copy_of = ?1
                 WHERE id = ?2",
                params![disc, other.disc],
            )?;
        }
        let changed = tx.execute(
            "UPDATE locations SET path=?,available=1,current=1 WHERE id=? AND path=?",
            params![key, location, prior_path],
        )?;
        if changed != 1 {
            return Err("Location was changed by another operation; refresh and retry".into());
        }
        write_parts(&tx, location, &stamps)?;
        tx.execute(
            "UPDATE discs SET strong_id=? WHERE id=?",
            params![verified.identity(), disc],
        )?;
        tx.execute("DELETE FROM ignored WHERE path=?", [&key])?;
        tx.commit()?;
        Ok(())
    }

    pub fn confirm_copy(&self, location: i64, other_location: i64) -> Result<(), String> {
        self.verify_copy(location, other_location)
            .map_err(|e| e.to_string())
    }

    fn verify_copy(&self, location: i64, other: i64) -> IndexResult<()> {
        if location == other {
            return Err("Select two distinct image locations".into());
        }
        let mut db = self.connection()?;
        let read = |id| {
            db.query_row(
                "SELECT path, disc_id FROM locations WHERE current AND id = ?",
                [id],
                |row| Ok((key_path(row.get(0)?), row.get::<_, i64>(1)?)),
            )
        };
        let (a, disc) = read(location)?;
        let (b, target) = read(other)?;
        let before = [parts(&db, location)?, parts(&db, other)?];
        let verified = DiscImage::open(&a)?;
        if verified.identity() != DiscImage::open(&b)?.identity() {
            return Err("Full disc identities differ; the locations were kept separate".into());
        }
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !before.iter().flatten().all(Stamp::unchanged) {
            return Err("A location changed during verification; rescan and retry".into());
        }
        for (path, expected_location) in [(&a, location), (&b, other)] {
            let entry = current(&tx, &path_key(path))?
                .filter(|entry| entry.location == expected_location)
                .ok_or("A location changed during verification; rescan and retry")?;
            if entry
                .strong
                .as_deref()
                .is_some_and(|identity| identity != verified.identity())
            {
                return Err("Selected image differs from its recorded full disc identity".into());
            }
        }
        tx.execute(
            "UPDATE discs SET strong_id=? WHERE id IN (?,?)",
            params![verified.identity(), disc, target],
        )?;
        tx.execute(
            "UPDATE discs SET game_id = (SELECT game_id FROM discs WHERE id = ?2),
                              number = (SELECT number FROM discs WHERE id = ?2),
                              copy_of = (SELECT COALESCE(copy_of, id) FROM discs WHERE id = ?2)
             WHERE id = ?1",
            params![disc, target],
        )?;
        tx.commit()?;
        Ok(())
    }
}
