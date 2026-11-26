//! Libretro's thumbnails use playlist names. Resolve those names from the upstream
//! Redump DAT, indexed by serial in the user's library on the first cover request.
use std::sync::{Mutex, atomic::AtomicBool};
use std::time::SystemTime;

use crate::library::transport;
use reqwest::{Client, Url};
use rusqlite::{Connection, params};

use super::{Library, Lookup, MAX_BYTES, RETRY_SECONDS, check_cancel, seconds};

mod thumbnail;
pub(super) use thumbnail::fetch_image;

const DAT: &str = "https://raw.githubusercontent.com/libretro/libretro-database/master/metadat/redump/Sony%20-%20PlayStation.dat";
pub(super) const BOXART: &str = "https://raw.githubusercontent.com/libretro-thumbnails/Sony_-_PlayStation/master/Named_Boxarts/";
const REFRESH_SECONDS: i64 = 30 * 86_400;
static REFRESH: Mutex<()> = Mutex::new(());

pub(super) fn resolve(
    lib: &Library,
    client: &Client,
    serial: &str,
    region: &str,
    release_name: Option<&str>,
    cancel: &AtomicBool,
) -> Result<Lookup, String> {
    let mut db = lib.connect()?;
    let now = seconds(SystemTime::now());
    // Serialize cold starts. Other downloads can use the index while the image is in flight.
    let guard = REFRESH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (last, retry): (Option<i64>, i64) = db
        .query_row(
            "SELECT sync_time,retry_after FROM libretro_catalog WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    check_cancel(cancel)?;
    if retry > now {
        return match cached(&db, serial, region, release_name)? {
            Lookup::Missing => {
                let mut retry_response = http::Response::new(Vec::<u8>::new());
                *retry_response.status_mut() = reqwest::StatusCode::SERVICE_UNAVAILABLE;
                retry_response
                    .headers_mut()
                    .insert(reqwest::header::RETRY_AFTER, (retry - now).into());
                Ok(Lookup::Http(retry_response))
            }
            hit => Ok(hit),
        };
    }
    if last.is_none_or(|last| now.saturating_sub(last) >= REFRESH_SECONDS) {
        let refresh = (|| {
            let response = transport::request(client.get(DAT), MAX_BYTES, cancel)
                .map_err(|e| e.to_string())?;
            if crate::library::http_retry::should_defer(response.status(), response.headers()) {
                return Ok(Some(response)); // The common download path persists Retry-After for the host.
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Libretro catalog returned HTTP {}",
                    response.status().as_u16()
                ));
            }
            let bytes = response.into_body();
            let dat = std::str::from_utf8(&bytes).map_err(|_| "Invalid Libretro DAT encoding")?;
            replace_index(&mut db, dat, now, cancel)?;
            Ok(None)
        })();
        match refresh {
            Ok(Some(limited)) => return Ok(Lookup::Http(limited)),
            Ok(None) => {}
            Err(error) => {
                check_cancel(cancel)?;
                db.execute(
                    "UPDATE libretro_catalog SET retry_after=?1 WHERE id=1",
                    [now + RETRY_SECONDS],
                )
                .map_err(|e| e.to_string())?;
                return cached_after_failure(&db, serial, region, release_name, error);
            }
        }
    }
    drop(guard);
    cached(&db, serial, region, release_name)
}

// A stale positive match still identifies the asset. A stale miss is inconclusive.
fn cached_after_failure(
    db: &Connection,
    serial: &str,
    region: &str,
    release_name: Option<&str>,
    error: String,
) -> Result<Lookup, String> {
    match cached(db, serial, region, release_name)? {
        Lookup::Missing => Err(error),
        hit => Ok(hit),
    }
}

fn field<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix('"')
}

fn replace_index(
    db: &mut Connection,
    dat: &str,
    now: i64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let tx = db.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM libretro_boxart_names", [])
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    let mut in_game = false;
    let mut name = "";
    let mut serial = "";
    let mut region = "";
    {
        let mut insert = tx
            .prepare(
                "INSERT OR IGNORE INTO libretro_boxart_names(serial,region,name) VALUES(?1,?2,?3)",
            )
            .map_err(|e| e.to_string())?;
        for line in dat.lines().map(str::trim) {
            if line == "game (" {
                if in_game {
                    return Err("Incomplete Libretro DAT entry".into());
                }
                check_cancel(cancel)?;
                in_game = true;
                (name, serial, region) = ("", "", "");
            } else if in_game {
                if line == ")" {
                    if !name.is_empty() && !serial.is_empty() {
                        count += insert
                            .execute(params![serial, region, name])
                            .map_err(|e| e.to_string())?;
                    }
                    in_game = false;
                } else if let Some(value) = field(line, "name \"") {
                    name = value;
                } else if let Some(value) = field(line, "serial \"") {
                    serial = value;
                } else if let Some(value) = field(line, "region \"") {
                    region = value;
                }
            }
        }
    }
    if count == 0 || in_game {
        return Err("Empty or incomplete Libretro PS1 catalog".into());
    }
    check_cancel(cancel)?;
    tx.execute(
        "UPDATE libretro_catalog SET sync_time=?1,retry_after=0 WHERE id=1",
        [now],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

fn matches_region(dat: &str, region: &str) -> bool {
    let markets = || dat.split([',', '/']).map(str::trim);
    region.is_empty()
        || markets().any(|market| crate::library::metadata::license_matches_market(region, market))
        || region
            .split([',', '/'])
            .map(str::trim)
            .all(|selected| markets().any(|market| market == selected))
}

fn cached(
    db: &Connection,
    serial: &str,
    region: &str,
    release_name: Option<&str>,
) -> Result<Lookup, String> {
    let exact = cached_match(
        db,
        "SELECT name,region FROM libretro_boxart_names WHERE serial=?1",
        serial,
        region,
        release_name,
    )?;
    if !matches!(exact, Lookup::Missing) || !crate::library::metadata::is_release_serial(serial) {
        return Ok(exact);
    }
    // Redump may list only printed part numbers (e.g. SLES-04107/GER,
    // SCES-02029-P). SYSTEM.CNF supplies the shorter boot code.
    cached_match(
        db,
        "SELECT name,region FROM libretro_boxart_names WHERE serial GLOB ?1",
        &format!("{serial}?*"),
        region,
        release_name,
    )
}

fn cached_match(
    db: &Connection,
    query: &str,
    key: &str,
    region: &str,
    release_name: Option<&str>,
) -> Result<Lookup, String> {
    let mut stmt = db.prepare(query).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([key]).map_err(|e| e.to_string())?;
    let mut selected = None;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let territory: String = row.get(1).map_err(|e| e.to_string())?;
        let name: String = row.get(0).map_err(|e| e.to_string())?;
        if release_name.is_some_and(|chosen| chosen != name) {
            continue;
        }
        // Several Redump entries have no region field but explicitly name the
        // market in the catalog. This also works with previously indexed rows.
        let markets = if territory.is_empty() {
            crate::library::metadata::named_markets(&name).unwrap_or("")
        } else {
            &territory
        };
        if !matches_region(markets, region) {
            continue;
        }
        if selected.as_ref().is_some_and(|previous| previous != &name) {
            return Ok(Lookup::NeedsChoice);
        }
        selected = Some(name);
    }
    let Some(name) = selected else {
        return Ok(Lookup::Missing);
    };
    Ok(Lookup::Image(image_url(BOXART, &name)?))
}

pub(super) fn image_url(base: &str, name: &str) -> Result<Url, String> {
    let filename = name.replace(
        ['&', '*', '/', ':', '`', '<', '>', '?', '\\', '|', '"'],
        "_",
    );
    let mut url = Url::parse(base).map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|()| "Invalid Libretro thumbnail endpoint")?
        .pop_if_empty()
        .push(&format!("{filename}.png"));
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_licenses_and_selected_markets_match_redump_editions() {
        let (_dir, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        // Redump's market fields include joint territories. UK and Scandinavian
        // discs frequently have a market tag in the playlist name only.
        let markets: &[(&str, &[&str])] = &[
            ("NTSC-U", &["USA", "Canada", "USA / Canada", "USA / Europe"]),
            (
                "NTSC-J",
                &[
                    "Japan",
                    "Asia",
                    "Japan / Asia",
                    "Korea",
                    "Taiwan",
                    "China",
                    "Singapore",
                ],
            ),
            (
                "PAL",
                &[
                    "Europe",
                    "Australia",
                    "Europe / Australia",
                    "Israel",
                    "Austria",
                    "Belgium",
                    "Belgium / Netherlands",
                    "Denmark",
                    "Finland",
                    "France",
                    "Germany",
                    "Greece",
                    "Ireland",
                    "Italy",
                    "Netherlands",
                    "Norway",
                    "Poland",
                    "Portugal",
                    "Russia",
                    "Scandinavia",
                    "Spain",
                    "Sweden",
                    "UK",
                    "USA / Europe",
                ],
            ),
        ];
        for &(license, territories) in markets {
            for &territory in territories {
                let region = if matches!(territory, "UK" | "Scandinavia") {
                    String::new()
                } else {
                    format!("region \"{territory}\"")
                };
                let market = territory.replace(" / ", ", ");
                let name = format!("Game (Demo) ({market}) (Rev 1)");
                let dat = format!("game (\nname \"{name}\"\n{region}\nserial \"SLES-01234\"\n)\n");
                replace_index(&mut db, &dat, 1, &AtomicBool::default()).unwrap();
                for filter in [license, &market] {
                    let Lookup::Image(url) = cached(&db, "SLES-01234", filter, None).unwrap()
                    else {
                        panic!("missing art for {territory} with {filter}");
                    };
                    assert_eq!(url, image_url(BOXART, &name).unwrap());
                }
                for other in ["NTSC-J", "NTSC-U", "PAL"] {
                    if other != license && !(territory == "USA / Europe" && other != "NTSC-J") {
                        assert!(
                            matches!(
                                cached(&db, "SLES-01234", other, None).unwrap(),
                                Lookup::Missing
                            ),
                            "{other} unexpectedly matched {territory}"
                        );
                    }
                }
            }
        }
        // The program can request an explicit market without assuming its TV
        // system. The sole current Singapore catalog entry lacks a disc serial.
        assert!(matches_region("Singapore", "Singapore"));
        assert!(!matches_region("Australia", "Europe"));
        assert!(!matches_region("Canada", "USA"));
        assert!(!matches_region("USA", "USA, Canada"));
        assert!(matches_region("USA / Canada", "Canada"));
    }

    #[test]
    fn printed_serials_can_supply_one_regional_match_for_a_boot_code() {
        let (_dir, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        let name = "All Star Action (Europe) (Disc 1)";
        let entry = |serial: &str, title: &str, market: &str| {
            format!("game (\nname \"{title}\"\nserial \"{serial}\"\nregion \"{market}\"\n)")
        };
        let suffixes = [
            entry("SLES-04107/GER", name, "Europe"),
            entry("SLES-04107/GER-0", name, "Europe"),
            entry("SLES-04107-F", "French edition", "France"),
        ]
        .join("\n");
        replace_index(&mut db, &suffixes, 1, &AtomicBool::default()).unwrap();
        let select = |db: &Connection, market| cached(db, "SLES-04107", market, None);
        let Lookup::Image(image) = select(&db, "Europe").unwrap() else {
            panic!("missing boot code with two printed serials for the same disc")
        };
        assert_eq!(image, image_url(BOXART, name).unwrap());
        assert!(matches!(select(&db, "PAL").unwrap(), Lookup::NeedsChoice));
        assert!(matches!(select(&db, "Canada").unwrap(), Lookup::Missing));
        let exact = entry("SLES-04107", "Known pressing", "Europe");
        replace_index(
            &mut db,
            &format!("{exact}\n{suffixes}"),
            2,
            &AtomicBool::default(),
        )
        .unwrap();
        let Lookup::Image(image) = select(&db, "Europe").unwrap() else {
            panic!("missing exact serial")
        };
        assert_eq!(image, image_url(BOXART, "Known pressing").unwrap());
    }

    #[test]
    #[ignore = "set EIDORYN_REDUMP_PSX to a downloaded Sony - PlayStation.dat"]
    fn all_serials_in_redump_preserve_their_markets_and_ambiguities() {
        use std::collections::{BTreeMap, BTreeSet};
        let data =
            std::fs::read_to_string(std::env::var_os("EIDORYN_REDUMP_PSX").unwrap()).unwrap();
        let (_dir, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        replace_index(&mut db, &data, 1, &AtomicBool::default()).unwrap();
        // An independent pass over game-level fields also checks the index
        // retained every serial (including alternative part numbers for art).
        let mut groups = BTreeMap::<(&str, &str), BTreeSet<&str>>::new();
        let mut no_serial = 0;
        for block in data.split("\ngame (\n").skip(1) {
            let field = |prefix: &str| {
                block
                    .lines()
                    .map(str::trim)
                    .find_map(|line| line.strip_prefix(prefix)?.strip_suffix('"'))
            };
            let Some(serial) = field("serial \"") else {
                no_serial += 1;
                continue;
            };
            let name = field("name \"").unwrap();
            let dat = field("region \"")
                .unwrap_or_else(|| crate::library::metadata::named_markets(name).unwrap_or(""));
            for market in dat
                .split([',', '/'])
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                groups.entry((serial, market)).or_default().insert(name);
            }
        }
        let mut printed = BTreeMap::<(&str, &str), BTreeSet<&str>>::new();
        for (&(serial, market), names) in &groups {
            let boot_length = if serial.starts_with("DTL-S") { 9 } else { 10 };
            if let Some(boot) = serial.get(..boot_length)
                && boot != serial
                && crate::library::metadata::is_release_serial(boot)
                && !groups.contains_key(&(boot, market))
            {
                printed.entry((boot, market)).or_default().extend(names);
            }
        }
        let printed_pairs = printed.len();
        groups.append(&mut printed);
        let mut unique = 0;
        let mut ambiguous = 0;
        for ((serial, market), names) in &groups {
            let result = cached(&db, serial, market, None);
            if names.len() == 1 {
                let Lookup::Image(url) = result.unwrap() else {
                    panic!("{serial}/{market}: missing")
                };
                assert_eq!(url, image_url(BOXART, names.first().unwrap()).unwrap());
                unique += 1;
            } else {
                assert!(
                    matches!(result.unwrap(), Lookup::NeedsChoice),
                    "{serial}/{market}"
                );
                ambiguous += 1;
            }
        }
        println!(
            "Redump: {unique} unique serial/market pairs, {ambiguous} ambiguous pairs, {printed_pairs} additional boot code/market pairs from printed serials, {no_serial} entries without serial"
        );
        assert!(unique > 1_000);
    }

    #[test]
    fn ntsc_j_disc_art_uses_the_matching_serials_market_and_preserves_ambiguity() {
        let (_dir, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        let catalog = r#"game (
            name "Castlevania - Symphony of the Night (Asia)"
            serial "SCPS-45196"
            region "Asia"
        )"#;
        let serial = "SCPS-45196";
        replace_index(&mut db, catalog, 1, &AtomicBool::default()).unwrap();
        let Lookup::Image(url) = cached(&db, serial, "NTSC-J", None).unwrap() else {
            panic!("the licensed disc has one serial-matched Asian release");
        };
        assert!(
            url.path()
                .ends_with("/Castlevania%20-%20Symphony%20of%20the%20Night%20(Asia).png")
        );
        assert!(matches!(
            cached(&db, serial, "Japan", None).unwrap(),
            Lookup::Missing
        ));
        assert!(matches!(
            cached(&db, serial, "USA", None).unwrap(),
            Lookup::Missing
        ));

        let multiple = format!("{catalog}\n{}", catalog.replace("Asia", "Japan"));
        replace_index(&mut db, &multiple, 2, &AtomicBool::default()).unwrap();
        assert!(matches!(
            cached(&db, serial, "NTSC-J", None).unwrap(),
            Lookup::NeedsChoice
        ));
        let Lookup::Image(url) = cached(&db, serial, "Asia", None).unwrap() else {
            panic!("the selected market identifies the artwork");
        };
        assert!(url.path().ends_with("(Asia).png"));
    }

    #[test]
    fn indexes_only_disc_fields_and_never_guesses_between_editions() {
        let (_tmp, lib) = super::super::tests::fixture();
        let mut db = lib.connect().unwrap();
        let cancel = AtomicBool::new(false);
        let one = r#"game (
            name "A & B: Demo (France)"
            region "France"
            serial "TEST-12345"
            rom ( name "unrelated" serial "TEST-99999" )
        )"#;
        replace_index(&mut db, one, 1, &cancel).unwrap();
        let Lookup::Image(url) = cached(&db, "TEST-12345", "PAL", None).unwrap() else {
            panic!("missing regional art")
        };
        assert!(url.path().ends_with("/A%20_%20B_%20Demo%20(France).png"));
        assert!(matches!(
            cached(&db, "TEST-12345", "USA", None).unwrap(),
            Lookup::Missing
        ));
        assert!(matches!(
            cached(&db, "TEST-99999", "", None).unwrap(),
            Lookup::Missing
        ));
        assert!(replace_index(&mut db, &format!("{one}\ngame ("), 2, &cancel).is_err());
        assert!(matches!(
            cached(&db, "TEST-12345", "PAL", None).unwrap(),
            Lookup::Image(_)
        ));
        db.execute(
            "UPDATE libretro_catalog SET retry_after=? WHERE id=1",
            [i64::MAX],
        )
        .unwrap();
        let client = transport::api().unwrap();
        assert!(matches!(
            resolve(&lib, client, "TEST-12345", "PAL", None, &cancel).unwrap(),
            Lookup::Image(_)
        ));
        assert!(
            matches!(resolve(&lib, client, "TEST-00000", "Europe", None, &cancel).unwrap(), Lookup::Http(response) if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE)
        );
        replace_index(
            &mut db,
            &format!("{one}\n{}", one.replace("Demo", "Demo (Rev 1)")),
            3,
            &cancel,
        )
        .unwrap();
        assert!(matches!(
            cached(&db, "TEST-12345", "PAL", None).unwrap(),
            Lookup::NeedsChoice
        ));
    }
}
