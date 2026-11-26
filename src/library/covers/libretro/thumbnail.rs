//! GitHub's raw endpoint serves a symlink blob as its literal target filename.
use http::{Response, StatusCode};
use reqwest::Url;

use crate::library::metadata::no_intro;

const MAX_LINKS: usize = 4;

pub(in crate::library::covers) fn fetch_image(
    mut url: Url,
    mut fetch: impl FnMut(Url) -> Result<Response<Vec<u8>>, String>,
) -> Result<Response<Vec<u8>>, String> {
    let base = url.join(".").map_err(|e| e.to_string())?;
    if ![
        super::BOXART,
        no_intro::COVER_BASE,
        no_intro::NDS_COVER_BASE,
    ]
    .contains(&base.as_str())
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Unexpected Libretro box-art directory".into());
    }
    let mut visited = Vec::new();
    loop {
        visited.push(url.clone());
        let response = fetch(url)?;
        let Some(filename) = link_filename(&response) else {
            return Ok(response);
        };
        url = base.clone();
        url.path_segments_mut()
            .map_err(|()| "Invalid Libretro box-art directory")?
            .pop_if_empty()
            .push(filename);
        if visited.contains(&url) || visited.len() > MAX_LINKS {
            return Err("Cyclic or too many Libretro box-art links".into());
        }
    }
}

fn link_filename(response: &Response<Vec<u8>>) -> Option<&str> {
    // Raw GitHub may label these image/png based on the requested filename.
    // A Git symlink stores a path without a trailing newline. A basename on
    // its own must fit in 255 bytes and stays within the original directory.
    if response.status() != StatusCode::OK || response.body().len() > 255 {
        return None;
    }
    if let Some(mime) = response.headers().get(http::header::CONTENT_TYPE) {
        let mime = mime.to_str().ok()?.split(';').next()?.trim();
        if !["image/png", "text/plain", "application/octet-stream"]
            .iter()
            .any(|expected| mime.eq_ignore_ascii_case(expected))
        {
            return None;
        }
    }
    let name = std::str::from_utf8(response.body()).ok()?;
    (name
        .strip_suffix(".png")
        .is_some_and(|stem| !stem.is_empty())
        && !name.contains(['/', '\\', ':', '<', '>'])
        && !name.chars().any(char::is_control))
    .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cover(base: &str) -> Url {
        super::super::image_url(base, "Regional game (Ireland)").unwrap()
    }

    #[test]
    fn follows_same_directory_png_basenames_on_both_platforms() {
        for base in [
            super::super::BOXART,
            no_intro::COVER_BASE,
            no_intro::NDS_COVER_BASE,
        ] {
            let start = cover(base);
            let mut calls = Vec::new();
            let response = fetch_image(start.clone(), |url| {
                calls.push(url);
                let body: &[u8] = match calls.len() {
                    1 => "Alias (日本).png".as_bytes(),
                    2 => b"Game #2 (100%).png",
                    3 => b"\x89PNG\r\n\x1a\n",
                    _ => panic!("unexpected followup"),
                };
                Ok(Response::builder()
                    .header("content-type", "image/png")
                    .body(body.to_vec())
                    .unwrap())
            })
            .unwrap();
            assert_eq!(calls[0], start);
            assert_eq!(
                calls[1].as_str(),
                format!("{base}Alias%20(%E6%97%A5%E6%9C%AC).png")
            );
            assert_eq!(
                calls[2].as_str(),
                format!("{base}Game%20%232%20(100%25).png")
            );
            assert_eq!(response.body(), b"\x89PNG\r\n\x1a\n");
        }
    }

    #[test]
    fn malformed_links_and_http_errors_never_trigger_followups() {
        let start = cover(super::super::BOXART);
        for name in [
            "../file.png",
            "subdir/file.png",
            "https://example.com/other.png",
            "C:\\f.png",
            "f.png\n",
            ".png",
            "<html>1.png",
        ] {
            let mut calls = 0;
            let response = fetch_image(start.clone(), |_| {
                calls += 1;
                Ok(Response::new(name.as_bytes().to_vec()))
            })
            .unwrap();
            assert_eq!(response.body(), name.as_bytes());
            assert_eq!(calls, 1, "{name}");
        }
        for status in [301, 403, 404, 410, 429, 503] {
            let mut calls = 0;
            let response = fetch_image(start.clone(), |_| {
                calls += 1;
                Ok(Response::builder()
                    .status(status)
                    .header("retry-after", "7")
                    .body(b"other.png".to_vec())
                    .unwrap())
            })
            .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["retry-after"], "7");
            assert_eq!(calls, 1);
        }
        assert!(
            link_filename(
                &Response::builder()
                    .header("content-type", "text/html")
                    .body(b"image.png".to_vec())
                    .unwrap()
            )
            .is_none()
        );
        for outside in [
            "https://x.invalid/game.png",
            "https://raw.githubusercontent.com/libretro-thumbnails/Sony_-_PlayStation/master/Named_Snaps/game.png",
            "https://raw.githubusercontent.com/libretro-thumbnails/Sony_-_PlayStation/master/Named_Boxarts/subdir/game.png",
        ] {
            assert!(
                fetch_image(Url::parse(outside).unwrap(), |_| panic!(
                    "unexpected directory fetched"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn cycles_and_unbounded_chains_stop_before_the_next_request() {
        for cycle in [true, false] {
            let mut calls = 0;
            let result = fetch_image(cover(no_intro::COVER_BASE), |_| {
                calls += 1;
                let target = if cycle && calls == 2 {
                    "Regional game (Ireland).png".to_owned()
                } else {
                    format!("Link{calls}.png")
                };
                Ok(Response::new(target.into_bytes()))
            });
            assert!(result.is_err());
            assert_eq!(calls, if cycle { 2 } else { MAX_LINKS + 1 });
        }
    }

    #[test]
    #[ignore = "live Libretro PS1 and GBA box-art symlinks; requires network"]
    fn live_symlinked_boxarts_decode_on_both_platforms() {
        use crate::library::{
            covers::{Lookup, Provider, download_lookup},
            transport,
        };
        let cancel = std::sync::atomic::AtomicBool::default();
        for (base, title) in [
            (super::super::BOXART, "Resident Evil 3 - Nemesis (Ireland)"),
            (
                no_intro::COVER_BASE,
                "All-Star Baseball 2004 (USA) (Beta 1)",
            ),
        ] {
            let url = super::super::image_url(base, title).unwrap();
            let crate::library::covers::CoverResponse::Http(image) = download_lookup(
                transport::api().unwrap(),
                Lookup::Image(url),
                Provider::Libretro,
                &cancel,
            )
            .unwrap() else {
                panic!("missing {title}")
            };
            assert_eq!(image.status(), StatusCode::OK, "{title}");
            let decoded = image::load_from_memory(image.body()).unwrap();
            assert!(decoded.width() > 0 && decoded.height() > 0, "{title}");
        }
    }
}
