//! Older libraries associated the downloaded front cover with just the product code.
use super::{Remote, RemoteKind, libretro, markets::MARKETS};
use crate::library::metadata::no_intro;
use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

pub(super) fn detach_mismatched(
    db: &Connection,
    id: i64,
    art: Remote<'_>,
    expected: &Url,
) -> rusqlite::Result<()> {
    let Some(release) = art.release else {
        return Ok(());
    };
    if matches!(art.kind, RemoteKind::Chosen { .. })
        || !(expected.as_str().starts_with(no_intro::COVER_BASE)
            || expected.as_str().starts_with(libretro::BOXART))
    {
        return Ok(());
    }
    let Some(stored) = db.prepare_cached(
        "SELECT source FROM cover_assets WHERE game_id=? AND kind='download' AND serial=? AND source<>?",
    )?.query_row(params![id, art.serial, expected.as_str()], |r| r.get::<_,String>(0)).optional()? else {
        return Ok(());
    };
    let wrong = if release.starts_with("libretro-gba/") {
        other_cartridge_image(db, art.serial, release, &stored)?
    } else {
        other_disc_image(&stored, expected)
    };
    if wrong {
        let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
        let deleted = tx.execute(
            "DELETE FROM cover_assets WHERE game_id=?1 AND kind='download' AND source=?2
             AND EXISTS(SELECT 1 FROM game_metadata WHERE game_id=?1 AND release_id=?3 AND cache_generation=?4)",
            params![id, stored, release, art.generation],
        )?;
        if deleted != 0 {
            tx.execute(
                "DELETE FROM cover_attempts WHERE game_id=? AND url=?",
                params![id, stored],
            )?;
        }
        tx.commit()?;
    }
    Ok(())
}

fn other_cartridge_image(
    db: &Connection,
    serial: &str,
    release: &str,
    stored: &str,
) -> rusqlite::Result<bool> {
    if !stored.starts_with(no_intro::COVER_BASE) {
        return Ok(false);
    }
    // Require an exact match to a known sibling. Alternate encodings and third-party
    // URLs do not establish a conflicting edition.
    let mut editions = db.prepare_cached(
        "SELECT title FROM fetched_releases
        WHERE serial=? AND id<>? AND id GLOB 'libretro-gba/*'",
    )?;
    let mut titles = editions.query(params![serial, release])?;
    while let Some(row) = titles.next()? {
        if libretro::image_url(no_intro::COVER_BASE, &row.get::<_, String>(0)?)
            .is_ok_and(|u| u.as_str() == stored)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn other_disc_image(stored: &str, expected: &Url) -> bool {
    let Some((base, _)) = stored.split_once('?') else {
        return false;
    };
    if base != libretro::BOXART {
        return false;
    }
    let Ok(prior) = Url::parse(stored) else {
        return false;
    };
    let field = |url: &Url, key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    if field(&prior, "serial") != field(expected, "serial") {
        return false;
    }
    if let (Some(old), Some(new)) = (field(&prior, "edition"), field(expected, "edition"))
        && old != new
    {
        return true;
    }
    let (Some(old), Some(new)) = (field(&prior, "region"), field(expected, "region")) else {
        return false;
    };
    let recognized = |region: &str| {
        region.split([',', '/']).map(str::trim).all(|name| {
            MARKETS
                .iter()
                .any(|market| market.name.eq_ignore_ascii_case(name))
        })
    };
    recognized(&old)
        && recognized(&new)
        && !MARKETS
            .iter()
            .any(|market| market.requested(&old) && market.requested(&new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicitly_conflicting_disc_markets_or_editions_detach_an_image() {
        let url = |query: &str| Url::parse(&format!("{}?{query}", libretro::BOXART)).unwrap();
        let expected = url("serial=SLPS-00004&region=Japan%2C+Asia&edition=Disc+Rev+1");
        for (prior, conflict) in [
            ("serial=SLPS-00004", false),
            ("serial=SLPS-00004&region=NTSC-J", false),
            (
                "region=Japan+%2F+Asia&serial=SLPS-00004&edition=Disc%20Rev%201",
                false,
            ),
            ("serial=SLPS-00004&region=Japan", false),
            ("serial=SLPS-00004&region=Canada", true),
            ("serial=SLPS-00004&region=Japan&edition=Disc", true),
        ] {
            assert_eq!(other_disc_image(url(prior).as_str(), &expected), conflict);
        }
        assert!(!other_disc_image(
            "https://custom.example/other.png",
            &expected
        ));
    }
}
