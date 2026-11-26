//! End-to-end cartridge evidence, including reused product codes and unnumbered dumps.
use super::*;
use crate::library::{covers, metadata, scrape};
use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

fn cartridge(code: [u8; 4], fill: u8) -> [u8; 512] {
    let mut bytes = [fill; 512];
    bytes[0xac..0xb0].copy_from_slice(&code);
    bytes
}

fn record(dat: &mut String, name: &str, region: &str, serial: Option<&str>, bytes: &[u8]) {
    writeln!(dat, "game (\nname \"{name}\"\nregion \"{region}\"").unwrap();
    if let Some(code) = serial {
        writeln!(dat, "serial \"{code}\"").unwrap();
    }
    writeln!(
        dat,
        "rom ( name \"catalog.gba\" size {} crc {:08X} sha1 {} )\n)",
        bytes.len(),
        crc32fast::hash(bytes),
        sha1_smol::Sha1::from(bytes).digest()
    )
    .unwrap();
}

fn recorded_library(path: &Path, dat: String) -> Library {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(8, 6)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    Library::open(path).unwrap().with_scrape_responses(
        http::Response::new(dat.into_bytes()),
        http::Response::new(png.into_inner()),
    )
}

fn id_for(lib: &Library, path: &Path) -> i64 {
    let path = path.canonicalize().unwrap();
    lib.games()
        .unwrap()
        .into_iter()
        .find(|g| g.locations.iter().any(|l| l.path == path))
        .unwrap_or_else(|| panic!("missing scanned cartridge {}", path.display()))
        .id
}

#[test]
fn a_known_code_with_the_wrong_size_requires_confirmation() {
    let tmp = tempfile::tempdir().unwrap();
    let retail = cartridge(*b"BTAE", 12);
    let mut changed = retail.to_vec();
    changed.extend_from_slice(&[0; 128]);
    let path = tmp.path().join("changed.gba");
    std::fs::write(&path, changed).unwrap();
    let mut dat = String::new();
    record(&mut dat, "Retail game (USA)", "USA", Some("BTAE"), &retail);
    let lib = recorded_library(&tmp.path().join("db.sqlite3"), dat);
    let cancel = AtomicBool::default();
    lib.scan_files(std::slice::from_ref(&path), &cancel, |_| {})
        .unwrap();
    let id = id_for(&lib, &path);
    let result = scrape::run(
        &lib,
        &[id],
        scrape::Mode::Incremental,
        covers::Provider::Libretro,
        "",
        &cancel,
        |_, _| {},
    );
    assert_eq!(
        (result.finished, result.needs_choice, result.failed),
        (1, 1, 0)
    );
    let card = metadata::list_by_id(&lib, &[id])
        .unwrap()
        .remove(&id)
        .unwrap();
    assert!(card.pending && card.release_id.is_none() && card.thumbnail.is_none());
}

#[test]
fn every_no_intro_market_resolves_from_scanned_cartridge_to_edition_and_cover() {
    // The DAT region and the header hint are independent. Several real market groups
    // share suffixes, and some unlicensed games provide no valid product code at all.
    let cases: &[(&[u8; 4], Option<&str>, &str)] = &[
        (b"T00U", Some("Australia"), "Australia"),
        (b"T01U", Some("Australia"), "Australia / Greece"),
        (b"AAMJ", Some("Japan"), "China"),
        (b"AAMJ", Some("Japan"), "Japan"),
        (b"T02C", Some("China"), "China"),
        (b"T03Q", Some("Denmark"), "Denmark"),
        (b"T04P", Some("Europe"), "Europe"),
        (b"T05X", Some("Europe"), "Europe"),
        (b"T06Y", Some("Europe"), "Italy"),
        (b"T07Z", Some("Europe"), "Spain"),
        (b"T08F", Some("France"), "France"),
        (b"T09D", Some("Germany"), "Germany"),
        (b"T10I", Some("Italy"), "Italy"),
        (b"T11A", None, "Japan / USA"),
        (b"T12K", Some("Korea"), "Korea"),
        (b"T13H", Some("Netherlands"), "Netherlands"),
        (b"    ", None, "Russia"),
        (b"T14S", Some("Spain"), "Spain"),
        (b"XXXX", Some("Europe"), "Taiwan"),
        (b"T15E", Some("USA"), "USA"),
        (b"T16E", Some("USA"), "USA / Australia"),
        (b"T17E", Some("USA"), "USA / Europe"),
        (b"T18E", Some("USA"), "World"),
        (b"0000", None, ""),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("library.db");
    let cancel = AtomicBool::default();
    let mut dat = String::new();
    let mut paths = Vec::<PathBuf>::new();
    for (index, (code, _, market)) in cases.iter().enumerate() {
        let bytes = cartridge(**code, u8::try_from(index).unwrap());
        let path = tmp.path().join(format!("private-{index}.gba"));
        std::fs::write(&path, bytes).unwrap();
        let serial =
            (!matches!(*market, "Russia" | "Taiwan")).then(|| std::str::from_utf8(*code).unwrap());
        record(
            &mut dat,
            &format!("Edition {index}"),
            market,
            serial,
            &bytes,
        );
        paths.push(path);
    }
    let lib = recorded_library(&db, dat);
    lib.scan_files(&paths, &cancel, |_| {}).unwrap();
    let ids: Vec<_> = paths.iter().map(|p| id_for(&lib, p)).collect();
    for ((_, hint, _), id) in cases.iter().zip(&ids) {
        assert_eq!(lib.games_by_id(&[*id]).unwrap()[0].region.as_deref(), *hint);
    }
    for mode in [scrape::Mode::Incremental, scrape::Mode::Full] {
        let result = scrape::run(
            &lib,
            &ids,
            mode,
            covers::Provider::Libretro,
            "",
            &cancel,
            |_, _| {},
        );
        assert_eq!(
            (result.finished, result.failed),
            (cases.len(), 0),
            "{result:?}"
        );
        let cards = metadata::list_by_id(&Library::open(&db).unwrap(), &ids).unwrap();
        for (index, ((code, _, market), id)) in cases.iter().zip(&ids).enumerate() {
            let card = &cards[id];
            assert_eq!(card.title, Some(format!("Edition {index}")), "{market}");
            assert_eq!(
                card.region.as_deref(),
                Some(market.replace(" / ", ", ").as_str()),
                "{market}"
            );
            assert!(!card.pending && card.date.is_none());
            assert!(image::open(card.thumbnail.as_ref().unwrap()).is_ok());
            assert_eq!(
                card.cover_source,
                Some(format!("{COVER_BASE}Edition%20{index}.png"))
            );
            let serial = std::str::from_utf8(*code).unwrap();
            assert_eq!(card.serial.as_deref(), is_code(serial).then_some(serial));
        }
    }
}

#[test]
fn digest_only_records_require_an_unambiguous_size_and_sha1_even_with_the_same_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::default();
    let db = tmp.path().join("library.db");
    let bytes = cartridge(*b"    ", 42);
    let mut dat = String::new();
    record(&mut dat, "Unknown", "Russia", None, &bytes);
    record(&mut dat, "Duplicate", "Taiwan", None, &bytes);
    let path = tmp.path().join("Unknown.gba");
    std::fs::write(&path, bytes).unwrap();
    let lib = recorded_library(&db, dat);
    lib.scan_files(std::slice::from_ref(&path), &cancel, |_| {})
        .unwrap();
    let id = id_for(&lib, &path);
    assert_eq!(
        metadata::refresh_online(&lib, id).unwrap(),
        metadata::RefreshOutcome::Candidates
    );
    assert_eq!(
        covers::fetch(&lib, id, covers::Provider::Libretro, "", &cancel).unwrap(),
        "skipped"
    );
    assert_eq!(metadata::candidates(&lib, id, "").unwrap().len(), 2);
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    let mut changed = bytes;
    changed[0] ^= 1;
    let mut only = String::new();
    record(&mut only, "Unknown", "Russia", None, &changed);
    let lib = recorded_library(&db, only);
    assert_eq!(
        metadata::refresh_online(&lib, id).unwrap(),
        metadata::RefreshOutcome::NoSerial
    );
    assert!(
        metadata::list_by_id(&lib, &[id]).unwrap()[&id]
            .release_id
            .is_none()
    );
    let mut only = String::new();
    record(&mut only, "Correct", "Russia", None, &bytes);
    let lib = recorded_library(&db, only);
    assert_eq!(
        covers::fetch(&lib, id, covers::Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    assert_eq!(
        metadata::list_by_id(&lib, &[id]).unwrap()[&id]
            .release_title
            .as_deref(),
        Some("Correct · Russia")
    );
    let offline = Library::open(&db).unwrap().with_scrape_responses(
        http::Response::builder()
            .status(503)
            .body(Vec::new())
            .unwrap(),
        http::Response::builder()
            .status(503)
            .body(Vec::new())
            .unwrap(),
    );
    let batch = metadata::Batch::default();
    let prepared = batch
        .prepare(&offline, id, scrape::Mode::Incremental, &cancel)
        .unwrap();
    assert_eq!(
        metadata::refresh::commit_one(&offline, id, prepared).unwrap(),
        metadata::RefreshOutcome::Updated
    );
    lib.clear_metadata_cache(metadata::CacheScope::All).unwrap();
    let db = lib.connect().unwrap();
    for table in ["metadata_responses", "fetched_releases", "cover_assets"] {
        let count: i64 = db
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE serial GLOB 'gba-rom:*'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert_eq!(
        covers::fetch_missing(&lib, id, covers::Provider::Libretro, "", &cancel).unwrap(),
        "skipped"
    );
    assert!(
        metadata::list_by_id(&lib, &[id]).unwrap()[&id]
            .thumbnail
            .is_none()
    );
}

#[test]
fn an_unnumbered_dump_with_a_known_header_but_different_size_uses_its_own_edition() {
    let tmp = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::default();
    let db = tmp.path().join("library.db");
    let licensed = cartridge(*b"BORJ", 3);
    let mut unnumbered = licensed.to_vec();
    unnumbered.resize(768, 42);
    let mut dat = String::new();
    record(&mut dat, "Licensed", "Japan", Some("BORJ"), &licensed);
    record(&mut dat, "Other game", "Taiwan", None, &unnumbered);
    let path = tmp.path().join("renamed.gba");
    std::fs::write(&path, &unnumbered).unwrap();
    let lib = recorded_library(&db, dat);
    lib.scan_files(std::slice::from_ref(&path), &cancel, |_| {})
        .unwrap();
    let id = id_for(&lib, &path);
    assert_eq!(
        covers::fetch_missing(&lib, id, covers::Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    let card = metadata::list_by_id(&lib, &[id])
        .unwrap()
        .remove(&id)
        .unwrap();
    assert_eq!(card.region.as_deref(), Some("Taiwan"));
    assert_eq!(card.serial.as_deref(), Some("BORJ"));
    assert_eq!(card.title.as_deref(), Some("Other game"));
}

#[test]
fn a_manual_choice_between_identical_digests_controls_the_cover_and_survives_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::default();
    let db = tmp.path().join("library.db");
    let bytes = cartridge(*b"    ", 27);
    let mut dat = String::new();
    record(&mut dat, "English", "World", None, &bytes);
    record(&mut dat, "Taiwanese", "Taiwan", None, &bytes);
    let path = tmp.path().join("English.gba");
    std::fs::write(&path, bytes).unwrap();
    let lib = recorded_library(&db, dat);
    lib.scan_files(std::slice::from_ref(&path), &cancel, |_| {})
        .unwrap();
    let id = id_for(&lib, &path);
    assert_eq!(
        metadata::refresh_online(&lib, id).unwrap(),
        metadata::RefreshOutcome::Candidates
    );
    let chosen = metadata::candidates(&lib, id, "")
        .unwrap()
        .into_iter()
        .find(|r| r.region == "Taiwan")
        .unwrap();
    assert_eq!(chosen.product_code(), None);
    metadata::select(&lib, id, &chosen.id).unwrap();
    assert_eq!(
        metadata::refresh_online(&lib, id).unwrap(),
        metadata::RefreshOutcome::Updated
    );
    assert_eq!(
        covers::fetch(&lib, id, covers::Provider::Libretro, "", &cancel).unwrap(),
        "ready"
    );
    let card = metadata::list_by_id(&lib, &[id])
        .unwrap()
        .remove(&id)
        .unwrap();
    assert_eq!(
        card.cover_source,
        Some(format!("{COVER_BASE}Taiwanese.png"))
    );
    assert_eq!(card.release_id, Some(chosen.id));
    assert_eq!(card.serial, None);
}

/// Audit an independent projection of every top-level game block in the upstream DAT.
/// The file is supplied at runtime so ordinary tests stay offline and the repository
/// contains neither a copy of the source catalog nor its artwork.
#[test]
#[ignore = "set EIDORYN_GBA_DAT to a local Libretro No-Intro GBA catalog"]
fn whole_catalog_retains_each_edition_and_its_verified_lookup_keys() {
    check_whole_catalog("EIDORYN_GBA_DAT", "", "gba-rom:");
}

#[test]
#[ignore = "set EIDORYN_NDS_DAT to a local Libretro No-Intro NDS catalog"]
fn whole_nds_catalog_retains_each_edition_and_its_verified_lookup_keys() {
    check_whole_catalog("EIDORYN_NDS_DAT", "NDS:", "nds-rom:");
}

fn check_whole_catalog(variable: &str, prefix: &str, digest_prefix: &str) {
    use std::collections::BTreeSet;
    let path = std::env::var_os(variable).expect("supply a No-Intro database");
    let text = std::fs::read_to_string(path).unwrap();
    let catalog = Catalog::parse(text.as_bytes(), &AtomicBool::default()).unwrap();
    let mut expected = BTreeMap::<String, BTreeSet<(String, String)>>::new();
    let mut markets = BTreeSet::new();
    let mut records = 0;
    let mut numbered = 0;
    for block in text.split("\ngame (").skip(1) {
        let fields: Vec<_> = block
            .lines()
            .map(str::trim)
            .take_while(|&l| l != ")")
            .collect();
        let value = |tag: &str| {
            fields
                .iter()
                .find_map(|s| s.strip_prefix(tag)?.strip_suffix('"'))
        };
        let title = value("name \"").unwrap();
        let region = value("region \"").unwrap_or("").replace(" / ", ", ");
        markets.insert(region.clone());
        records += 1;
        // Extract independent expectations directly from the source payload, including
        // nonstandard product codes and every ROM within an unnumbered catalog record.
        let keys = if let Some(code) = value("serial \"").filter(|c| {
            c.len() == 4
                && c.chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        }) {
            numbered += 1;
            vec![format!("{prefix}{code}")]
        } else {
            fields
                .iter()
                .filter(|s| s.starts_with("rom ("))
                .map(|line| {
                    let attrs: Vec<_> = line.split_ascii_whitespace().collect();
                    let attr =
                        |name: &str| attrs.windows(2).rfind(|pair| pair[0] == name).unwrap()[1];
                    let size = attr("size").parse::<u64>().unwrap();
                    let sha1 = attr("sha1").to_ascii_lowercase();
                    assert!(
                        sha1.len() == 40 && sha1.bytes().all(|b| b.is_ascii_hexdigit()),
                        "{title}"
                    );
                    format!("{digest_prefix}{size}:{sha1}")
                })
                .collect()
        };
        assert!(
            !keys.is_empty(),
            "{title} has neither product code nor ROM signature"
        );
        for key in keys {
            expected
                .entry(key)
                .or_default()
                .insert((title.to_owned(), region.clone()));
        }
    }
    assert!(records > 0 && numbered < records);
    assert_eq!(catalog.0.len(), expected.len());
    let mut reused = 0;
    let mut digest_collisions = 0;
    for (key, editions) in expected {
        if editions.len() > 1 {
            if key.starts_with(digest_prefix) {
                digest_collisions += 1;
            } else {
                reused += 1;
            }
        }
        let response = catalog.response(&key).unwrap();
        let mut actual = BTreeSet::new();
        for release in response.releases {
            assert_eq!(release.serial, key);
            assert_eq!(release.id, edition_id(&key, &release.title));
            assert_eq!(release.date, "");
            assert!(actual.insert((release.title, release.region)));
        }
        assert_eq!(actual, editions, "lookup key {key}");
    }
    eprintln!(
        "{records} editions, {} market groups, {numbered} valid product codes (counting editions), {} digest-only editions, {reused} reused codes, {digest_collisions} ambiguous digest keys",
        markets.len(),
        records - numbered
    );
}

#[test]
fn nds_and_gba_share_a_library_without_sharing_catalogs_for_the_same_code() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("library.db");
    let cancel = AtomicBool::default();
    let mut ds = vec![0; 0x404];
    ds[..8].copy_from_slice(b"DS TITLE");
    ds[0x0c..0x10].copy_from_slice(b"BJBE");
    // Independent, valid ARM9 and ARM7 direct-boot descriptors.
    for (offset, file, address) in [
        (0x20, 0x200_u32, 0x0200_0000_u32),
        (0x30, 0x400, 0x0380_0000),
    ] {
        for (field, word) in [file, address, address, 4].into_iter().enumerate() {
            let start = offset + field * 4;
            ds[start..start + 4].copy_from_slice(&word.to_le_bytes());
        }
        ds[file as usize..file as usize + 4].copy_from_slice(&0xeaff_fffe_u32.to_le_bytes());
    }
    let gba = cartridge(*b"BJBE", 1);
    let mut games = Vec::new();
    for (file, bytes, platform, title, base) in [
        (
            "renamed.NDS",
            ds.as_slice(),
            GamePlatform::Nds,
            "007 - Blood Stone (USA)",
            NDS_COVER_BASE,
        ),
        (
            "renamed.gba",
            gba.as_slice(),
            GamePlatform::Gba,
            "007 - Everything or Nothing (USA)",
            COVER_BASE,
        ),
    ] {
        let path = tmp.path().join(file);
        std::fs::write(&path, bytes).unwrap();
        let mut dat = String::new();
        record(&mut dat, title, "USA", Some("BJBE"), bytes);
        let lib = recorded_library(&db, dat);
        let report = lib
            .scan_files(std::slice::from_ref(&path), &cancel, |_| {})
            .unwrap();
        assert_eq!(report.added, 1);
        let id = id_for(&lib, &path);
        let imported = lib.games_by_id(&[id]).unwrap().pop().unwrap();
        assert_eq!(imported.platform_kind(), Some(platform));
        assert_eq!(imported.serial.as_deref(), Some("BJBE"));
        let result = scrape::run(
            &lib,
            &[id],
            scrape::Mode::Full,
            covers::Provider::Igdb,
            "",
            &cancel,
            |_, _| {},
        );
        assert_eq!(
            (result.finished, result.failed, result.needs_choice),
            (1, 0, 0),
            "{result:?}"
        );
        let card = &metadata::list_by_id(&lib, &[id]).unwrap()[&id];
        assert_eq!(card.title.as_deref(), Some(title));
        assert_eq!(card.serial.as_deref(), Some("BJBE"));
        assert!(card.cover_source.as_deref().unwrap().starts_with(base));
        assert!(image::open(card.thumbnail.as_ref().unwrap()).is_ok());
        games.push((id, card.release_id.clone().unwrap()));
    }
    let lib = Library::open(&db).unwrap();
    for (index, (id, edition)) in games.iter().enumerate() {
        let candidates = metadata::candidates(&lib, *id, "BJBE").unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, *edition);
        assert_eq!(candidates[0].product_code(), Some("BJBE"));
        assert!(metadata::select(&lib, *id, &games[1 - index].1).is_err());
    }
    assert!(
        metadata::candidates(&lib, games[0].0, "NDS")
            .unwrap()
            .is_empty()
    );
    lib.remove_games_and_clear_metadata(&[games[0].0]).unwrap();
    let card = &metadata::list_by_id(&lib, &[games[1].0]).unwrap()[&games[1].0];
    assert_eq!(card.release_id.as_ref(), Some(&games[1].1));
    assert!(image::open(card.thumbnail.as_ref().unwrap()).is_ok());
    assert_eq!(
        metadata::candidates(&lib, games[0].0, "BJBE")
            .unwrap()
            .len(),
        0
    );
    ds[0x20..0x24].copy_from_slice(&0xffff_ffff_u32.to_le_bytes());
    let invalid = tmp.path().join("broken.nds");
    std::fs::write(&invalid, ds).unwrap();
    assert!(GamePlatform::Nds.probe(&invalid).is_err());
}
