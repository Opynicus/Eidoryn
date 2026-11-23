//! Regional GBA releases from Libretro's No-Intro DAT. The cartridge code comes
//! from the ROM header; local filenames are never sent to the provider.
use super::{Provenance, ProviderError, Release, SourceResponse, transport};
use crate::library::http_retry::RequestLimit;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const DAT: &str = "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/no-intro/Nintendo%20-%20Game%20Boy%20Advance.dat";
pub(in crate::library) const COVER_BASE: &str = "https://raw.githubusercontent.com/libretro-thumbnails/Nintendo_-_Game_Boy_Advance/master/Named_Boxarts/";
const PREFIX: &str = "libretro-gba/";
const PROVIDER: &str = "Libretro No-Intro (GBA)";
static DOWNLOAD: RequestLimit = RequestLimit::new(1, Duration::ZERO);
struct Cache {
    generation: u64,
    fetched: Option<(Instant, Result<Catalog, ProviderError>)>,
}

static CATALOG: Mutex<Cache> = Mutex::new(Cache {
    generation: 0,
    fetched: None,
});

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CATALOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn is_code(serial: &str) -> bool {
    serial.len() == 4
        && serial
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

pub(in crate::library) fn edition_id(serial: &str, name: &str) -> String {
    format!("{PREFIX}{serial}/{:x}", Sha256::digest(name.as_bytes()))
}

pub(super) fn clear_cache() {
    let mut state = cache();
    state.generation = state.generation.wrapping_add(1);
    state.fetched = None;
}

struct Catalog(BTreeMap<String, Vec<(String, String)>>);

impl Catalog {
    fn parse(bytes: &[u8], cancel: &AtomicBool) -> Result<Self, ProviderError> {
        let dat = std::str::from_utf8(bytes).map_err(|e| ProviderError::Invalid(e.to_string()))?;
        let mut records = BTreeMap::<String, Vec<(String, String)>>::new();
        let mut game = None;
        for line in dat.lines().map(str::trim) {
            if line == "game (" {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProviderError::Cancelled);
                }
                if game.is_some() {
                    return Err(ProviderError::Invalid("Incomplete GBA DAT record".into()));
                }
                game = Some(("", "", ""));
            } else if let Some((name, region, serial)) = &mut game {
                if line == ")" {
                    if !name.is_empty() && is_code(serial) {
                        let rows = records.entry((*serial).to_owned()).or_default();
                        let row = ((*name).to_owned(), region.replace(" / ", ", "));
                        if !rows.contains(&row) {
                            rows.push(row);
                        }
                    }
                    game = None;
                } else if let Some(line) = line.strip_suffix('"') {
                    if let Some(value) = line.strip_prefix("name \"") {
                        *name = value;
                    } else if let Some(value) = line.strip_prefix("region \"") {
                        *region = value;
                    } else if let Some(value) = line.strip_prefix("serial \"") {
                        *serial = value;
                    }
                }
            }
        }
        if records.is_empty() || game.is_some() {
            return Err(ProviderError::Invalid(
                "Invalid or empty Libretro GBA catalog".into(),
            ));
        }
        Ok(Self(records))
    }

    fn response(&self, code: &str) -> Result<SourceResponse, ProviderError> {
        let rows = self.0.get(code).map_or(&[][..], Vec::as_slice);
        let releases = rows
            .iter()
            .map(|(name, region)| Release {
                id: edition_id(code, name),
                serial: code.into(),
                title: name.clone(),
                region: region.clone(),
                date: String::new(),
                work_date: String::new(),
                aliases: String::new(),
            })
            .collect();
        Ok(SourceResponse {
            source: Provenance {
                provider: PROVIDER,
                payload_version: 1,
            },
            releases,
            raw: serde_json::to_vec(&(code, rows))
                .map_err(|e| ProviderError::Invalid(e.to_string()))?,
        })
    }
}

pub(super) fn lookup(code: &str, cancel: &AtomicBool) -> Result<SourceResponse, ProviderError> {
    let _permit = DOWNLOAD.acquire_typed(cancel)?;
    let generation = {
        let state = cache();
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
        let response = transport::request(transport::api()?.get(DAT), 8 * 1024 * 1024, cancel)?;
        if !response.status().is_success() {
            DOWNLOAD.defer(response.headers());
            return Err(ProviderError::http(response.status(), response.headers()));
        }
        Catalog::parse(response.body(), cancel)
    })();
    if matches!(fetched, Err(ProviderError::Cancelled)) {
        return Err(ProviderError::Cancelled);
    }
    let mut state = cache();
    if state.generation != generation {
        return Err(ProviderError::Cancelled);
    }
    let (_, result) = state.fetched.insert((Instant::now(), fetched));
    result.as_ref().map_err(Clone::clone)?.response(code)
}

/// Only a unique catalog entry in the cartridge's region can supply automatic artwork.
pub(in crate::library) fn boxart_name(
    code: &str,
    region: &str,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    if !is_code(code) {
        return Ok(None);
    }
    let response = lookup(code, cancel).map_err(|err| err.to_string())?;
    Ok(super::matching_serial(
        &response.releases,
        Some(code),
        (!region.is_empty()).then_some(region),
    )
    .map(|release| release.title.clone()))
}

#[cfg(feature = "ui-tests")]
pub(super) fn recorded_response(
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
            super::super::matching_serial(&usa.releases, Some("AXXE"), Some("USA")).unwrap();
        assert_eq!(record.region, "USA, Europe");
        assert_eq!(record.date, "");
        assert!(db.response("DIFF").unwrap().releases.is_empty());
        let jpn = db.response("AXXJ").unwrap();
        assert_eq!(jpn.releases.len(), 2);
        assert_ne!(jpn.releases[0].id, jpn.releases[1].id);
        assert!(
            super::super::matching_serial(&jpn.releases, Some("AXXJ"), Some("Japan")).is_none()
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
    fn japanese_castlevania_matches_catalog_and_downloads_selected_art() {
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
            metadata::RefreshOutcome::Candidates
        );
        let editions = metadata::candidates(&lib, game.id, "").unwrap();
        let retail = editions
            .iter()
            .find(|edition| edition.title == "Castlevania - Akatsuki no Minuet (Japan)")
            .unwrap();
        metadata::select(&lib, game.id, &retail.id).unwrap();
        covers::fetch_release(
            &lib,
            game.id,
            covers::Provider::Libretro,
            "",
            retail,
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
