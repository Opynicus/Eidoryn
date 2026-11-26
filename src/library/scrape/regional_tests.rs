//! Live public-provider contract. Only the temporary library contains downloaded artwork.
use super::{Library, Mode, covers, metadata, run};
use std::{path::Path, sync::atomic::AtomicBool};

#[derive(Debug)]
struct Case<'a> {
    platform: &'a str,
    market: &'a str,
    serial: Option<&'a str>,
    territory: Option<&'a str>,
    outcome: &'a str,
    boxart: Option<&'a str>,
    title: &'a str,
}

fn cases() -> Vec<Case<'static>> {
    include_str!("../../../tests/fixtures/regional-scrape.tsv")
        .lines()
        .skip(1)
        .map(|line| {
            let columns: Vec<_> = line.split('\t').collect();
            let [platform, market, serial, territory, outcome, art, title] = columns[..] else {
                panic!("Invalid regional fixture: {line}");
            };
            Case {
                platform,
                market,
                serial: (serial != "-").then_some(serial),
                territory: (territory != "-").then_some(territory),
                outcome,
                boxart: (art != "-").then_some(art),
                title,
            }
        })
        .collect()
}

fn verify(lib: &Library, case: &Case<'_>, id: i64) -> Result<(), String> {
    let card = metadata::list_by_id(lib, &[id])?
        .remove(&id)
        .ok_or("card missing")?;
    if case.outcome == "matched" {
        let expected = case.market.replace(" / ", ", ");
        if card.release_id.is_none()
            || card.region.as_deref() != Some(expected.as_str())
            || card.title.as_deref().is_none_or(str::is_empty)
            || card.release_title.as_deref().is_none_or(str::is_empty)
            || card.pending
            || card.serial.as_deref() != case.serial
        {
            return Err(format!(
                "expected {expected} release, got {:?}, {:?}, pending={}",
                card.release_id, card.region, card.pending
            ));
        }
        if case.platform == "GBA"
            && (card.title.as_deref() != Some(case.title) || card.date.is_some())
        {
            return Err(format!(
                "unexpected GBA title/date: {:?} {:?}",
                card.title, card.date
            ));
        }
    } else if card.release_id.is_some() || card.pending != (case.outcome == "choice") {
        return Err(format!(
            "unexpected assignment {:?}, pending={}",
            card.release_id, card.pending
        ));
    }
    match (case.boxart, &card.thumbnail) {
        (None, None)
            if case.outcome != "matched" || card.status.as_deref() == Some("not_found") =>
        {
            Ok(())
        }
        (Some(expected), Some(thumb)) => {
            let original: String = lib
                .connect()?
                .query_row(
                    "SELECT original FROM cover_assets WHERE game_id=?1 AND kind='download'",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let dimensions = image::image_dimensions(thumb).map_err(|e| e.to_string())?;
            if dimensions.0.min(dimensions.1) == 0 || dimensions.0.max(dimensions.1) > 256 {
                return Err(format!("invalid thumbnail {dimensions:?}"));
            }
            check_blob(&lib.base_dir().join(original), expected)
        }
        _ => Err(format!(
            "expected art {:?}, thumbnail {:?}",
            case.boxart, card.thumbnail
        )),
    }
}

// The independently captured Named_Boxarts Git tree identifies the exact regional image.
fn check_blob(image: &Path, expected: &str) -> Result<(), String> {
    let bytes = std::fs::read(image).map_err(|e| e.to_string())?;
    let mut hash = sha1_smol::Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()).as_bytes());
    hash.update(&bytes);
    if hash.digest().to_string() == expected {
        Ok(())
    } else {
        Err(format!(
            "image differs from upstream blob {expected}: {}",
            image.display()
        ))
    }
}

#[test]
#[ignore = "downloads live regional metadata and front covers from public providers"]
fn live_regional_scrape_matrix() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("regional.sqlite3");
    let lib = Library::open(&path).unwrap();
    let cases = cases();
    let ids: Vec<i64> = (1..=i64::try_from(cases.len()).unwrap()).collect();
    let mut db = lib.connect().unwrap();
    let tx = db.transaction().unwrap();
    for (case, id) in cases.iter().zip(&ids) {
        tx.execute(
            "INSERT INTO games(id,platform,fallback_title) VALUES(?,?,'Local game')",
            rusqlite::params![id, case.platform],
        )
        .unwrap();
        metadata::identify(&tx, *id, case.serial, case.territory, "Local game").unwrap();
    }
    tx.commit().unwrap();
    let mut failures = Vec::new();
    for mode in [Mode::Incremental, Mode::Full] {
        let report = run(
            &lib,
            &ids,
            mode,
            covers::Provider::Libretro,
            "",
            &AtomicBool::default(),
            |_, _| {},
        );
        eprintln!(
            "{mode:?}: {} finished, {} awaiting a choice, {} failed, {} deferred",
            report.finished, report.needs_choice, report.failed, report.retry_waiting
        );
        if report.finished != ids.len() || report.failed != 0 || report.retry_waiting != 0 {
            failures.push(format!("{mode:?}: {:?}", report.errors));
        }
        let choices = cases.iter().filter(|case| case.outcome == "choice").count();
        if report.needs_choice != choices {
            failures.push(format!(
                "{mode:?}: expected {choices} awaiting a choice, got {}",
                report.needs_choice
            ));
        }
        let reopened = Library::open(&path).unwrap();
        for (case, id) in cases.iter().zip(&ids) {
            if let Err(error) = verify(&reopened, case, *id) {
                let failure = format!(
                    "{mode:?} {} {} {:?} {}: {error}",
                    case.platform, case.market, case.serial, case.title
                );
                eprintln!("{failure}");
                failures.push(failure);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    verify_manual_selections(&lib, &path, &cases, &ids);
}

fn verify_manual_selections(lib: &Library, path: &Path, cases: &[Case<'_>], ids: &[i64]) {
    let selections = [
        (
            "SLPS-00004",
            "gamedb/SLPS-00004".to_owned(),
            "Japan",
            Some("075a0ecef10c9ef5a1cd92733f250d3c95af0a93"),
        ),
        (
            "SLPS-00004",
            "gamedb/SLPS-00004-R1".to_owned(),
            "Japan",
            Some("2794d51188cf8c21d0c203c518b208da10bcce4f"),
        ),
        (
            "AAMJ",
            metadata::no_intro::edition_id("AAMJ", "Akumajou Dracula - Circle of the Moon (Japan)"),
            "Japan",
            Some("732d111caa26177416b3a857639d25aa687deaab"),
        ),
        (
            "AAMJ",
            metadata::no_intro::edition_id("AAMJ", "Akumajou Dracula - Circle of the Moon (China)"),
            "China",
            None,
        ),
    ];
    for (serial, release, market, boxart) in selections {
        let (case, &id) = cases
            .iter()
            .zip(ids)
            .find(|(c, _)| c.serial == Some(serial))
            .unwrap();
        let edition = metadata::candidates(lib, id, "")
            .unwrap()
            .into_iter()
            .find(|r| r.id == release)
            .unwrap();
        metadata::select(lib, id, &edition.id).unwrap();
        run_successfully(lib, &[id], Mode::Full);
        let expected = Case {
            outcome: "matched",
            title: &edition.title,
            market,
            boxart,
            ..*case
        };
        verify(&Library::open(path).unwrap(), &expected, id).unwrap();
    }
}

#[test]
#[ignore = "set EIDORYN_CASTLEVANIA_MEDIA to the local media directory; downloads public covers"]
fn live_castlevania_import_and_edition_selection() {
    let media = std::env::var_os("EIDORYN_CASTLEVANIA_MEDIA").expect("EIDORYN_CASTLEVANIA_MEDIA");
    let samples = [
        (
            "PS1",
            "Castlevania - Symphony of the Night (Asia).chd",
            "SCPS-45196",
            "Asia",
            "cb7731a641b8c40527e4149e3b7812839e48babc",
        ),
        (
            "PS1",
            "Castlevania - Symphony of the Night (USA).chd",
            "SLUS-00067",
            "USA",
            "d08bea8298eb2bf049f8bb5f070cce1fd996b596",
        ),
        (
            "GBA",
            "Castlevania - Akatsuki no Minuet (Japan).gba",
            "A2CJ",
            "Japan",
            "86da8c50e8f62af94d7541b407e9b1d64a745a76",
        ),
        (
            "GBA",
            "Castlevania - Aria of Sorrow (USA).gba",
            "A2CE",
            "USA",
            "6f901c3cf5dec923311b9aa4ad19ef0646afe681",
        ),
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite3");
    let lib = Library::open(&path).unwrap();
    let cancel = AtomicBool::default();
    let files = samples.map(|(_, name, ..)| Path::new(&media).join(name));
    let scan = lib.scan_files(&files, &cancel, |_| {}).unwrap();
    assert_eq!((scan.added, scan.failed), (4, 0), "{:?}", scan.errors);
    let ids = lib.visible_games_for_scraping().unwrap();
    run_successfully(&lib, &ids, Mode::Incremental);
    let reopened = Library::open(&path).unwrap();
    let games = reopened.games().unwrap();
    for (platform, name, serial, market, image) in samples {
        let game = games
            .iter()
            .find(|g| g.serial.as_deref() == Some(serial))
            .unwrap();
        assert_eq!(game.platform, platform);
        let title = Path::new(name).file_stem().unwrap().to_str().unwrap();
        let case = Case {
            platform,
            market,
            serial: Some(serial),
            territory: None,
            outcome: "matched",
            boxart: Some(image),
            title,
        };
        verify(&reopened, &case, game.id).unwrap();
    }
    // The Virtual Console edition shares the USA cartridge code and has a linked front cover.
    let game = games
        .iter()
        .find(|g| g.serial.as_deref() == Some("A2CE"))
        .unwrap();
    let edition = metadata::candidates(&reopened, game.id, "")
        .unwrap()
        .into_iter()
        .find(|e| e.title.ends_with("(Virtual Console)"))
        .unwrap();
    metadata::select(&reopened, game.id, &edition.id).unwrap();
    run_successfully(&reopened, &[game.id], Mode::Full);
    let case = Case {
        platform: "GBA",
        market: "USA",
        serial: Some("A2CE"),
        territory: None,
        outcome: "matched",
        boxart: Some("6f901c3cf5dec923311b9aa4ad19ef0646afe681"),
        title: &edition.title,
    };
    verify(&Library::open(&path).unwrap(), &case, game.id).unwrap();
}

fn run_successfully(lib: &Library, ids: &[i64], mode: Mode) {
    let result = run(
        lib,
        ids,
        mode,
        covers::Provider::Libretro,
        "",
        &AtomicBool::default(),
        |_, _| {},
    );
    assert_eq!(
        (result.finished, result.failed),
        (ids.len(), 0),
        "{:?}",
        result.errors
    );
}
