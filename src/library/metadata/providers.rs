//! Resolve editions by disc serial. Work publication dates never fill edition dates.
use super::{
    MetadataProvider, Provenance, ProviderError, SourceResponse, gamedb, matching::region_matches,
    wikidata,
};
use serde_json::value::RawValue;
use std::sync::atomic::AtomicBool;

pub(super) fn is_release_serial(serial: &str) -> bool {
    wikidata::is_supported_serial(serial) || gamedb::is_supported_serial(serial)
}

pub(super) fn lookup_release(
    serial: &str,
    cancel: &AtomicBool,
) -> Result<SourceResponse, ProviderError> {
    if !wikidata::is_supported_serial(serial) {
        return gamedb::GameDb.search(serial, cancel);
    }
    let primary = wikidata::Wikidata.search(serial, cancel);
    resolve(primary, || gamedb::GameDb.search(serial, cancel))
}

fn resolve(
    primary: Result<SourceResponse, ProviderError>,
    secondary: impl FnOnce() -> Result<SourceResponse, ProviderError>,
) -> Result<SourceResponse, ProviderError> {
    let mut primary = match primary {
        Ok(response) => response,
        Err(ProviderError::Cancelled) => return Err(ProviderError::Cancelled),
        Err(error) => {
            return match secondary() {
                Ok(fallback) if !fallback.releases.is_empty() => Ok(fallback),
                Err(ProviderError::Cancelled) => Err(ProviderError::Cancelled),
                Err(other) if other.retryable() && !error.retryable() => Err(other),
                _ => Err(error),
            };
        }
    };
    if primary.releases.is_empty() {
        return secondary();
    }
    let [release] = primary.releases.as_mut_slice() else {
        return Ok(primary); // Let the user choose between conflicting editions.
    };
    if !release.date.is_empty() {
        return Ok(primary);
    }

    let fallback = match secondary() {
        Ok(response) => response,
        Err(ProviderError::Cancelled) => return Err(ProviderError::Cancelled),
        Err(_) => return Ok(primary), // The first source still identifies the edition.
    };
    let [extra] = fallback.releases.as_slice() else {
        // One work can cover multiple regional revisions of the same disc.
        if fallback.releases.len() > 1
            && fallback
                .releases
                .iter()
                .any(|r| r.serial == release.serial && region_matches(&release.region, &r.region))
        {
            return Ok(fallback);
        }
        return Ok(primary);
    };
    if extra.serial == release.serial
        && !extra.date.is_empty()
        && release.region != extra.region
        && region_matches(&release.region, &extra.region)
    {
        // A country-specific date should be displayed with its original market.
        return Ok(fallback);
    }
    if extra.serial == release.serial && extra.region == release.region && !extra.date.is_empty() {
        // Keep the Wikidata ID so a previously confirmed edition can gain a date.
        primary.raw = combined_evidence(&primary.raw, &fallback.raw)
            .map_err(|err| ProviderError::Invalid(err.to_string()))?;
        primary.source = Provenance {
            provider: "Wikidata + GameDB-PSX",
            payload_version: 1,
        };
        release.date.clone_from(&extra.date);
    }
    Ok(primary)
}

fn combined_evidence(wikidata: &[u8], gamedb_psx: &[u8]) -> serde_json::Result<Vec<u8>> {
    #[derive(serde::Serialize)]
    struct Sources<'a> {
        wikidata: &'a RawValue,
        gamedb_psx: &'a RawValue,
    }
    serde_json::to_vec(&Sources {
        wikidata: serde_json::from_slice(wikidata)?,
        gamedb_psx: serde_json::from_slice(gamedb_psx)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::Release;
    use super::*;

    const SERIAL: &str = "SLUS-12345";

    fn response(source: Provenance, editions: &[(&str, &str, &str)]) -> SourceResponse {
        SourceResponse {
            source,
            releases: editions
                .iter()
                .map(|&(id, region, date)| Release {
                    id: id.into(),
                    serial: SERIAL.into(),
                    region: region.into(),
                    date: date.into(),
                    title: "Example".into(),
                    aliases: String::new(),
                    work_date: String::from("1994"),
                })
                .collect(),
            raw: serde_json::to_vec(&serde_json::json!({"source":source.provider})).unwrap(),
        }
    }

    #[test]
    fn complete_and_ambiguous_primary_editions_do_not_query_a_second_catalog() {
        for editions in [
            vec![("Q1/disc", "USA", "1996")],
            vec![("Q1/rev1", "USA", ""), ("Q1/rev2", "USA", "")],
        ] {
            let primary = response(wikidata::Wikidata::SOURCE, &editions);
            let result = resolve(Ok(primary), || panic!("secondary lookup")).unwrap();
            assert_eq!(result.releases.len(), editions.len());
            assert_eq!(result.source.provider, wikidata::Wikidata::SOURCE.provider);
        }
    }

    #[test]
    fn fills_only_the_exact_edition_date_and_retains_both_sources() {
        let primary = || response(wikidata::Wikidata::SOURCE, &[("Q1/disc", "USA", "")]);
        let fallback = |region| {
            response(
                gamedb::GameDb::SOURCE,
                &[("gamedb/disc", region, "1996-08")],
            )
        };
        let merged = resolve(Ok(primary()), || Ok(fallback("USA"))).unwrap();
        let release = &merged.releases[0];
        assert_eq!(release.id, "Q1/disc");
        assert_eq!(release.date, "1996-08");
        assert_eq!(release.work_date, "1994");
        let raw: serde_json::Value = serde_json::from_slice(&merged.raw).unwrap();
        assert_eq!(
            raw["wikidata"]["source"],
            wikidata::Wikidata::SOURCE.provider
        );
        assert_eq!(raw["gamedb_psx"]["source"], gamedb::GameDb::SOURCE.provider);
        let different_region = resolve(Ok(primary()), || Ok(fallback("Europe"))).unwrap();
        assert_eq!(different_region.releases[0].date, "");
        let different_serial = resolve(Ok(primary()), || {
            let mut other = fallback("USA");
            other.releases[0].serial = "SLUS-12346".into();
            Ok(other)
        })
        .unwrap();
        assert_eq!(different_serial.releases[0].date, "");

        let revisions = resolve(Ok(primary()), || {
            Ok(response(
                gamedb::GameDb::SOURCE,
                &[
                    ("gamedb/original", "USA", "1996"),
                    ("gamedb/reissue", "USA", "1998"),
                ],
            ))
        })
        .unwrap();
        assert_eq!(revisions.releases.len(), 2);
        assert_eq!(revisions.releases[1].id, "gamedb/reissue");
        let european = || response(wikidata::Wikidata::SOURCE, &[("Q1/disc", "Europe", "")]);
        let french = resolve(Ok(european()), || Ok(fallback("France"))).unwrap();
        assert_eq!(french.releases[0].region, "France");
        assert_eq!(french.releases[0].date, "1996-08");
        let countries = resolve(Ok(european()), || {
            Ok(response(
                gamedb::GameDb::SOURCE,
                &[
                    ("gamedb/fr", "France", "1996"),
                    ("gamedb/de", "Germany", "1997"),
                ],
            ))
        })
        .unwrap();
        assert_eq!(countries.releases.len(), 2);
    }

    #[test]
    fn falls_back_on_empty_results_or_an_outage_and_propagates_cancellation() {
        let fallback = || {
            Ok(response(
                gamedb::GameDb::SOURCE,
                &[("gamedb/disc", "USA", "1996")],
            ))
        };
        for primary in [
            Ok(response(wikidata::Wikidata::SOURCE, &[])),
            Err(ProviderError::Timeout),
        ] {
            let result = resolve(primary, fallback).unwrap();
            assert_eq!(result.source.provider, gamedb::GameDb::SOURCE.provider);
        }
        assert!(matches!(
            resolve(Err(ProviderError::Cancelled), || panic!("secondary lookup")),
            Err(ProviderError::Cancelled)
        ));
        assert!(matches!(
            resolve(Err(ProviderError::Timeout), || Ok(response(
                gamedb::GameDb::SOURCE,
                &[]
            ))),
            Err(ProviderError::Timeout)
        ));
        assert!(matches!(
            resolve(
                Ok(response(
                    wikidata::Wikidata::SOURCE,
                    &[("Q1/disc", "USA", "")]
                )),
                || Err(ProviderError::Cancelled)
            ),
            Err(ProviderError::Cancelled)
        ));
    }
}
