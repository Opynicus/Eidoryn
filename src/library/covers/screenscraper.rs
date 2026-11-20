//! Resolve a PS1 serial to its regional front cover using the ScreenScraper v2 API.
use super::Lookup;
use crate::library::transport;
use reqwest::{Client, StatusCode, Url, header::CONTENT_TYPE};
use serde_json::Value;
use std::{env, sync::atomic::AtomicBool};

const ENDPOINT: &str = "https://api.screenscraper.fr/api2/jeuInfos.php";
const MAX_RESPONSE: usize = 1024 * 1024;

fn setting(name: &str) -> Option<String> {
    env::var(format!("EIDORYN_SCREENSCRAPER_{name}"))
        .ok()
        .filter(|value| !value.is_empty())
}

fn region_code(region: &str) -> Option<&'static str> {
    match region {
        "USA" | "us" => Some("us"),
        "Europe" | "eu" => Some("eu"),
        "Japan" | "jp" => Some("jp"),
        _ => None,
    }
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
    let reported = rom
        .and_then(|v| v.get("romserial").or_else(|| v.pointer("/romid/romserial")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    if reported.is_some_and(|s| {
        !s.split([',', ';', '|'])
            .any(|id| id.trim().eq_ignore_ascii_case(serial))
    }) {
        return Ok(Lookup::Missing);
    }
    let Some(media) = game.get("medias").and_then(Value::as_array) else {
        return Ok(Lookup::Missing);
    };
    let front = media
        .iter()
        .find(|m| m["type"] == "box-2D" && m["region"] == region);
    match front {
        Some(m) => media_url(
            m["url"]
                .as_str()
                .ok_or("Missing ScreenScraper media address")?,
        )
        .map(Lookup::Image),
        None => Ok(Lookup::Missing),
    }
}

pub(super) fn resolve(
    client: &Client,
    serial: &str,
    region: &str,
    cancel: &AtomicBool,
) -> Result<Lookup, String> {
    let Some(code) = region_code(region) else {
        return Ok(Lookup::Missing);
    };
    let valid = serial.split_once('-').is_some_and(|(prefix, digits)| {
        prefix.len() == 4
            && prefix.bytes().all(|b| b.is_ascii_uppercase())
            && digits.len() == 5
            && digits.bytes().all(|b| b.is_ascii_digit())
    });
    if !valid {
        return Err("Expected a PS1 disc serial for ScreenScraper".into());
    }
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
    let response = transport::request(request, MAX_RESPONSE, cancel).map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Ok(Lookup::Http(response));
    }
    let bytes = response.into_body();
    parse(&bytes, serial, code)
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
        assert!(matches!(run(&data, "us"), Lookup::Image(_)));
        assert!(matches!(run(&data, "eu"), Lookup::Missing));
        data["response"]["jeu"]["rom"]["romserial"] = json!("TEST-10002");
        assert!(matches!(run(&data, "us"), Lookup::Missing));
        data["response"]["jeu"]["rom"]["romserial"] = json!("TEST-10001");
        data["response"]["jeu"]["systeme"]["id"] = json!("58");
        assert!(matches!(run(&data, "us"), Lookup::Missing));
        assert!(region_code("Unknown").is_none());
    }

    #[test]
    fn service_errors_and_untrusted_media_never_echo_secrets() {
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
