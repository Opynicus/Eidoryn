//! Request limits and HTTP Retry-After deadlines for metadata and artwork services.
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::transport::ProviderError;

/// Bound in-flight requests and the time between starts independently of game worker count.
pub(super) struct RequestLimit {
    spacing: Duration,
    state: Mutex<RequestState>,
    released: Condvar,
}

struct RequestState {
    max_active: usize,
    active: usize,
    last_start: Option<Instant>,
    retry_after: i64,
}

impl RequestLimit {
    pub(super) const fn new(max_active: usize, spacing: Duration) -> Self {
        Self {
            spacing,
            state: Mutex::new(RequestState {
                max_active,
                active: 0,
                last_start: None,
                retry_after: 0,
            }),
            released: Condvar::new(),
        }
    }

    pub(super) fn resize(&self, maximum: u8) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.max_active = usize::from(maximum.max(1));
        self.released.notify_all();
    }

    pub(super) fn acquire(&self, cancel: &AtomicBool) -> Result<RequestPermit<'_>, String> {
        self.acquire_typed(cancel).map_err(|e| e.to_string())
    }

    pub(super) fn acquire_typed(
        &self,
        cancel: &AtomicBool,
    ) -> Result<RequestPermit<'_>, ProviderError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ProviderError::Cancelled);
            }
            if state.retry_after > seconds(SystemTime::now()) {
                return Err(ProviderError::Http {
                    status: 429,
                    retry_at: Some(state.retry_after),
                });
            }
            let remaining = state.last_start.map_or(Duration::ZERO, |last| {
                self.spacing.saturating_sub(last.elapsed())
            });
            if state.active < state.max_active && remaining.is_zero() {
                state.active += 1;
                state.last_start = Some(Instant::now());
                return Ok(RequestPermit(self));
            }
            // Cancellation is signalled independently of the service's in-flight requests.
            let poll_cancel = Duration::from_millis(200);
            let wait = if state.active >= state.max_active {
                poll_cancel
            } else {
                remaining.min(poll_cancel)
            };
            (state, _) = self
                .released
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(super) fn defer(&self, headers: &reqwest::header::HeaderMap) {
        let after = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|h| h.to_str().ok());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.retry_after = state
            .retry_after
            .max(retry_deadline(after, seconds(SystemTime::now())));
        self.released.notify_all();
    }
}

pub(super) struct RequestPermit<'a>(&'a RequestLimit);

impl Drop for RequestPermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        self.0.released.notify_one();
    }
}

pub(super) fn should_defer(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error() && headers.contains_key(reqwest::header::RETRY_AFTER)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_spacing_and_backoff_apply_across_requests() {
        let spacing = Duration::from_millis(10);
        let limit = RequestLimit::new(2, spacing);
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let first = limit.acquire(&cancel).unwrap();
        let second = limit.acquire(&cancel).unwrap();
        assert!(start.elapsed() >= spacing);
        drop((first, second));

        let response = http::Response::builder()
            .status(429)
            .header("Retry-After", "10")
            .body(())
            .unwrap();
        limit.defer(response.headers());
        assert!(limit.acquire(&cancel).is_err());
    }

    #[test]
    fn occupied_permit_can_be_cancelled_while_waiting() {
        let limit = RequestLimit::new(1, Duration::ZERO);
        let cancel = AtomicBool::new(false);
        let first = limit.acquire(&cancel).unwrap();
        let (waiting, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                waiting.send(()).unwrap();
                limit.acquire(&cancel).is_ok()
            });
            receiver.recv().unwrap();
            cancel.store(true, Ordering::Relaxed);
            assert!(!waiter.join().unwrap());
        });
        drop(first);
        assert!(limit.acquire(&AtomicBool::new(false)).is_ok());
    }
}
