//! Shared HTTPS connection pools and size-bounded, cancellable provider requests.
use std::{
    fmt,
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use reqwest::{Client, RequestBuilder, StatusCode, header, redirect::Policy};

use crate::library::http_retry::{RequestLimit, retry_deadline, seconds};

/// Failure categories used by the persistent job queue. No response body, URL or
/// credential is attached to an HTTP or transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderError {
    Cancelled,
    Timeout,
    Transport,
    Http { status: u16, retry_at: Option<i64> },
    Invalid(String),
    Database(String),
}

impl ProviderError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Transport => "network",
            Self::Http { status: 429, .. } => "rate_limited",
            Self::Http {
                status: 401 | 403, ..
            } => "credentials",
            Self::Http {
                status: 404 | 410, ..
            } => "not_found",
            Self::Http {
                status: 500..=599, ..
            } => "upstream",
            Self::Http { .. } => "http",
            Self::Invalid(_) => "invalid_data",
            Self::Database(_) => "database",
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport
                | Self::Timeout
                | Self::Database(_)
                | Self::Http {
                    status: 408 | 429 | 500..=599,
                    ..
                }
        )
    }

    pub(crate) fn retry_after(&self) -> Option<i64> {
        match self {
            Self::Http { retry_at, .. } => *retry_at,
            _ => None,
        }
    }

    pub(crate) fn http(status: StatusCode, headers: &header::HeaderMap) -> Self {
        let after = headers
            .get(header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok());
        let now = seconds(SystemTime::now());
        let retry_at = (status == StatusCode::TOO_MANY_REQUESTS || after.is_some())
            .then(|| retry_deadline(after, now));
        Self::Http {
            status: status.as_u16(),
            retry_at,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Cancelled"),
            Self::Timeout => f.write_str("Provider request timed out"),
            Self::Transport => f.write_str("Provider network request failed"),
            Self::Http { status, .. } => write!(f, "Provider returned HTTP {status}"),
            Self::Invalid(message) | Self::Database(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<rusqlite::Error> for ProviderError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

impl From<reqwest::Error> for ProviderError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else {
            Self::Transport
        }
    }
}

fn build(redirect: Policy) -> Result<Client, ProviderError> {
    Client::builder()
        .https_only(true)
        .referer(false)
        .redirect(redirect)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(25))
        .user_agent(concat!(
            "Eidoryn/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/Opynicus/Eidoryn)"
        ))
        .build()
        .map_err(Into::into)
}

// Credentials are used only with the fixed, no-redirect API pool.
static API: LazyLock<Result<Client, ProviderError>> = LazyLock::new(|| build(Policy::none()));
static ART: LazyLock<Result<Client, ProviderError>> = LazyLock::new(|| build(Policy::limited(5)));

pub(crate) fn api() -> Result<&'static Client, ProviderError> {
    API.as_ref().map_err(Clone::clone)
}
pub(crate) fn art() -> Result<&'static Client, ProviderError> {
    ART.as_ref().map_err(Clone::clone)
}

static METADATA_REQUESTS: RequestLimit = RequestLimit::new(6, Duration::ZERO);
static COVER_REQUESTS: RequestLimit = RequestLimit::new(3, Duration::ZERO);

pub(crate) fn configure(metadata_workers: u8, cover_workers: u8) {
    METADATA_REQUESTS.resize(metadata_workers);
    COVER_REQUESTS.resize(cover_workers);
}

pub(crate) fn cover(
    builder: RequestBuilder,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<http::Response<Vec<u8>>, ProviderError> {
    let _permit = COVER_REQUESTS.acquire_typed(cancel)?;
    request_inner(builder, limit, cancel)
}

pub(crate) fn request(
    builder: RequestBuilder,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<http::Response<Vec<u8>>, ProviderError> {
    let _permit = METADATA_REQUESTS.acquire_typed(cancel)?;
    request_inner(builder, limit, cancel)
}

/// Socket reads run on the same Tokio runtime as the database pool. Cancellation
/// drops the in-flight future, including a stalled connect or response body.
fn request_inner(
    builder: RequestBuilder,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<http::Response<Vec<u8>>, ProviderError> {
    crate::library::runtime()
        .map_err(ProviderError::Database)?
        .block_on(async {
            let mut reading = std::pin::pin!(receive(builder, limit));
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ProviderError::Cancelled);
                }
                if let Ok(done) =
                    tokio::time::timeout(Duration::from_millis(75), reading.as_mut()).await
                {
                    #[cfg(test)]
                    sample::record(done.as_ref().is_ok_and(|r| r.status().is_success()));
                    return if cancel.load(Ordering::Relaxed) {
                        Err(ProviderError::Cancelled)
                    } else {
                        done
                    };
                }
            }
        })
}

async fn receive(
    builder: RequestBuilder,
    limit: usize,
) -> Result<http::Response<Vec<u8>>, ProviderError> {
    let mut response = builder.send().await?;
    let status = response.status();
    let mut result = http::Response::default();
    *result.status_mut() = status;
    for name in [header::CONTENT_TYPE, header::RETRY_AFTER] {
        if let Some(value) = response.headers().get(&name) {
            result.headers_mut().insert(name, value.clone());
        }
    }
    if status.is_success() {
        let oversized =
            || ProviderError::Invalid(format!("Provider response exceeds the {limit} byte limit"));
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(oversized());
        }
        let bytes: &mut Vec<u8> = result.body_mut();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > limit - bytes.len() {
                return Err(oversized());
            }
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok(result)
}

/// Process-wide observations for the opt-in end-to-end test. Run that test alone
/// to measure completed HTTP operations independently of other live tests.
#[cfg(test)]
pub(crate) mod sample {
    use std::sync::atomic::{AtomicU64, Ordering};

    static OK: AtomicU64 = AtomicU64::new(0);
    static FAILED: AtomicU64 = AtomicU64::new(0);

    pub(super) fn record(success: bool) {
        let counter = if success { &OK } else { &FAILED };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn totals() -> (u64, u64) {
        (OK.load(Ordering::Relaxed), FAILED.load(Ordering::Relaxed))
    }
}
