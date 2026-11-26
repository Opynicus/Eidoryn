//! IGDB game localizations associate each localized cover with a region.
//! Its covers have no platform field, so a shared multi-platform entry cannot
//! establish that the image is the PlayStation box.

use std::{sync::atomic::AtomicBool, time::Duration};

use crate::library::transport;
use reqwest::{Client, RequestBuilder, Url};
use serde::Deserialize;

use super::{
    Lookup,
    markets::{MARKETS, Market},
};
use crate::library::http_retry::RequestLimit;

// The IGDB API allows four requests per second and eight simultaneous connections.
static REQUESTS: RequestLimit = RequestLimit::new(8, Duration::from_millis(250));

const GAMES: &str = "https://api.igdb.com/v4/games";
const MAX_RESULTS: usize = 50;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Deserialize)]
struct Game {
    name: String,
    #[serde(default)]
    platforms: Vec<Platform>,
    #[serde(default)]
    alternative_names: Vec<Name>,
    #[serde(default, rename = "game_localizations")]
    localizations: Vec<Localization>,
}

#[derive(Deserialize)]
struct Platform {
    slug: String,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Localization {
    #[serde(default)]
    name: String,
    region: Option<Region>,
    cover: Option<Cover>,
}

#[derive(Deserialize)]
struct Region {
    #[serde(default)]
    name: String,
    #[serde(default)]
    identifier: String,
}

#[derive(Deserialize)]
struct Cover {
    image_id: String,
}

pub(super) fn resolve(
    client: &Client,
    title: &str,
    region: &str,
    cancel: &AtomicBool,
) -> Result<Lookup, String> {
    let title = title.trim();
    let Some(query) = query(title) else {
        return Ok(Lookup::Missing);
    };
    let request = lookup_request(client, query, |key| {
        std::env::var(key).ok().filter(|v| !v.trim().is_empty())
    })?;
    let _permit = REQUESTS.acquire(cancel)?;
    let response =
        transport::request(request, MAX_BYTES, cancel).map_err(|error| error.to_string())?;
    if crate::library::http_retry::should_defer(response.status(), response.headers()) {
        REQUESTS.defer(response.headers());
    }
    lookup(response, title, region)
}

fn lookup_request(
    client: &Client,
    query: String,
    credential: impl Fn(&str) -> Option<String>,
) -> Result<RequestBuilder, &'static str> {
    let missing = "Set EIDORYN_IGDB_CLIENT_ID and EIDORYN_IGDB_ACCESS_TOKEN for IGDB";
    Ok(client
        .post(GAMES)
        .header(
            "Client-ID",
            credential("EIDORYN_IGDB_CLIENT_ID").ok_or(missing)?,
        )
        .bearer_auth(credential("EIDORYN_IGDB_ACCESS_TOKEN").ok_or(missing)?)
        .header("Accept", "application/json")
        .body(query))
}

fn query(title: &str) -> Option<String> {
    if title.is_empty() || title.len() > 512 || title.chars().any(char::is_control) {
        return None;
    }
    let escaped = title.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!(
        "search \"{escaped}\"; where platforms.slug = \"ps\"; limit {MAX_RESULTS}; \
        fields name,platforms.slug,alternative_names.name,game_localizations.name,\
        game_localizations.cover.image_id,game_localizations.region.identifier,\
        game_localizations.region.name;"
    ))
}

fn lookup(response: http::Response<Vec<u8>>, title: &str, region: &str) -> Result<Lookup, String> {
    if !response.status().is_success() {
        return Ok(Lookup::Http(response));
    }
    let bytes = response.body();
    if bytes.len() > MAX_BYTES {
        return Err("IGDB response exceeds the 512 KiB limit".into());
    }
    let games: Vec<Game> =
        serde_json::from_slice(bytes).map_err(|_| "Invalid IGDB game response")?;
    Ok(match regional_cover(&games, title, region) {
        Some(image) => Lookup::Image(
            Url::parse(&format!(
                "https://images.igdb.com/igdb/image/upload/t_cover_big_2x/{image}.jpg"
            ))
            .map_err(|e| e.to_string())?,
        ),
        None => Lookup::Missing,
    })
}

fn regional_cover<'a>(games: &'a [Game], title: &str, region: &str) -> Option<&'a str> {
    if games.len() >= MAX_RESULTS {
        return None; // A truncated search cannot establish uniqueness.
    }
    let title = title.trim().to_lowercase();
    let exact = |name: &str| name.trim().to_lowercase() == title;
    let mut matches = games.iter().filter(|g| {
        g.platforms.iter().any(|p| p.slug == "ps")
            && (exact(&g.name)
                || g.alternative_names.iter().any(|n| exact(&n.name))
                || g.localizations.iter().any(|l| exact(&l.name)))
    });
    let game = matches.next()?;
    if matches.next().is_some() || game.platforms.len() != 1 {
        return None;
    }
    let images_for = |continental| {
        game.localizations.iter().filter_map(move |l| {
            let market = l.region.as_ref()?;
            let eligible = if continental {
                market.north_america()
            } else {
                market.matches(region)
            };
            eligible.then_some(l.cover.as_ref()?.image_id.as_str())
        })
    };
    let mut images = images_for(false).peekable();
    // Retain the existing US fallback only when IGDB has no US-specific front.
    if images.peek().is_none() && region == "USA" {
        images = images_for(true).peekable();
    }
    let image = images.next()?;
    (!image.is_empty()
        && image.len() <= 128
        && image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && images.all(|other| other == image))
    .then_some(image)
}

impl Region {
    fn north_america(&self) -> bool {
        let (name, id) = (self.name.trim(), self.identifier.trim());
        (is_north_america(name) && (id.is_empty() || is_north_america(id)))
            || (name.is_empty() && is_north_america(id))
    }

    fn matches(&self, release: &str) -> bool {
        let name = named_market(&self.name);
        let id = self.identifier.trim();
        let identified = named_market(id).or_else(|| {
            // A lowercase locale such as "fr" names a language. It cannot distinguish
            // France from Canada. The legacy US/JP/EU identifiers are region aliases.
            let territory = locale_territory(id).or_else(|| {
                (id.bytes().all(|b| b.is_ascii_uppercase()) || matches!(id, "us" | "jp" | "eu"))
                    .then_some(id)
            })?;
            MARKETS
                .iter()
                .find(|m| m.identifier.eq_ignore_ascii_case(territory))
        });
        match (name, identified) {
            (Some(a), Some(b)) if a.name != b.name => false,
            (Some(market), _) | (_, Some(market)) => {
                let continental = is_north_america(self.name.trim()) || is_north_america(id);
                market.requested(release)
                    && (!continental || matches!(market.name, "USA" | "Canada"))
            }
            _ => false,
        }
    }
}

fn is_north_america(label: &str) -> bool {
    label.eq_ignore_ascii_case("north-america") || label.eq_ignore_ascii_case("north america")
}

fn named_market(name: &str) -> Option<&'static Market> {
    let name = name.trim();
    let canonical = match name.to_ascii_lowercase().as_str() {
        "united states" | "united states of america" => "USA",
        "united kingdom" | "great britain" => "UK",
        "south korea" | "republic of korea" => "Korea",
        "worldwide" => "World",
        _ => name,
    };
    MARKETS
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(canonical))
}

// For locale identifiers the territory follows the language and optional script,
// e.g. fr-CA or zh-Hant-TW. Language alone does not identify a different country.
fn locale_territory(locale: &str) -> Option<&str> {
    let mut parts = locale.split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let next = parts.next()?;
    let territory = if next.len() == 4 && next.bytes().all(|b| b.is_ascii_alphabetic()) {
        parts.next()?
    } else {
        next
    };
    (territory.len() == 2 && parts.next().is_none()).then_some(territory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn game() -> Value {
        json!({"name":"Test Adventure", "platforms":[{"slug":"ps"}],
        "cover":{"image_id":"generic"},
        "alternative_names":[{"name":"テストアドベンチャー"}],
        "game_localizations":[
            {"region":{"identifier":"ja-JP"},"cover":{"image_id":"cojp"}},
            {"region":{"name":"North America"},"cover":{"image_id":"cous"}}
        ]})
    }

    fn result(games: &Value, title: &str, region: &str) -> Option<String> {
        let response = http::Response::new(serde_json::to_vec(games).unwrap());
        match lookup(response, title, region).unwrap() {
            Lookup::Image(url) => Some(url.to_string()),
            Lookup::NeedsChoice | Lookup::Missing => None,
            Lookup::Http(_) => panic!("unexpected HTTP error"),
        }
    }

    #[test]
    fn resolves_only_exact_playstation_localization_art() {
        let games = json!([game(), {"name":"Test Adventure","platforms":[{"slug":"ps2"}]}]);
        assert_eq!(
            result(&games, "テストアドベンチャー", "Japan").as_deref(),
            Some("https://images.igdb.com/igdb/image/upload/t_cover_big_2x/cojp.jpg")
        );
        assert!(
            result(&games, "test adventure", "USA")
                .unwrap()
                .ends_with("/cous.jpg")
        );
        assert!(result(&games, "Adventure", "USA").is_none());
        assert!(result(&games, "Test Adventure", "Europe").is_none());
    }

    #[test]
    fn selects_each_release_market_from_a_mixed_localization_response() {
        let markets = [
            ("Asia", "Asia"),
            ("Australia", "en-AU"),
            ("Austria", "de-AT"),
            ("Belgium", "fr-BE"),
            ("Canada", "fr_CA"),
            ("China", "zh-Hans-CN"),
            ("Denmark", "da-DK"),
            ("Europe", "EU"),
            ("Finland", "fi-FI"),
            ("France", "fr-FR"),
            ("Germany", "de-DE"),
            ("Greece", "el-GR"),
            ("Ireland", "en-IE"),
            ("Israel", "he-IL"),
            ("Italy", "it-IT"),
            ("Japan", "ja-JP"),
            ("Korea", "ko-KR"),
            ("Netherlands", "nl-NL"),
            ("Norway", "nb-NO"),
            ("Poland", "pl-PL"),
            ("Portugal", "pt-PT"),
            ("Russia", "ru-RU"),
            ("Scandinavia", "Scandinavia"),
            ("Singapore", "en-SG"),
            ("Spain", "es-ES"),
            ("Sweden", "sv-SE"),
            ("Taiwan", "zh-Hant-TW"),
            ("UK", "en-GB"),
            ("USA", "en-US"),
            ("World", "Worldwide"),
        ];
        let mut localized = game();
        localized["game_localizations"] = json!(
            markets
                .iter()
                .map(|(market, id)| {
                    json!({"region":{"identifier":id},"cover":{"image_id":market}})
                })
                .collect::<Vec<_>>()
        );
        let games = json!([localized]);
        for (market, _) in markets {
            let cover =
                format!("https://images.igdb.com/igdb/image/upload/t_cover_big_2x/{market}.jpg");
            assert_eq!(
                result(&games, "Test Adventure", market),
                Some(cover),
                "{market}"
            );
        }
        for ambiguous in [
            "Belgium / Netherlands",
            "Japan, Asia",
            "USA / Europe",
            "Unknown",
            "",
        ] {
            assert!(
                result(&games, "Test Adventure", ambiguous).is_none(),
                "{ambiguous}"
            );
        }
    }

    #[test]
    fn inconsistent_regions_and_language_only_names_do_not_override_country_evidence() {
        let mut localized = game();
        localized["game_localizations"] = json!([
            {"region":{"name":"Canada","identifier":"en-US"},"cover":{"image_id":"conflict"}},
            {"region":{"name":"French"},"cover":{"image_id":"language"}},
            {"region":{"identifier":"fr","name":"French"},"cover":{"image_id":"language2"}},
            {"region":{"identifier":"fr-FR","name":"North America"},"cover":{"image_id":"inconsistent"}},
            {"region":{"name":"United Kingdom"},"cover":{"image_id":"uk"}},
            {"region":{"name":"South Korea"},"cover":{"image_id":"kr"}}
        ]);
        let games = json!([localized]);
        for market in ["USA", "Canada", "France"] {
            assert!(
                result(&games, "Test Adventure", market).is_none(),
                "{market}"
            );
        }
        assert!(
            result(&games, "Test Adventure", "UK")
                .unwrap()
                .ends_with("/uk.jpg")
        );
        assert!(
            result(&games, "Test Adventure", "Japan / Korea")
                .unwrap()
                .ends_with("/kr.jpg")
        );
    }

    #[test]
    fn country_specific_art_precedes_the_legacy_north_american_fallback() {
        let mut localized = game();
        let response = json!([localized]);
        assert!(result(&response, "Test Adventure", "Canada").is_none());
        localized["game_localizations"].as_array_mut().unwrap().extend([
            json!({"region":{"name":"North America","identifier":"en-US"},"cover":{"image_id":"specific_us"}}),
            json!({"region":{"name":"North America","identifier":"fr-CA"},"cover":{"image_id":"specific_ca"}}),
        ]);
        let response = json!([localized]);
        for (market, file) in [("USA", "/specific_us.jpg"), ("Canada", "/specific_ca.jpg")] {
            assert!(
                result(&response, "Test Adventure", market)
                    .unwrap()
                    .ends_with(file)
            );
        }
    }

    #[test]
    fn ambiguous_title_platform_or_art_cannot_supply_a_cover() {
        assert!(result(&json!([game(), game()]), "Test Adventure", "USA").is_none());
        let mut mixed = game();
        mixed["platforms"] = json!([{"slug":"ps"}, {"slug":"pc"}]);
        assert!(result(&json!([mixed]), "Test Adventure", "USA").is_none());
        let mut collision = game();
        collision["game_localizations"][0]["region"] = json!({"identifier":"US"});
        collision["game_localizations"][1]["region"] = json!({"identifier":"en-US"});
        assert!(result(&json!([collision]), "Test Adventure", "USA").is_none());
        let mut invalid = game();
        invalid["game_localizations"][0]["cover"]["image_id"] = json!("../screenshot.jpg");
        assert!(result(&json!([invalid]), "Test Adventure", "Japan").is_none());
        assert!(result(&json!(vec![game(); MAX_RESULTS]), "Test Adventure", "USA").is_none());
    }

    #[test]
    fn preserves_http_status_and_limits_upstream_data() {
        let client = Client::new();
        for missing in ["EIDORYN_IGDB_CLIENT_ID", "EIDORYN_IGDB_ACCESS_TOKEN"] {
            let request = lookup_request(&client, query("Game").unwrap(), |key| {
                (key != missing).then(|| "test-value".to_owned())
            });
            assert_eq!(
                request.err(),
                Some("Set EIDORYN_IGDB_CLIENT_ID and EIDORYN_IGDB_ACCESS_TOKEN for IGDB")
            );
        }
        for status in [401, 403] {
            let denied = http::Response::builder()
                .status(status)
                .body(b"private response".to_vec())
                .unwrap();
            assert!(
                matches!(lookup(denied, "Game", "USA"), Ok(Lookup::Http(response)) if response.status() == status)
            );
        }
        let retry = http::Response::builder()
            .status(429)
            .header("Retry-After", "30")
            .body(Vec::<u8>::new())
            .unwrap();
        let Lookup::Http(response) = lookup(retry, "Test", "USA").unwrap() else {
            panic!("expected HTTP response")
        };
        assert_eq!(response.headers()["Retry-After"], "30");
        let too_large = http::Response::new(vec![b' '; MAX_BYTES + 1]);
        assert!(lookup(too_large, "Test", "USA").is_err());
        assert!(lookup(http::Response::new(b"invalid JSON".to_vec()), "Test", "USA").is_err());
        assert!(query("a\nb").is_none());
        assert!(
            query(r#"one\two "three""#)
                .unwrap()
                .starts_with(r#"search "one\\two \"three\"";"#)
        );
    }
}
