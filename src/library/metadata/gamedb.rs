//! Serial-indexed PlayStation releases, including revisions that share a boot serial.
use super::{
    MetadataProvider, Provenance, ProviderError, Release, SourceResponse, transport, valid_date,
};
use crate::library::http_retry::{RequestLimit, should_defer};
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const CATALOG: &str =
    "https://github.com/niemasd/GameDB-PSX/releases/latest/download/PSX.data.json";
const MAX_BYTES: usize = 12 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_hours(24);
const RETRY_DELAY: Duration = Duration::from_secs(30);
static CACHE: Mutex<CatalogCache> = Mutex::new(CatalogCache {
    generation: 0,
    catalog: None,
    failure: None,
});
// Other workers wait cancellably for the first download, then reuse its index.
static DOWNLOAD: RequestLimit = RequestLimit::new(1, Duration::ZERO);

pub(super) struct GameDb;

fn cache() -> MutexGuard<'static, CatalogCache> {
    CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Release the shared catalog after a user explicitly clears the entire release cache.
pub(super) fn clear_cache() {
    let mut cache = cache();
    cache.generation = cache.generation.wrapping_add(1);
    cache.catalog = None;
    cache.failure = None;
}

pub(super) fn is_supported_serial(serial: &str) -> bool {
    let Some((prefix, number)) = serial.split_once('-') else {
        return false;
    };
    matches!(
        prefix,
        "CPCS"
            | "ESPM"
            | "PAPX"
            | "PBPX"
            | "PCPD"
            | "PCPX"
            | "PEPX"
            | "PTPX"
            | "PUPX"
            | "SCAJ"
            | "SCED"
            | "SCES"
            | "SCPM"
            | "SCPS"
            | "SCUS"
            | "SCZS"
            | "SIPS"
            | "SLED"
            | "SLES"
            | "SLKA"
            | "SLPM"
            | "SLPS"
            | "SLUS"
            | "SPUS"
    ) && number.len() == 5
        && number.bytes().all(|c| c.is_ascii_digit())
}

struct Catalog {
    // Keep the original records for the SQLite provenance payload. Index the
    // serial *field* because revisions can have different top-level keys.
    by_serial: HashMap<String, Vec<(String, Box<RawValue>)>>,
}

impl Catalog {
    fn parse(bytes: &[u8]) -> Result<Self, ProviderError> {
        #[derive(Deserialize)]
        struct Disc<'a> {
            serial: Option<&'a str>,
        }
        let records: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(bytes)
            .map_err(|e| ProviderError::Invalid(format!("Invalid GameDB-PSX catalog: {e}")))?;
        let mut by_serial: HashMap<_, Vec<_>> = HashMap::new();
        for (key, record) in records {
            if let Ok(Disc {
                serial: Some(serial),
            }) = serde_json::from_str(record.get())
                && is_supported_serial(serial)
            {
                by_serial
                    .entry(serial.to_owned())
                    .or_default()
                    .push((key, record));
            }
        }
        if by_serial.is_empty() {
            return Err(ProviderError::Invalid(
                "GameDB-PSX catalog contains no PlayStation serials".into(),
            ));
        }
        Ok(Self { by_serial })
    }

    fn lookup(&self, serial: &str) -> Result<SourceResponse, ProviderError> {
        let mut releases = Vec::new();
        let mut raw = BTreeMap::new();
        for (key, source) in self.by_serial.get(serial).into_iter().flatten() {
            raw.insert(key, source);
            let record: Value = serde_json::from_str(source.get())
                .map_err(|e| ProviderError::Invalid(e.to_string()))?;
            let Some(region) = release_region(serial, &record) else {
                continue;
            };
            let title = record
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if title.is_empty() {
                continue;
            }
            let date = record
                .get("release_date")
                .and_then(Value::as_str)
                .filter(|value| !region.contains(',') && valid_date(value));
            releases.push(Release {
                id: format!("gamedb/{key}"),
                serial: serial.into(),
                region: region.into(),
                title: title.into(),
                aliases: record
                    .get("release_name")
                    .and_then(Value::as_str)
                    .filter(|name| *name != title)
                    .unwrap_or_default()
                    .into(),
                date: date.unwrap_or_default().into(),
                work_date: String::new(),
            });
        }
        let raw = serde_json::to_vec(&raw).map_err(|e| ProviderError::Invalid(e.to_string()))?;
        Ok(SourceResponse {
            releases,
            raw,
            source: GameDb::SOURCE,
        })
    }
}

/// The catalog groups several release markets under one video territory.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoTerritory {
    NtscU,
    NtscJ,
    Pal,
}

impl VideoTerritory {
    fn from_catalog(code: &str) -> Option<Self> {
        match code {
            "NTSC-U" => Some(Self::NtscU),
            "NTSC-J" => Some(Self::NtscJ),
            "PAL" => Some(Self::Pal),
            _ => None,
        }
    }

    fn for_market(market: &str) -> Option<Self> {
        match market {
            "USA" | "Canada" => Some(Self::NtscU),
            "Japan" | "Asia" | "Korea" | "Taiwan" | "China" => Some(Self::NtscJ),
            "Australia" | "Israel" => Some(Self::Pal),
            country if crate::library::european_market(country) => Some(Self::Pal),
            _ => None,
        }
    }
}

fn release_region<'a>(serial: &str, record: &'a Value) -> Option<&'a str> {
    let territory = VideoTerritory::from_catalog(record.get("region")?.as_str()?)?;
    let name = record
        .get("release_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // A release name can have other parenthetical groups before its markets,
    // and language, revision and disc tags after them.
    for group in name
        .split('(')
        .skip(1)
        .filter_map(|s| s.split_once(')').map(|(tag, _)| tag))
    {
        let mut territories = group
            .split(',')
            .map(|market| VideoTerritory::for_market(market.trim()));
        if territories.clone().all(|candidate| candidate.is_some()) {
            return territories
                .any(|candidate| candidate == Some(territory))
                .then_some(group);
        }
    }
    match territory {
        VideoTerritory::NtscU => Some("USA"),
        VideoTerritory::Pal => Some("Europe"),
        // These NTSC-J serials also cover Asian markets outside Japan.
        VideoTerritory::NtscJ
            if serial.starts_with("SCPS-45")
                || serial.starts_with("SCAJ-")
                || serial.starts_with("SLKA-") =>
        {
            None
        }
        VideoTerritory::NtscJ => Some("Japan"),
    }
}

fn download(
    client: &reqwest::Client,
    url: &str,
    cancel: &AtomicBool,
    limit: &RequestLimit,
) -> Result<Catalog, ProviderError> {
    // The redirected download contains the entire disc catalog, even for a single serial.
    let request = client.get(url).timeout(Duration::from_secs(120));
    let response = transport::request(request, MAX_BYTES, cancel)?;
    if should_defer(response.status(), response.headers()) {
        limit.defer(response.headers());
    }
    if !response.status().is_success() {
        return Err(ProviderError::http(response.status(), response.headers()));
    }
    Catalog::parse(response.body())
}

struct CatalogCache {
    generation: u64,
    catalog: Option<(Instant, Catalog)>,
    failure: Option<(Instant, ProviderError)>,
}

impl CatalogCache {
    fn reuse(&self, serial: &str, now: Instant) -> Option<Result<SourceResponse, ProviderError>> {
        if let Some((loaded, catalog)) = &self.catalog
            && now.duration_since(*loaded) < MAX_AGE
        {
            return Some(catalog.lookup(serial));
        }
        if let Some((failed, cause)) = &self.failure
            && now.duration_since(*failed) < RETRY_DELAY
        {
            return Some(self.stale_or_error(serial, cause));
        }
        None
    }

    fn stale_or_error(
        &self,
        serial: &str,
        error: &ProviderError,
    ) -> Result<SourceResponse, ProviderError> {
        if *error != ProviderError::Cancelled
            && let Some((_, catalog)) = &self.catalog
        {
            let previous = catalog.lookup(serial)?;
            if !previous.releases.is_empty() {
                return Ok(previous);
            }
        }
        Err(error.clone())
    }

    fn publish(
        &mut self,
        serial: &str,
        result: Result<Catalog, ProviderError>,
        now: Instant,
    ) -> Result<SourceResponse, ProviderError> {
        match result {
            Ok(catalog) => {
                let response = catalog.lookup(serial);
                self.catalog = Some((now, catalog));
                self.failure = None;
                response
            }
            Err(error) => {
                if error != ProviderError::Cancelled {
                    self.failure = Some((now, error.clone()));
                }
                self.stale_or_error(serial, &error)
            }
        }
    }
}

impl MetadataProvider for GameDb {
    const SOURCE: Provenance = Provenance {
        provider: "GameDB-PSX",
        payload_version: 1,
    };

    fn search(&self, serial: &str, cancel: &AtomicBool) -> Result<SourceResponse, ProviderError> {
        if !is_supported_serial(serial) {
            return Err(ProviderError::Invalid(
                "Expected a PS1 serial such as SLUS-00067".into(),
            ));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(ProviderError::Cancelled);
        }
        let generation = cache().generation;
        let permit = DOWNLOAD.acquire_typed(cancel);
        // A waiting worker can be cancelled while another holds the cache.
        if matches!(permit, Err(ProviderError::Cancelled)) {
            return Err(ProviderError::Cancelled);
        }
        let _permit = {
            let cache = cache();
            if cache.generation != generation {
                return Err(ProviderError::Cancelled);
            }
            if let Some(response) = cache.reuse(serial, Instant::now()) {
                return response;
            }
            match permit {
                Ok(permit) => permit,
                Err(error) => return cache.stale_or_error(serial, &error),
            }
        };
        // The public releases URL redirects to GitHub's asset CDN.
        let result =
            transport::art().and_then(|client| download(client, CATALOG, cancel, &DOWNLOAD));
        let mut cache = cache();
        if cache.generation != generation {
            return Err(ProviderError::Cancelled);
        }
        cache.publish(serial, result, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "SLPS-10000": {"serial":"SLPS-10000","region":"NTSC-J","title":"Example","release_date":"1996","release_name":"Example (Japan)","extra":{"test":"kept"}},
            "SLPS-10000-R1": {"serial":"SLPS-10000","region":"NTSC-J","title":"Example (Rev 1)","release_date":"1997-04"},
            "SLUS-10000": {"serial":"SLUS-10000","region":"NTSC-U","title":"US Example","release_date":"1997-02-30"},
            "SLUS-10001": {"serial":"SLUS-10001","region":"NTSC-U","title":"Undated"},
            "SCPS-45001": {"serial":"SCPS-45001","region":"NTSC-J","title":"Asian release"},
            "SCPS-45002": {"serial":"SCPS-45002","region":"NTSC-J","title":"Asian release","release_name":"Example (Asia)"},
            "SLPS-10001": {"serial":"SLPS-10001","region":"NTSC-U","title":"Incorrect region","release_name":"Example (Japan)"},
            "SLPS-10002": {"region":"NTSC-J","title":"Missing serial"},
            "SLPS-10003": {"serial":"SLPS-10003","region":"Unknown","title":"Unknown territory","release_name":"Example (Japan)"},
        })).unwrap()
    }

    #[test]
    fn returns_each_revision_with_source_precision_and_original_evidence() {
        let catalog = Catalog::parse(&fixture()).unwrap();
        let result = catalog.lookup("SLPS-10000").unwrap();
        assert_eq!(result.source.provider, "GameDB-PSX");
        assert_eq!(
            result
                .releases
                .iter()
                .map(|r| (r.id.as_str(), r.date.as_str()))
                .collect::<Vec<_>>(),
            [
                ("gamedb/SLPS-10000", "1996"),
                ("gamedb/SLPS-10000-R1", "1997-04")
            ]
        );
        let raw: Value = serde_json::from_slice(&result.raw).unwrap();
        assert_eq!(raw.as_object().unwrap().len(), 2);
        assert_eq!(raw["SLPS-10000"]["extra"]["test"], "kept");
        assert_eq!(catalog.lookup("SLUS-10000").unwrap().releases[0].date, "");
        assert_eq!(catalog.lookup("SLUS-10001").unwrap().releases[0].date, "");
        assert_eq!(
            catalog.lookup("SCPS-45002").unwrap().releases[0].region,
            "Asia"
        );
        for serial in [
            "SLPS-99999",
            "SLPS-10001",
            "SLPS-10002",
            "SLPS-10003",
            "SCPS-45001",
        ] {
            assert!(catalog.lookup(serial).unwrap().releases.is_empty());
        }
        for invalid in [
            "FAKE-12345",
            "../../game",
            "SLUS-00１23",
            "Disc 1",
            "SLUS-0001",
        ] {
            assert!(!is_supported_serial(invalid));
        }
        assert!(Catalog::parse(b"<html>").is_err());
        assert!(Catalog::parse(br#"{"broken":{"id":"?"}}"#).is_err());
    }

    #[test]
    fn explicit_markets_keep_their_scope_and_joint_markets_have_no_regional_date() {
        for (market, video, expected_date) in [
            ("Australia", "PAL", "1997-03"),
            ("Canada", "NTSC-U", "1997-03"),
            ("Israel", "PAL", "1997-03"),
            ("France", "PAL", "1997-03"),
            ("Japan, Asia", "NTSC-J", ""),
            ("Europe, Australia", "PAL", ""),
        ] {
            let entry = json!({"SLUS-10000":{"serial":"SLUS-10000","region":video,"release_name":format!("Example (Demo) ({market})"),"title":"Example","release_date":"1997-03"}});
            let catalog = Catalog::parse(&serde_json::to_vec(&entry).unwrap()).unwrap();
            let result = catalog.lookup("SLUS-10000").unwrap();
            assert_eq!(result.releases[0].region, market);
            assert_eq!(result.releases[0].date, expected_date);
        }
    }

    #[test]
    fn failed_refresh_reuses_known_records_and_backs_off_for_uncached_games() {
        let loaded = Instant::now();
        let now = loaded + MAX_AGE;
        let mut cache = CatalogCache {
            generation: 0,
            catalog: Some((loaded, Catalog::parse(&fixture()).unwrap())),
            failure: None,
        };
        assert!(cache.reuse("SLPS-10000", now).is_none());
        assert_eq!(
            cache
                .publish("SLPS-10000", Err(ProviderError::Cancelled), now)
                .unwrap_err(),
            ProviderError::Cancelled
        );
        assert!(cache.failure.is_none());
        assert_eq!(
            cache
                .publish("SLPS-10000", Err(ProviderError::Timeout), now)
                .unwrap()
                .releases
                .len(),
            2
        );
        assert_eq!(
            cache
                .reuse("SLPS-10000", now)
                .unwrap()
                .unwrap()
                .releases
                .len(),
            2
        );
        assert_eq!(
            cache.reuse("SLUS-99999", now).unwrap().unwrap_err(),
            ProviderError::Timeout
        );
        let now = now + RETRY_DELAY;
        assert!(cache.reuse("SLUS-99999", now).is_none());
        cache
            .publish("SLPS-10000", Catalog::parse(&fixture()), now)
            .unwrap();
        assert!(cache.failure.is_none());
        assert!(
            cache
                .reuse("SLUS-99999", now)
                .unwrap()
                .unwrap()
                .releases
                .is_empty()
        );
    }

    #[test]
    #[ignore = "requires the public GameDB-PSX release service"]
    fn live_gamedb_keeps_a_regional_release_date_and_distinct_ps1_revisions() {
        let cancel = AtomicBool::new(false);
        let us = GameDb.search("SLUS-00067", &cancel).unwrap();
        assert!(
            us.releases
                .iter()
                .any(|r| r.region == "USA" && r.date == "1997-10-15")
        );
        let revisions = GameDb.search("SLPS-00004", &cancel).unwrap();
        assert!(revisions.releases.len() >= 2);
        assert!(revisions.releases.iter().all(|r| r.region == "Japan"));
    }

    #[test]
    #[ignore = "requires a local loopback socket"]
    fn fetches_the_public_catalog_through_http() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/PSX.data.json", listener.local_addr().unwrap());
        thread::scope(|scope| {
            scope.spawn(|| {
                let (mut socket, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&socket);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with("GET /PSX.data.json "));
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                let payload = fixture();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
                .unwrap();
                socket.write_all(&payload).unwrap();
            });
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let catalog = download(
                &client,
                &url,
                &AtomicBool::new(false),
                &RequestLimit::new(1, Duration::ZERO),
            )
            .unwrap();
            assert_eq!(catalog.lookup("SLPS-10000").unwrap().releases.len(), 2);
        });
    }
}
