//! Request limits and HTTP Retry-After deadlines for metadata and artwork services.
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bound in-flight requests and the time between starts independently of game worker count.
pub(super) struct RequestLimit {
    max_active: usize,
    spacing: Duration,
    state: Mutex<RequestState>,
    released: Condvar,
}

struct RequestState {
    active: usize,
    last_start: Option<Instant>,
    retry_after: i64,
}

impl RequestLimit {
    pub(super) const fn new(max_active: usize, spacing: Duration) -> Self {
        Self {
            max_active,
            spacing,
            state: Mutex::new(RequestState {
                active: 0,
                last_start: None,
                retry_after: 0,
            }),
            released: Condvar::new(),
        }
    }

    pub(super) fn acquire(&self, cancel: &AtomicBool) -> Result<RequestPermit<'_>, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            if state.retry_after > seconds(SystemTime::now()) {
                return Err("Service rate limit reached. Retry later.".into());
            }
            let remaining = state.last_start.map_or(Duration::ZERO, |last| {
                self.spacing.saturating_sub(last.elapsed())
            });
            if state.active < self.max_active && remaining.is_zero() {
                state.active += 1;
                state.last_start = Some(Instant::now());
                return Ok(RequestPermit(self));
            }
            // Cancellation is signalled independently of the service's in-flight requests.
            let poll_cancel = Duration::from_millis(200);
            let wait = if state.active == self.max_active {
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

    pub(super) fn defer(&self, response: &reqwest::blocking::Response) {
        let after = response
            .headers()
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
            .body(Vec::new())
            .unwrap()
            .into();
        limit.defer(&response);
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
