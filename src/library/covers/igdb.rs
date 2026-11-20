//! IGDB game localizations associate each localized cover with a region.
//! Its covers have no platform field, so a shared multi-platform entry cannot
//! establish that the image is the PlayStation box.

use std::{sync::atomic::AtomicBool, time::Duration};

use crate::library::transport;
use reqwest::{Client, Url};
use serde::Deserialize;

use super::Lookup;
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
    let configured = "Set EIDORYN_IGDB_CLIENT_ID and EIDORYN_IGDB_ACCESS_TOKEN for IGDB";
    let credential = |key| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or(configured)
    };
    let request = client
        .post(GAMES)
        .header("Client-ID", credential("EIDORYN_IGDB_CLIENT_ID")?)
        .bearer_auth(credential("EIDORYN_IGDB_ACCESS_TOKEN")?)
        .header("Accept", "application/json")
        .body(query);
    let _permit = REQUESTS.acquire(cancel)?;
    let response =
        transport::request(request, MAX_BYTES, cancel).map_err(|error| error.to_string())?;
    if crate::library::http_retry::should_defer(response.status(), response.headers()) {
        REQUESTS.defer(response.headers());
    }
    lookup(response, title, region)
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
    let mut images = game.localizations.iter().filter_map(|l| {
        l.region
            .as_ref()
            .filter(|r| r.matches(region))
            .and(l.cover.as_ref())
            .map(|c| c.image_id.as_str())
    });
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
    fn matches(&self, region: &str) -> bool {
        let identifier = self.identifier.to_ascii_lowercase().replace('_', "-");
        let name = self.name.to_ascii_lowercase();
        match region {
            "USA" => {
                matches!(
                    identifier.as_str(),
                    "us" | "usa" | "en-us" | "north-america"
                ) || matches!(
                    name.as_str(),
                    "united states" | "united states of america" | "north america"
                )
            }
            "Japan" => matches!(identifier.as_str(), "jp" | "ja-jp") || name == "japan",
            "Europe" => identifier == "eu" || name == "europe",
            _ => false,
        }
    }
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
            Lookup::Missing => None,
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
    fn ambiguous_title_platform_or_art_cannot_supply_a_cover() {
        assert!(result(&json!([game(), game()]), "Test Adventure", "USA").is_none());
        let mut mixed = game();
        mixed["platforms"] = json!([{"slug":"ps"}, {"slug":"pc"}]);
        assert!(result(&json!([mixed]), "Test Adventure", "USA").is_none());
        let mut collision = game();
        collision["game_localizations"][0]["region"] = json!({"identifier":"US"});
        assert!(result(&json!([collision]), "Test Adventure", "USA").is_none());
        let mut invalid = game();
        invalid["game_localizations"][0]["cover"]["image_id"] = json!("../screenshot.jpg");
        assert!(result(&json!([invalid]), "Test Adventure", "Japan").is_none());
        assert!(result(&json!(vec![game(); MAX_RESULTS]), "Test Adventure", "USA").is_none());
    }

    #[test]
    fn preserves_http_status_and_limits_upstream_data() {
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
