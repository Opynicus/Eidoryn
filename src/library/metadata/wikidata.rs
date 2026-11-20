//! Query the source catalog by PS1 serial and preserve the reported date precision.
use super::{
    MetadataProvider, Provenance, ProviderError, Release, SourceResponse, transport, valid_date,
};
use crate::library::http_retry::RequestLimit;
use reqwest::Client;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
    time::Duration,
};

const ENDPOINT: &str = "https://query.wikidata.org/sparql";
const MAX_RESPONSE: usize = 1024 * 1024;
// The public Wikidata Query Service accepts five concurrent queries per IP.
static REQUESTS: RequestLimit = RequestLimit::new(5, Duration::ZERO);

// The serial and all substitutions below come from checked identifiers and fixed tables.
// Start with the indexed P2606 literal. Work-level platform alone is ambiguous for ports.
const QUERY: &str = r#"
SELECT DISTINCT ?game ?title ?local ?alias ?date ?precision ?firstDate ?firstPrecision WHERE {
  ?game wdt:P2606 "$SERIAL"; wdt:P400 wd:Q10677; p:P2606 ?id; rdfs:label ?title.
  FILTER(LANG(?title) = "en")
  ?id ps:P2606 "$SERIAL"; a wikibase:BestRank.
  FILTER NOT EXISTS { ?id pq:P400 ?foreign. FILTER(?foreign != wd:Q10677) }
  FILTER(EXISTS { ?id pq:P400 wd:Q10677 } || ?game = $REVIEWED ||
         NOT EXISTS { ?game wdt:P400 ?otherPlatform. FILTER(?otherPlatform != wd:Q10677) })
  OPTIONAL { ?game rdfs:label ?local. FILTER(LANG(?local) = "ja") }
  OPTIONAL { ?game skos:altLabel ?alias. FILTER(LANG(?alias) = "en") }
  OPTIONAL {
    ?game p:P577 ?edition. ?edition pq:P291 ?place; psv:P577 ?time.
    FILTER(?place IN ($PLACES))
    FILTER NOT EXISTS { ?edition wikibase:rank wikibase:DeprecatedRank }
    FILTER(EXISTS { ?edition pq:P400 wd:Q10677 } ||
           (NOT EXISTS { ?edition pq:P400 ?p } &&
            (?game = wd:Q719176 ||
             NOT EXISTS { ?game wdt:P400 ?other. FILTER(?other != wd:Q10677) })))
    FILTER NOT EXISTS {
      ?game wdt:P2606 ?otherSerial.
      FILTER(?otherSerial != "$SERIAL" && REGEX(?otherSerial, "$TERRITORY"))
    }
    ?time wikibase:timeValue ?date; wikibase:timePrecision ?precision.
  }
  OPTIONAL {
    ?game p:P577 ?first. ?first psv:P577 ?firstTime.
    FILTER NOT EXISTS { ?first wikibase:rank wikibase:DeprecatedRank }
    FILTER NOT EXISTS { ?first pq:P291 ?firstPlace }
    FILTER NOT EXISTS { ?first pq:P400 ?firstPlatform }
    FILTER NOT EXISTS { ?game wdt:P400 ?platform. FILTER(?platform != wd:Q10677) }
    FILTER NOT EXISTS { ?game p:P577 ?different. FILTER(?different != ?first) }
    ?firstTime wikibase:timeValue ?firstDate; wikibase:timePrecision ?firstPrecision.
  }
}
"#;

struct Territory {
    region: &'static str,
    places: &'static str,
    serial_pattern: &'static str,
}

fn territory(serial: &str) -> Result<Territory, String> {
    let Some((prefix, number)) = serial.split_once('-') else {
        return Err("Expected a PS1 serial such as SLUS-00067".into());
    };
    if number.len() != 5 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Expected a PS1 serial such as SLUS-00067".into());
    }
    let (region, places, serial_pattern) = match prefix {
        "SCUS" | "SLUS" => ("USA", "wd:Q30, wd:Q49", "^(SCUS|SLUS)-"),
        "SCES" | "SLES" => ("Europe", "wd:Q46", "^(SCES|SLES)-"),
        "SCPS" | "SLPS" | "SLPM" | "SIPS" if !serial.starts_with("SCPS-45") => {
            ("Japan", "wd:Q17", "^(SCPS|SLPS|SLPM|SIPS)-")
        }
        _ => return Err("Unknown PS1 release territory for this serial".into()),
    };
    Ok(Territory {
        region,
        places,
        serial_pattern,
    })
}

pub(in crate::library) fn serial_region(serial: &str) -> Option<&'static str> {
    territory(serial).ok().map(|t| t.region)
}

fn query(serial: &str, territory: &Territory) -> String {
    // These reviewed disc identifiers also occur on works listing later ports.
    // This allows lookup of their live records when the serial statement omits a platform.
    let reviewed = match serial {
        "SCES-00344" | "SCPS-10031" | "SCUS-94900" => "wd:Q719176",
        "SCES-00001" | "SLPS-00001" | "SCUS-94300" => "wd:Q15087581",
        "SLPS-01776" => "wd:Q7642406",
        _ => "wd:Q0", // no reviewed item
    };
    QUERY
        .replace("$SERIAL", serial)
        .replace("$PLACES", territory.places)
        .replace("$TERRITORY", territory.serial_pattern)
        .replace("$REVIEWED", reviewed)
}

#[derive(Deserialize)]
struct Value {
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    game: Value,
    title: Value,
    local: Option<Value>,
    alias: Option<Value>,
    date: Option<Value>,
    precision: Option<Value>,
    first_date: Option<Value>,
    first_precision: Option<Value>,
}

#[derive(Deserialize)]
struct Response {
    results: Results,
}

#[derive(Deserialize)]
struct Results {
    bindings: Vec<Row>,
}

fn date(value: Option<&Value>, precision: Option<&Value>) -> Option<String> {
    let width = match precision?.value.as_str() {
        "9" => 4,
        "10" => 7,
        "11" => 10,
        _ => return None,
    };
    let date = value?.value.trim_start_matches('+').get(..width)?;
    valid_date(date).then(|| date.to_owned())
}

#[derive(Default)]
struct Edition {
    title: String,
    aliases: BTreeSet<String>,
    dates: BTreeSet<String>,
    work_date: String,
}

fn parse(bytes: &[u8], serial: &str, region: &str) -> Result<Vec<Release>, String> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid Wikidata response: {e}"))?;
    let mut editions: BTreeMap<String, Edition> = BTreeMap::new();
    for row in response.results.bindings {
        let qid = row
            .game
            .value
            .strip_prefix("http://www.wikidata.org/entity/")
            .or_else(|| {
                row.game
                    .value
                    .strip_prefix("https://www.wikidata.org/entity/")
            })
            .filter(|id| {
                id.strip_prefix('Q')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .ok_or("Invalid Wikidata item identifier")?;
        if row.title.value.trim().is_empty() {
            return Err("Missing Wikidata title".into());
        }
        let edition = editions.entry(qid.to_owned()).or_default();
        let english = row.title.value;
        edition.title = if region == "Japan" {
            row.local
                .filter(|l| !l.value.trim().is_empty())
                .map_or_else(|| english.clone(), |l| l.value)
        } else {
            english.clone()
        };
        edition.aliases.insert(english);
        if let Some(alias) = row.alias {
            edition.aliases.insert(alias.value);
        }
        if let Some(date) = date(row.date.as_ref(), row.precision.as_ref()) {
            edition.dates.insert(date);
        }
        if let Some(first) = date(row.first_date.as_ref(), row.first_precision.as_ref()) {
            edition.work_date = first;
        }
    }
    let mut releases = Vec::new();
    for (qid, mut edition) in editions {
        edition.aliases.remove(&edition.title);
        let aliases = edition.aliases.into_iter().collect::<Vec<_>>().join("|");
        if edition.dates.is_empty() {
            edition.dates.insert(String::new());
        }
        let conflict = edition.dates.len() > 1;
        for date in edition.dates {
            let id = if conflict {
                format!("{qid}/{serial}/{date}")
            } else {
                format!("{qid}/{serial}")
            };
            releases.push(Release {
                id,
                serial: serial.to_owned(),
                region: region.to_owned(),
                title: edition.title.clone(),
                date,
                aliases: aliases.clone(),
                work_date: edition.work_date.clone(),
            });
        }
    }
    Ok(releases)
}

/// Wikidata returns normalized release details together with all exact-serial candidates.
pub(super) struct Wikidata;

impl MetadataProvider for Wikidata {
    const SOURCE: Provenance = Provenance {
        provider: super::LIVE,
        payload_version: 1,
    };

    fn search(&self, serial: &str, cancel: &AtomicBool) -> Result<SourceResponse, ProviderError> {
        let territory = territory(serial).map_err(ProviderError::Invalid)?;
        let _permit = REQUESTS.acquire_typed(cancel)?;
        let response = query_source(transport::api()?, ENDPOINT, serial, &territory, cancel)?;
        if crate::library::http_retry::should_defer(response.status(), response.headers()) {
            REQUESTS.defer(response.headers());
        }
        decode_response(response, serial, &territory)
    }
}

fn query_source(
    client: &Client,
    endpoint: &str,
    serial: &str,
    territory: &Territory,
    cancel: &AtomicBool,
) -> Result<http::Response<Vec<u8>>, ProviderError> {
    let request = client
        .get(endpoint)
        .query(&[
            ("query", query(serial, territory).as_str()),
            ("format", "json"),
        ])
        .header("Accept", "application/sparql-results+json");
    transport::request(request, MAX_RESPONSE, cancel)
}

#[cfg(feature = "ui-tests")]
pub(super) fn recorded_response(
    response: http::Response<Vec<u8>>,
    serial: &str,
) -> Result<SourceResponse, ProviderError> {
    let territory = territory(serial).map_err(ProviderError::Invalid)?;
    decode_response(response, serial, &territory)
}

fn decode_response(
    response: http::Response<Vec<u8>>,
    serial: &str,
    territory: &Territory,
) -> Result<SourceResponse, ProviderError> {
    if !response.status().is_success() {
        return Err(ProviderError::http(response.status(), response.headers()));
    }
    let raw = response.into_body();
    let releases = parse(&raw, serial, territory.region).map_err(ProviderError::Invalid)?;
    Ok(SourceResponse {
        source: Wikidata::SOURCE,
        releases,
        raw,
    })
}

#[cfg(test)]
fn fetch(serial: &str) -> Result<SourceResponse, String> {
    Wikidata
        .search(serial, &AtomicBool::new(false))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_to_valid_identifiers_before_network_access() {
        for invalid in [
            "SCUS-94900; DROP",
            "PS2",
            "SCPS-45001",
            "SLES-12３45",
            "/tmp/test.iso",
        ] {
            assert!(territory(invalid).is_err(), "{invalid}");
        }
        let t = territory("SCUS-94900").unwrap();
        assert_eq!(t.region, "USA");
        let text = query("SCUS-94900", &t);
        assert!(text.contains("wdt:P2606 \"SCUS-94900\""));
        assert!(!text.contains("$SERIAL"));
    }

    #[test]
    fn preserves_source_precision_and_conflicting_editions() {
        fn bind(value: &str) -> serde_json::Value {
            json!({"value":value})
        }
        let mut a = json!({"game":bind("http://www.wikidata.org/entity/Q1"),
                          "title":bind("Example"),"alias":bind("Demo"),"local":bind("例"),
                          "date":bind("1999-01-01T00:00:00Z"),"precision":bind("9")});
        let mut rows = vec![a.clone()];
        a["date"] = bind("2000-02-01T00:00:00Z");
        a["precision"] = bind("10");
        rows.push(a);
        let result = parse(
            json!({"results":{"bindings":rows}}).to_string().as_bytes(),
            "SLPS-00001",
            "Japan",
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].date, "1999");
        assert_eq!(result[1].id, "Q1/SLPS-00001/2000-02");
        assert_eq!(result[0].title, "例");
        assert_eq!(result[0].aliases, "Demo|Example");
        assert!(parse(b"{}", "SLUS-12345", "USA").is_err());
        assert!(
            parse(br#"{"results":{"bindings":[]}}"#, "SLUS-12345", "USA")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    #[ignore = "requires the public Wikidata Query Service"]
    fn live_wikidata_serial_metadata() {
        let SourceResponse { releases, raw, .. } = fetch("SCUS-94900").unwrap();
        let response: Response = serde_json::from_slice(&raw).unwrap();
        assert!(
            response
                .results
                .bindings
                .iter()
                .any(|row| row.title.value == "Crash Bandicoot")
        );
        let crash = releases
            .iter()
            .find(|r| r.id == "Q719176/SCUS-94900")
            .unwrap();
        assert_eq!(crash.title, "Crash Bandicoot");
        assert_eq!(crash.date, "1996-09-09");
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::atomic::Ordering,
        thread,
    };

    fn serve(
        status: u16,
        headers: &str,
        body: &str,
        delay: Duration,
    ) -> Result<SourceResponse, ProviderError> {
        let listen = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/sparql", listen.local_addr().unwrap());
        let client = Client::builder()
            .no_proxy()
            .timeout(if delay.is_zero() {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(150)
            })
            .build()
            .unwrap();
        let response = format!(
            "HTTP/1.1 {status} test\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        thread::scope(|scope| {
            scope.spawn(|| {
                let (mut socket, _) = listen.accept().unwrap();
                let mut input = BufReader::new(&socket);
                let mut request = String::new();
                input.read_line(&mut request).unwrap();
                assert!(request.starts_with("GET /sparql?query="));
                assert!(request.contains("SCUS-94900"));
                loop {
                    request.clear();
                    if input.read_line(&mut request).unwrap() == 0 || request == "\r\n" {
                        break;
                    }
                }
                thread::sleep(delay);
                let _ = socket.write_all(response.as_bytes());
            });
            let serial = "SCUS-94900";
            let territory = territory(serial).unwrap();
            let response = query_source(
                &client,
                &endpoint,
                serial,
                &territory,
                &AtomicBool::new(false),
            )?;
            decode_response(response, serial, &territory)
        })
    }

    // Some build sandboxes disable loopback sockets; this exercises the actual HTTP
    // client and parser without any upstream data or credentials.
    #[test]
    #[ignore = "requires a local loopback socket"]
    fn mock_wikidata_errors_and_cancellation() {
        for (status, code, retry) in [
            (404, "not_found", false),
            (400, "http", false),
            (429, "rate_limited", true),
            (503, "upstream", true),
        ] {
            let error = serve(
                status,
                "Retry-After: 120\r\n",
                "sensitive-service-body",
                Duration::ZERO,
            )
            .unwrap_err();
            assert_eq!(error.code(), code);
            assert_eq!(error.retryable(), retry);
            assert!(!error.to_string().contains("sensitive"));
            if status == 429 {
                assert!(
                    error.retry_after().unwrap()
                        > crate::library::http_retry::seconds(std::time::SystemTime::now())
                );
            }
        }
        for invalid in ["not JSON", r#"{"results":{"bindings":[{}]}}"#] {
            assert_eq!(
                serve(200, "", invalid, Duration::ZERO).unwrap_err().code(),
                "invalid_data"
            );
        }
        assert!(
            serve(200, "", r#"{"results":{"bindings":[]}}"#, Duration::ZERO)
                .unwrap()
                .releases
                .is_empty()
        );
        assert_eq!(
            serve(200, "", "", Duration::from_millis(350)).unwrap_err(),
            ProviderError::Timeout
        );

        let listen = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listen.local_addr().unwrap());
        let cancel = AtomicBool::new(false);
        let client = Client::builder().no_proxy().build().unwrap();
        thread::scope(|scope| {
            let (closed, observed) = std::sync::mpsc::channel();
            let flag = &cancel;
            scope.spawn(move || {
                let (mut stream, _) = listen.accept().unwrap();
                let mut request = BufReader::new(&stream);
                let mut line = String::new();
                loop {
                    line.clear();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                    .unwrap();
                flag.store(true, Ordering::Relaxed);
                observed.recv_timeout(Duration::from_secs(2)).unwrap();
            });
            assert_eq!(
                transport::request(client.get(&url), 200, &cancel).unwrap_err(),
                ProviderError::Cancelled
            );
            closed.send(()).unwrap();
        });
    }
}
