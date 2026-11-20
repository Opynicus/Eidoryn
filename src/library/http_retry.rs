//! HTTP Retry-After deadlines for the metadata and artwork services.
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const RETRY_SECONDS: i64 = 300;

pub(super) fn seconds(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

pub(super) fn retry_deadline(header: Option<&str>, now: i64) -> i64 {
    header
        .and_then(|h| {
            h.parse::<u64>()
                .ok()
                .map(|s| now.saturating_add(i64::try_from(s).unwrap_or(i64::MAX)))
                .or_else(|| httpdate::parse_http_date(h).ok().map(seconds))
        })
        .unwrap_or_else(|| now.saturating_add(RETRY_SECONDS))
        .max(now)
}
