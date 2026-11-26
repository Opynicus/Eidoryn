//! Regional Nintendo cartridge releases from Libretro's No-Intro DAT. The cartridge code comes
//! from the ROM header; local filenames are never sent to the provider.
use super::{Provenance, ProviderError, Release, SourceResponse, transport};
use crate::library::{GamePlatform, Library, http_retry::RequestLimit};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod cartridge;

pub(in crate::library) const COVER_BASE: &str = "https://raw.githubusercontent.com/libretro-thumbnails/Nintendo_-_Game_Boy_Advance/master/Named_Boxarts/";
pub(in crate::library) const NDS_COVER_BASE: &str = "https://raw.githubusercontent.com/libretro-thumbnails/Nintendo_-_Nintendo_DS/master/Named_Boxarts/";
pub(super) const PAYLOAD_VERSION: u32 = 2;
static DOWNLOAD: RequestLimit = RequestLimit::new(1, Duration::ZERO);

struct Cache {
    generation: u64,
    fetched: Option<(Instant, Result<Catalog, ProviderError>)>,
}

struct Source {
    platform: GamePlatform,
    id_prefix: &'static str,
    provider: &'static str,
    dat: &'static str,
    boxart: &'static str,
    cache: Mutex<Cache>,
}

const EMPTY_CACHE: Cache = Cache {
    generation: 0,
    fetched: None,
};

static GBA: Source = Source {
    platform: GamePlatform::Gba,
    id_prefix: "libretro-gba",
    provider: "Libretro No-Intro (GBA)",
    dat: "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/no-intro/Nintendo%20-%20Game%20Boy%20Advance.dat",
    boxart: COVER_BASE,
    cache: Mutex::new(EMPTY_CACHE),
};
static NDS: Source = Source {
    platform: GamePlatform::Nds,
    id_prefix: "libretro-nds",
    provider: "Libretro No-Intro (NDS)",
    dat: "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/no-intro/Nintendo%20-%20Nintendo%20DS.dat",
    boxart: NDS_COVER_BASE,
    cache: Mutex::new(EMPTY_CACHE),
};

fn source(key: &str) -> &'static Source {
    if key.starts_with("NDS:") || key.starts_with("nds-rom:") {
        &NDS
    } else {
        &GBA
    }
}

impl Source {
    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(super) fn is_code(serial: &str) -> bool {
    serial.len() == 4
        && serial
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

pub(super) fn cache_key(platform: Option<GamePlatform>, code: &str) -> std::borrow::Cow<'_, str> {
    if platform == Some(GamePlatform::Nds) {
        format!("NDS:{code}").into()
    } else {
        code.into()
    }
}

pub(in crate::library) fn product_code(key: &str) -> Option<&str> {
    (!is_signature(key)).then(|| key.strip_prefix("NDS:").unwrap_or(key))
}

pub(in crate::library) fn cover_base(key: &str) -> &'static str {
    source(key).boxart
}

pub(in crate::library) fn edition_id(serial: &str, name: &str) -> String {
    format!(
        "{}/{serial}/{:x}",
        source(serial).id_prefix,
        Sha256::digest(name.as_bytes())
    )
}

pub(super) fn clear_cache() {
    for catalog in [&GBA, &NDS] {
        let mut state = catalog.cache();
        state.generation = state.generation.wrapping_add(1);
        state.fetched = None;
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Rom {
    size: u64,
    crc: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha1: Option<String>,
}

impl Rom {
    fn from_dat(line: &str) -> Option<Self> {
        // Skip the quoted filename; names can contain spaces and field-like text.
        let fields = line
            .strip_prefix("rom ( name \"")?
            .rsplit_once("\" size ")?
            .1;
        let mut fields = fields.split_ascii_whitespace();
        let size = fields.next()?.parse().ok()?;
        let crc = fields.by_ref().skip_while(|&field| field != "crc").nth(1)?;
        Some(Self {
            size,
            crc: u32::from_str_radix(crc, 16).ok()?,
            sha1: fields
                .skip_while(|&field| field != "sha1")
                .nth(1)
                .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_ascii_lowercase),
        })
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Edition {
    name: String,
    region: String,
    roms: Vec<Rom>,
}

// An unnumbered release is keyed only by its verified DAT signature.
pub(super) fn is_signature(key: &str) -> bool {
    key.starts_with("gba-rom:") || key.starts_with("nds-rom:")
}

fn signature_key(platform: GamePlatform, size: u64, sha1: &str) -> String {
    let prefix = if platform == GamePlatform::Nds {
        "nds-rom"
    } else {
        "gba-rom"
    };
    format!("{prefix}:{size}:{sha1}")
}

pub(super) fn market_compatible(header: Option<&str>, market: &str) -> bool {
    header.is_none_or(|hint| {
        market == "World"
            || market
                .split([',', '/'])
                .map(str::trim)
                .any(|m| m == hint || (hint == "Europe" && crate::library::european_market(m)))
    })
}

struct Catalog(BTreeMap<String, Vec<Edition>>);

impl Catalog {
    fn parse(bytes: &[u8], cancel: &AtomicBool) -> Result<Self, ProviderError> {
        let dat = std::str::from_utf8(bytes).map_err(|e| ProviderError::Invalid(e.to_string()))?;
        let mut records = BTreeMap::<String, Vec<Edition>>::new();
        let mut game = None;
        for line in dat.lines().map(str::trim) {
            if line == "game (" {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProviderError::Cancelled);
                }
                if game.is_some() {
                    return Err(ProviderError::Invalid(
                        "Incomplete No-Intro DAT record".into(),
                    ));
                }
                game = Some(("", Edition::default()));
            } else if let Some((serial, edition)) = &mut game {
                if line == ")" {
                    let edition = std::mem::take(edition);
                    if !edition.name.is_empty() {
                        if is_code(serial) {
                            let rows = records.entry((*serial).to_owned()).or_default();
                            if let Some(prior) =
                                rows.iter_mut().find(|row| row.name == edition.name)
                            {
                                prior.roms.extend(edition.roms);
                            } else {
                                rows.push(edition);
                            }
                        } else {
                            for rom in &edition.roms {
                                if let Some(sha1) = &rom.sha1 {
                                    let rows =
                                        records.entry(format!("{}:{sha1}", rom.size)).or_default();
                                    if !rows.iter().any(|row| row.name == edition.name) {
                                        rows.push(edition.clone());
                                    }
                                }
                            }
                        }
                    }
                    game = None;
                } else if let Some(rom) = Rom::from_dat(line) {
                    edition.roms.push(rom);
                } else if let Some(line) = line.strip_suffix('"') {
                    if let Some(value) = line.strip_prefix("name \"") {
                        value.clone_into(&mut edition.name);
                    } else if let Some(value) = line.strip_prefix("region \"") {
                        edition.region = value.replace(" / ", ", ");
                    } else if let Some(value) = line.strip_prefix("serial \"") {
                        *serial = value;
                    }
                }
            }
        }
        if records.is_empty() || game.is_some() {
            return Err(ProviderError::Invalid(
                "Invalid or empty Libretro No-Intro catalog".into(),
            ));
        }
        Ok(Self(records))
    }

    fn response(&self, code: &str) -> Result<SourceResponse, ProviderError> {
        let key = code.split_once(':').map_or(code, |(_, key)| key);
        let rows = self.0.get(key).map_or(&[][..], Vec::as_slice);
        let releases = rows
            .iter()
            .map(|edition| Release {
                id: edition_id(code, &edition.name),
                serial: code.into(),
                title: edition.name.clone(),
                region: edition.region.clone(),
                date: String::new(),
                work_date: String::new(),
                aliases: String::new(),
            })
            .collect();
        Ok(SourceResponse {
            source: Provenance {
                provider: source(code).provider,
                payload_version: PAYLOAD_VERSION,
            },
            releases,
            raw: serde_json::to_vec(rows).map_err(|e| ProviderError::Invalid(e.to_string()))?,
        })
    }
}

pub(super) fn lookup(
    lib: &Library,
    code: &str,
    cancel: &AtomicBool,
) -> Result<SourceResponse, ProviderError> {
    #[cfg(feature = "ui-tests")]
    if let Some(responses) = &lib.scrape_responses {
        return recorded_response(&responses.metadata, code, cancel);
    }
    #[cfg(not(feature = "ui-tests"))]
    let _ = lib;
    let catalog = source(code);
    let _permit = DOWNLOAD.acquire_typed(cancel)?;
    let generation = {
        let state = catalog.cache();
        if let Some((at, result)) = &state.fetched {
            let ttl = if result.is_ok() {
                Duration::from_hours(24)
            } else {
                Duration::from_secs(30)
            };
            if at.elapsed() < ttl {
                return result.as_ref().map_err(Clone::clone)?.response(code);
            }
        }
        state.generation
    };
    let fetched = (|| {
        let response =
            transport::request(transport::api()?.get(catalog.dat), 8 * 1024 * 1024, cancel)?;
        if !response.status().is_success() {
            DOWNLOAD.defer(response.headers());
            return Err(ProviderError::http(response.status(), response.headers()));
        }
        Catalog::parse(response.body(), cancel)
    })();
    if matches!(fetched, Err(ProviderError::Cancelled)) {
        return Err(ProviderError::Cancelled);
    }
    let mut state = catalog.cache();
    if state.generation != generation {
        return Err(ProviderError::Cancelled);
    }
    let (_, result) = state.fetched.insert((Instant::now(), fetched));
    result.as_ref().map_err(Clone::clone)?.response(code)
}

/// Edition evidence persisted with a fetched serial. Older responses only stored titles.
pub(super) fn cached_editions(conn: &Connection, code: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT payload FROM metadata_responses WHERE serial=? AND provider=? AND payload_version=?",
        params![code, source(code).provider, PAYLOAD_VERSION],
        |row| row.get(0),
    ).optional()
}

pub(super) fn unnumbered_key(
    conn: &Connection,
    id: i64,
    platform: GamePlatform,
    cancel: &AtomicBool,
) -> Result<Option<String>, ProviderError> {
    let proof = cartridge::inspect(conn, id, platform, None, |_| true, cancel)?;
    Ok(match proof {
        Some(cartridge::Proof::Digest(rom)) => rom
            .sha1
            .map(|sha1| signature_key(platform, rom.size, &sha1)),
        _ => None,
    })
}

pub(super) fn matching_title(
    conn: &Connection,
    id: i64,
    code: &str,
    region: Option<&str>,
    raw: &[u8],
    cancel: &AtomicBool,
) -> Result<Option<String>, ProviderError> {
    let editions: Vec<Edition> =
        serde_json::from_slice(raw).map_err(|e| ProviderError::Invalid(e.to_string()))?;
    let only = match editions.as_slice() {
        [edition] if is_signature(code) || market_compatible(region, &edition.region) => {
            Some(edition)
        }
        _ => None,
    };
    if is_signature(code) || editions.iter().all(|e| e.roms.is_empty()) {
        return Ok(only.map(|e| e.name.clone()));
    }
    // Check a unique compatible code using only the file length. A different size
    // may belong to an unnumbered release. Ambiguous codes need a matching ROM.
    let needs_digest = |size| {
        only.is_none()
            && editions
                .iter()
                .any(|e| e.roms.iter().any(|r| r.size == size))
    };
    let platform = source(code).platform;
    let rom =
        match cartridge::inspect(conn, id, platform, product_code(code), needs_digest, cancel)? {
            Some(cartridge::Proof::Digest(rom)) => rom,
            Some(cartridge::Proof::Size(size)) => {
                return Ok(only
                    .filter(|e| e.roms.iter().any(|r| r.size == size))
                    .map(|e| e.name.clone()));
            }
            None => return Ok(only.map(|e| e.name.clone())),
        };
    let mut exact = editions.iter().filter(|e| {
        e.roms.iter().any(|r| {
            r.size == rom.size && r.crc == rom.crc && (r.sha1.is_none() || r.sha1 == rom.sha1)
        })
    });
    Ok(exact
        .next()
        .filter(|_| exact.next().is_none())
        .map(|e| e.name.clone()))
}

#[cfg(feature = "ui-tests")]
fn recorded_response(
    response: &http::Response<Vec<u8>>,
    code: &str,
    cancel: &AtomicBool,
) -> Result<SourceResponse, ProviderError> {
    if !response.status().is_success() {
        return Err(ProviderError::http(response.status(), response.headers()));
    }
    Catalog::parse(response.body(), cancel)?.response(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAT_FIXTURE: &[u8] = br#"game (
        name "A & B (USA, Europe)"
        region "USA / Europe"
        serial "AXXE"
        rom ( name "test.gba" serial "DIFF" )
    )
    game (
        name "A & B (Japan)"
        region "Japan"
        serial "AXXJ"
    )
    game (
        name "A & B (Japan) (Rev 1)"
        region "Japan"
        serial "AXXJ"
    )"#;

    #[test]
    fn no_intro_matches_exact_regional_codes_and_keeps_revisions_distinct() {
        let db = Catalog::parse(DAT_FIXTURE, &AtomicBool::default()).unwrap();
        let usa = db.response("AXXE").unwrap();
        let record =
            super::super::matching_serial(&usa.releases, Some("AXXE"), Some("USA"), true).unwrap();
        assert_eq!(record.region, "USA, Europe");
        assert_eq!(record.date, "");
        assert!(db.response("DIFF").unwrap().releases.is_empty());
        let jpn = db.response("AXXJ").unwrap();
        assert_eq!(jpn.releases.len(), 2);
        assert_ne!(jpn.releases[0].id, jpn.releases[1].id);
        assert!(
            super::super::matching_serial(&jpn.releases, Some("AXXJ"), Some("Japan"), true)
                .is_none()
        );
    }

    #[test]
    fn same_code_roms_match_their_own_revision_after_refreshing_a_legacy_cache() {
        use crate::library::{Library, metadata, scrape::Mode};
        use metadata::RefreshOutcome::{Candidates, Updated};
        let temp = tempfile::tempdir().unwrap();
        let lib = Library::open(&temp.path().join("library.db")).unwrap();
        let cancel = AtomicBool::default();
        let mut dat = String::new();
        let mut ids = Vec::new();
        for index in 0..3_u8 {
            let mut bytes = [index; 512];
            bytes[0xac..0xb0].copy_from_slice(b"AXXJ");
            let path = temp.path().join(format!("private-{index}.gba"));
            std::fs::write(&path, bytes).unwrap();
            lib.scan_files(&[path], &cancel, |_| {}).unwrap();
            ids.push(lib.visible_game_ids().unwrap().into_iter().max().unwrap());
            if index < 2 {
                use std::fmt::Write;
                writeln!(dat, "game (\n name \"Release {index}\"\n region \"Japan\"\n serial \"AXXJ\"\n rom ( name \"a.gba\" size 512 crc {:08X} )\n)", crc32fast::hash(&bytes)).unwrap();
            }
        }
        let catalog = Catalog::parse(dat.as_bytes(), &cancel).unwrap();
        let fetch = |code: &str| catalog.response(code).map_err(|e| e.to_string());
        let legacy = metadata::refresh::refresh_with(&lib, ids[0], |key| {
            let mut reply = fetch(key)?;
            reply.source.payload_version = 1;
            reply.raw = br#"["AXXJ",[["Release 0","Japan"],["Release 1","Japan"]]]"#.to_vec();
            Ok(reply)
        })
        .unwrap();
        assert_eq!(legacy, Candidates);

        let batch = metadata::Batch::default();
        metadata::set_override(&lib, ids[0], metadata::Field::Title, Some("My cartridge")).unwrap();
        assert_eq!(
            batch
                .refresh_with(&lib, ids[0], Mode::Incremental, &cancel, fetch)
                .unwrap(),
            Updated
        );
        assert_eq!(
            batch
                .refresh_with(&lib, ids[1], Mode::Incremental, &cancel, |_| panic!(
                    "shared catalog downloaded twice"
                ))
                .unwrap(),
            Updated
        );
        let other_batch = metadata::Batch::default();
        assert_eq!(
            other_batch
                .refresh_with(&lib, ids[2], Mode::Incremental, &cancel, |_| panic!(
                    "should use the offline DAT response"
                ))
                .unwrap(),
            Candidates
        );
        let cards = metadata::list_by_id(&lib, &ids).unwrap();
        assert_eq!(cards[&ids[0]].title.as_deref(), Some("My cartridge"));
        assert_eq!(
            cards[&ids[0]].release_id.as_deref(),
            Some(edition_id("AXXJ", "Release 0").as_str())
        );
        assert_eq!(
            cards[&ids[1]].release_id.as_deref(),
            Some(edition_id("AXXJ", "Release 1").as_str())
        );
        assert!(cards[&ids[2]].release_id.is_none());
        let manual = edition_id("AXXJ", "Release 1");
        metadata::select(&lib, ids[0], &manual).unwrap();
        assert_eq!(
            metadata::refresh::refresh_with(&lib, ids[0], fetch).unwrap(),
            Updated
        );
        assert_eq!(
            metadata::list_by_id(&lib, &ids).unwrap()[&ids[0]]
                .release_id
                .as_deref(),
            Some(manual.as_str())
        );
    }

    #[cfg(feature = "ui-tests")]
    #[test]
    fn mixed_library_jobs_use_gba_catalog_and_art_preserving_manual_data_and_platform() {
        use crate::library::{Library, covers, metadata, scrape};
        let temp = tempfile::tempdir().unwrap();
        let lib = Library::open(&temp.path().join("library.db")).unwrap();
        let mut db = lib.connect().unwrap();
        let tx = db.transaction().unwrap();
        for (id, platform, serial, region) in [
            (1, "GBA", "AXXE", "USA"),
            (2, "GBA", "AXXJ", "Japan"),
            (3, "PS1", "SCUS-94900", "USA"),
        ] {
            tx.execute(
                "INSERT INTO games(id,platform,fallback_title) VALUES(?,?,'Local')",
                rusqlite::params![id, platform],
            )
            .unwrap();
            metadata::identify(&tx, id, Some(serial), Some(region), "Local").unwrap();
        }
        tx.commit().unwrap();
        metadata::set_override(&lib, 1, metadata::Field::Title, Some("Mine")).unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(8, 8)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let lib = lib.with_scrape_responses(
            http::Response::new(DAT_FIXTURE.to_vec()),
            http::Response::new(png.into_inner()),
        );
        let result = scrape::run(
            &lib,
            &[1, 2],
            scrape::Mode::Full,
            covers::Provider::Custom,
            "invalid PS1 template",
            &AtomicBool::default(),
            |_, _| {},
        );
        assert_eq!(
            (result.finished, result.failed),
            (2, 0),
            "{:?}",
            result.errors
        );
        let cards = metadata::list_by_id(&lib, &[1, 2]).unwrap();
        assert_eq!(cards[&1].title.as_deref(), Some("Mine"));
        assert_eq!(
            cards[&1].release_title.as_deref(),
            Some("A & B (USA, Europe) · USA, Europe")
        );
        assert!(cards[&1].thumbnail.as_ref().is_some_and(|p| p.is_file()));
        assert!(
            cards[&1]
                .cover_source
                .as_deref()
                .unwrap()
                .starts_with(COVER_BASE)
        );
        assert!(cards[&2].pending);
        assert!(cards[&2].thumbnail.is_none());
        let choices = metadata::candidates(&lib, 2, "").unwrap();
        assert_eq!(choices.len(), 2);
        assert!(metadata::candidates(&lib, 3, "A & B").unwrap().is_empty());
        assert!(metadata::select(&lib, 3, &choices[0].id).is_err());
        metadata::select(&lib, 2, &choices[0].id).unwrap();
        lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
        assert!(metadata::candidates(&lib, 2, "AXXJ").unwrap().is_empty());
        assert!(
            metadata::list_by_id(&lib, &[1]).unwrap()[&1]
                .thumbnail
                .is_none()
        );
    }
    #[test]
    #[ignore = "requires the locally supplied Japanese Aria of Sorrow cartridge and live Libretro"]
    fn japanese_castlevania_matches_retail_rom_and_downloads_art() {
        use crate::library::{Library, covers, metadata};
        let rom = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/Castlevania - Akatsuki no Minuet (Japan).gba");
        let temp = tempfile::tempdir().unwrap();
        let lib = Library::open(&temp.path().join("library.db")).unwrap();
        lib.scan_files(&[rom], &AtomicBool::default(), |_| {})
            .unwrap();
        let game = lib.games().unwrap().remove(0);
        assert_eq!(game.serial.as_deref(), Some("A2CJ"));
        assert_eq!(
            metadata::refresh_online(&lib, game.id).unwrap(),
            metadata::RefreshOutcome::Updated
        );
        assert_eq!(
            metadata::list_by_id(&lib, &[game.id]).unwrap()[&game.id]
                .title
                .as_deref(),
            Some("Castlevania - Akatsuki no Minuet (Japan)")
        );
        covers::fetch(
            &lib,
            game.id,
            covers::Provider::Libretro,
            "",
            &AtomicBool::default(),
        )
        .unwrap();
        let cards = metadata::list_by_id(&lib, &[game.id]).unwrap();
        assert!(image::open(cards[&game.id].thumbnail.as_ref().unwrap()).is_ok());
        assert!(
            cards[&game.id]
                .cover_source
                .as_deref()
                .unwrap()
                .starts_with(COVER_BASE)
        );
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod regional_tests;
