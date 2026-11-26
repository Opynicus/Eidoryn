//! Resolve a PS1 serial to its regional front cover using the ScreenScraper v2 API.
use super::{Lookup, markets::MARKETS};
use crate::library::{metadata, transport};
use reqwest::{Client, RequestBuilder, StatusCode, Url, header::CONTENT_TYPE};
use serde_json::Value;
use std::{env, sync::atomic::AtomicBool};

const ENDPOINT: &str = "https://api.screenscraper.fr/api2/jeuInfos.php";
const MAX_RESPONSE: usize = 1024 * 1024;

fn setting(name: &str) -> Option<String> {
    env::var(format!("EIDORYN_SCREENSCRAPER_{name}"))
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn media_url(raw: &str) -> Result<Url, String> {
    let invalid = "Unexpected ScreenScraper media address";
    let url = Url::parse(raw).map_err(|_| invalid)?;
    if url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("api.screenscraper.fr" | "www.screenscraper.fr" | "screenscraper.fr")
        )
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.path() == "/api2/mediaJeu.php"
    {
        Ok(url)
    } else {
        Err(invalid.into())
    }
}

fn parse(bytes: &[u8], serial: &str, region: &str) -> Result<Lookup, String> {
    // The server can echo credentialed media URLs and account information, even in errors.
    let payload: Value = serde_json::from_slice(bytes).map_err(|_| "Invalid ScreenScraper JSON")?;
    if payload.pointer("/response/erreur").is_some() || payload.get("erreur").is_some() {
        return Err("ScreenScraper rejected the lookup".into());
    }
    let game = payload
        .pointer("/response/jeu")
        .and_then(Value::as_object)
        .ok_or("Missing game in ScreenScraper response")?;
    match game.get("systeme").and_then(|value| value.get("id")) {
        Some(value) if value.as_str() == Some("57") || value.as_u64() == Some(57) => {}
        Some(_) => return Ok(Lookup::Missing),
        None => return Err("Missing platform in ScreenScraper response".into()),
    }
    // When the API identifies a specific ROM, ignore a conflicting serial.
    let rom = game.get("rom");
    let rom_field = |field: &str| {
        rom.and_then(|v| {
            v.get("romid")
                .and_then(|id| id.get(field))
                .or_else(|| v.get(field))
        })
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    };
    let reported = rom_field("romserial");
    if reported.is_some_and(|s| {
        !s.split([',', ';', '|'])
            .any(|id| id.trim().eq_ignore_ascii_case(serial))
    }) {
        return Ok(Lookup::Missing);
    }
    let Some(media) = game.get("medias").and_then(Value::as_array) else {
        return Ok(Lookup::Missing);
    };
    let rom_regions = rom_field("romregions");
    let disc_only = matches!(region, "NTSC-U" | "NTSC-J" | "PAL");
    let mut front = None;
    for item in media.iter().filter(|m| m["type"] == "box-2D") {
        let Some(code) = item["region"].as_str() else {
            continue;
        };
        let Some(market) = MARKETS.iter().find(|m| m.screenscraper == Some(code)) else {
            continue;
        };
        let rom_match = rom_regions.map(|codes| codes.split(',').any(|part| part.trim() == code));
        // Game-wide media may include a sibling release in the same licensing territory.
        let matches = if disc_only {
            rom_match == Some(true) && metadata::region_matches(region, market.name)
        } else {
            market.requested(region) && rom_match != Some(false)
        };
        if matches {
            let url = media_url(
                item["url"]
                    .as_str()
                    .ok_or("Missing ScreenScraper media address")?,
            )?;
            if front.as_ref().is_some_and(|existing| *existing != url) {
                return Ok(Lookup::Missing);
            }
            front = Some(url);
        }
    }
    Ok(front.map_or(Lookup::Missing, Lookup::Image))
}

pub(super) fn resolve(
    client: &Client,
    serial: &str,
    region: &str,
    cancel: &AtomicBool,
) -> Result<Lookup, String> {
    if !matches!(region, "NTSC-U" | "NTSC-J" | "PAL")
        && !MARKETS
            .iter()
            .any(|m| m.screenscraper.is_some() && m.requested(region))
    {
        return Ok(Lookup::Missing);
    }
    let valid = serial.split_once('-').is_some_and(|(prefix, digits)| {
        prefix.len() == 4
            && prefix.bytes().all(|b| b.is_ascii_uppercase())
            && digits.len() == 5
            && digits.bytes().all(|b| b.is_ascii_digit())
    });
    if !valid {
        return Err("Expected a PS1 disc serial for ScreenScraper".into());
    }
    let request = lookup_request(client, serial, setting)?;
    let response = transport::request(request, MAX_RESPONSE, cancel).map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Ok(Lookup::Http(response));
    }
    parse(response.body(), serial, region)
}

fn lookup_request(
    client: &Client,
    serial: &str,
    setting: impl Fn(&str) -> Option<String>,
) -> Result<RequestBuilder, String> {
    let dev_id = setting("DEV_ID").ok_or("ScreenScraper requires EIDORYN_SCREENSCRAPER_DEV_ID")?;
    let dev_password = setting("DEV_PASSWORD")
        .ok_or("ScreenScraper requires EIDORYN_SCREENSCRAPER_DEV_PASSWORD")?;
    let mut request = client.get(ENDPOINT).query(&[
        ("devid", dev_id.as_str()),
        ("devpassword", dev_password.as_str()),
        ("softname", concat!("Eidoryn ", env!("CARGO_PKG_VERSION"))),
        ("output", "json"),
        ("systemeid", "57"),
        ("romtype", "iso"),
        ("serialnum", serial),
    ]);
    match (setting("USER"), setting("PASSWORD")) {
        (Some(user), Some(password)) => {
            request = request.query(&[("ssid", user), ("sspassword", password)]);
        }
        (None, None) => {}
        _ => return Err("Set both ScreenScraper user and password to use a player account".into()),
    }
    Ok(request)
}

/// ScreenScraper documents a successful text response containing NOMEDIA for absent art.
pub(super) fn fetch_image(
    client: &Client,
    url: Url,
    cancel: &AtomicBool,
) -> Result<Option<http::Response<Vec<u8>>>, String> {
    let response =
        transport::cover(client.get(url), super::MAX_BYTES, cancel).map_err(|e| e.to_string())?;
    image_response(response)
}

fn image_response(
    response: http::Response<Vec<u8>>,
) -> Result<Option<http::Response<Vec<u8>>>, String> {
    let plain_text = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/plain"));
    if response.status() != StatusCode::OK || !plain_text {
        return Ok(Some(response));
    }
    let text = response.body();
    if text.len() <= 64 && text.trim_ascii() == b"NOMEDIA" {
        Ok(None)
    } else {
        Err("Unexpected ScreenScraper text response for media".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> Value {
        json!({"response":{"jeu":{"systeme":{"id":"57"},"rom":{"romserial":"TEST-10001"},"medias":[
            {"type":"box-2D-back","region":"us","url":"https://example.org/back"},
            {"type":"box-2D","region":"jp","url":"https://example.org/jp"},
            {"type":"box-2D","region":"us","url":"https://api.screenscraper.fr/api2/mediaJeu.php?gameid=0&media=box-2D(us)"}
        ]}}})
    }

    #[test]
    fn selects_matching_platform_serial_region_and_front() {
        let mut data = payload();
        let run = |data: &Value, region| {
            parse(&serde_json::to_vec(data).unwrap(), "TEST-10001", region).unwrap()
        };
        assert!(matches!(run(&data, "USA"), Lookup::Image(_)));
        assert!(matches!(run(&data, "us"), Lookup::Image(_)));
        assert!(matches!(run(&data, "Europe"), Lookup::Missing));
        data["response"]["jeu"]["rom"]["romserial"] = json!("TEST-10002");
        assert!(matches!(run(&data, "USA"), Lookup::Missing));
        data["response"]["jeu"]["rom"]["romserial"] = json!("TEST-10001");
        data["response"]["jeu"]["systeme"]["id"] = json!("58");
        assert!(matches!(run(&data, "USA"), Lookup::Missing));
    }

    fn fronts(codes: &[&str], rom_regions: Option<&str>) -> Value {
        let media: Vec<_> = codes.iter().map(|code| json!({
            "type": "box-2D", "region": code,
            "url": format!("https://api.screenscraper.fr/api2/mediaJeu.php?media=box-2D({code})"),
        })).collect();
        json!({"response":{"jeu":{"systeme":{"id":57},
            "rom":{"romid":{"romserial":"SLPM-86023","romregions":rom_regions}},
            "medias": media}}})
    }

    fn selected(data: &Value, requested: &str) -> Option<String> {
        match parse(&serde_json::to_vec(data).unwrap(), "SLPM-86023", requested).unwrap() {
            Lookup::Image(url) => Some(
                url.query_pairs()
                    .find(|(k, _)| k == "media")
                    .unwrap()
                    .1
                    .into_owned(),
            ),
            Lookup::NeedsChoice | Lookup::Missing => None,
            Lookup::Http(_) => panic!("unexpected HTTP response"),
        }
    }

    #[test]
    fn selects_only_the_requested_front_across_screenscraper_markets() {
        let markets = [
            ("Asia", "asi"),
            ("Australia", "au"),
            ("Canada", "ca"),
            ("China", "cn"),
            ("Denmark", "dk"),
            ("Europe", "eu"),
            ("Finland", "fi"),
            ("France", "fr"),
            ("Germany", "de"),
            ("Greece", "gr"),
            ("Israel", "il"),
            ("Italy", "it"),
            ("Japan", "jp"),
            ("Korea", "kr"),
            ("Netherlands", "nl"),
            ("Norway", "no"),
            ("Poland", "pl"),
            ("Portugal", "pt"),
            ("Russia", "ru"),
            ("Spain", "sp"),
            ("Sweden", "se"),
            ("Taiwan", "tw"),
            ("UK", "uk"),
            ("USA", "us"),
            ("World", "wor"),
        ];
        let codes: Vec<_> = markets.iter().map(|(_, code)| *code).collect();
        let data = fronts(&codes, None);
        for (market, expected) in markets {
            assert_eq!(
                selected(&data, market),
                Some(format!("box-2D({expected})")),
                "{market}"
            );
        }
        for unavailable in [
            "Austria",
            "Belgium",
            "Ireland",
            "Scandinavia",
            "Singapore",
            "",
            "Unknown",
        ] {
            assert_eq!(selected(&data, unavailable), None, "{unavailable}");
        }
    }

    #[test]
    fn intersects_territory_with_the_identified_rom_and_rejects_ambiguous_media() {
        for (license, code) in [("NTSC-U", "ca"), ("NTSC-J", "kr"), ("PAL", "au")] {
            let codes = ["us", "jp", "eu", "ca", "kr", "au"];
            let data = fronts(&codes, Some(code));
            assert_eq!(
                selected(&data, license),
                Some(format!("box-2D({code})")),
                "{license}"
            );
            assert_eq!(selected(&fronts(&codes, None), license), None);
        }
        let codes = ["us", "ca", "jp"];
        let data = fronts(&codes, Some("ca, us"));
        assert_eq!(selected(&data, "NTSC-U"), None);
        assert_eq!(selected(&data, "Japan"), None);
        assert_eq!(selected(&data, "USA / Canada"), None);
        let shared = fronts(&["ca", "ca"], Some("us,ca"));
        assert_eq!(
            selected(&shared, "USA, Canada").as_deref(),
            Some("box-2D(ca)")
        );
        let mut conflict = shared;
        conflict["response"]["jeu"]["medias"][0]["url"] =
            json!("https://api.screenscraper.fr/api2/mediaJeu.php?media=different");
        assert_eq!(selected(&conflict, "Canada"), None);
    }

    #[test]
    fn service_errors_and_untrusted_media_never_echo_secrets() {
        let client = Client::new();
        for (missing, expected) in [
            (
                "DEV_ID",
                "ScreenScraper requires EIDORYN_SCREENSCRAPER_DEV_ID",
            ),
            (
                "DEV_PASSWORD",
                "ScreenScraper requires EIDORYN_SCREENSCRAPER_DEV_PASSWORD",
            ),
            (
                "USER",
                "Set both ScreenScraper user and password to use a player account",
            ),
            (
                "PASSWORD",
                "Set both ScreenScraper user and password to use a player account",
            ),
        ] {
            let result = lookup_request(&client, "SLPM-86023", |key| {
                (key != missing).then(|| "test-value".to_owned())
            });
            assert_eq!(result.err().as_deref(), Some(expected));
        }
        for raw in [
            "http://api.screenscraper.fr/api2/mediaJeu.php",
            "https://api.screenscraper.fr.evil.test/api2/mediaJeu.php",
            "https://api.screenscraper.fr:9999/api2/mediaJeu.php",
            "https://pass@api.screenscraper.fr/api2/mediaJeu.php",
            "https://api.screenscraper.fr/other.php?key=private",
        ] {
            assert_eq!(
                media_url(raw).unwrap_err(),
                "Unexpected ScreenScraper media address"
            );
        }
        assert!(
            matches!(parse(br#"{"response":{"erreur":"private"}}"#, "TEST-10001", "us"),Err(e) if !e.contains("private"))
        );
    }

    #[test]
    fn missing_media_sentinel_preserves_http_errors_and_binary_streams() {
        let response = |status, mime, bytes: &[u8]| {
            http::Response::builder()
                .status(status)
                .header("content-type", mime)
                .body(bytes.to_vec())
                .unwrap()
        };
        let text = "text/plain; charset=utf-8";
        assert!(
            image_response(response(200, text, b"NOMEDIA\n"))
                .unwrap()
                .is_none()
        );
        assert!(
            matches!(image_response(response(200, text, b"private credentials")), Err(e) if !e.contains("private"))
        );
        let limited = image_response(response(429, text, b"NOMEDIA"))
            .unwrap()
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        let good = image_response(response(200, "image/png", b"data"))
            .unwrap()
            .unwrap();
        assert_eq!(good.body(), b"data");
    }
}
