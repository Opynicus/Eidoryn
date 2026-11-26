//! File evidence for ambiguous and unnumbered cartridges. Only scraping reads a full ROM.
use super::{ProviderError, Rom};
use crate::library::{GamePlatform, key_path};
use rusqlite::{Connection, params};
use std::{
    fs::{File, Metadata},
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) enum Proof {
    Size(u64),
    Digest(Rom),
}

fn digest(path: &Path, initial: &Metadata, cancel: &AtomicBool) -> Option<Rom> {
    let size = initial.len();
    let mut file = File::open(path).ok()?.take(size + 1);
    let mut crc = crc32fast::Hasher::new();
    let mut sha1 = sha1_smol::Sha1::new();
    let mut bytes = [0u8; 8192];
    let mut read = 0;
    while !cancel.load(Ordering::Relaxed) {
        let count = file.read(&mut bytes).ok()?;
        if count == 0 {
            let after = path.metadata().ok()?;
            if read != size
                || after.len() != size
                || after.modified().ok()? != initial.modified().ok()?
            {
                return None;
            }
            return Some(Rom {
                size,
                crc: crc.finalize(),
                sha1: Some(sha1.digest().to_string()),
            });
        }
        read += count as u64;
        crc.update(&bytes[..count]);
        sha1.update(&bytes[..count]);
    }
    None
}

pub(super) fn inspect(
    conn: &Connection,
    id: i64,
    platform: GamePlatform,
    code: Option<&str>,
    needs_digest: impl Fn(u64) -> bool,
    cancel: &AtomicBool,
) -> Result<Option<Proof>, ProviderError> {
    let (header_len, max_len) = match platform {
        GamePlatform::Gba => (eidoryn_gba::HEADER_LEN, eidoryn_gba::MAX_ROM_LEN),
        GamePlatform::Nds => (eidoryn_nds::HEADER_LEN, eidoryn_nds::MAX_ROM_LEN),
        GamePlatform::Ps1 => return Ok(None),
    };
    let locations = conn.prepare_cached(
        "SELECT l.path FROM locations l JOIN discs d ON l.disc_id=d.id
         WHERE d.game_id=?1 AND (?2 IS NULL OR d.serial=?2) AND l.current AND l.available ORDER BY l.id",
    )?.query_map(params![id,code], |r| Ok(key_path(r.get(0)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for path in locations {
        let Ok(meta) = path.metadata() else {
            continue;
        };
        if !meta.is_file() || !(header_len as u64..=max_len as u64).contains(&meta.len()) {
            continue;
        }
        if !needs_digest(meta.len()) {
            return Ok(Some(Proof::Size(meta.len())));
        }
        let rom = digest(&path, &meta, cancel);
        if cancel.load(Ordering::Relaxed) {
            return Err(ProviderError::Cancelled);
        }
        if let Some(rom) = rom {
            return Ok(Some(Proof::Digest(rom)));
        }
    }
    Ok(None)
}
