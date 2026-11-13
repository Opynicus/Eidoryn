//! Frame deadlines driven by explicit host time and video progress.
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FramePacing {
    deadline: Instant,
    frame_deadline: Instant,
    next_frame: u64,
}

impl FramePacing {
    pub(super) fn new(now: Instant, frames: u64, fps: f64) -> Self {
        Self {
            deadline: now,
            frame_deadline: now + Duration::from_secs_f64(1.0 / fps),
            next_frame: frames + 1,
        }
    }

    pub(super) fn wait_duration(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }

    pub(super) fn frame_reached(&self, frames: u64) -> bool {
        frames >= self.next_frame
    }

    pub(super) fn finish_batch(&mut self, now: Instant, frames: u64, fps: f64, finished: bool) {
        self.deadline = now;
        if finished {
            // Preserve small scheduling jitter; discard a backlog longer than one frame.
            self.deadline = self.frame_deadline.max(now);
            let interval = Duration::from_secs_f64(1.0 / fps);
            self.frame_deadline = if now.saturating_duration_since(self.frame_deadline) > interval {
                now + interval
            } else {
                self.frame_deadline + interval
            };
            self.next_frame = frames + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_batches_continue_without_moving_the_frame_target() {
        let start = Instant::now();
        let mut pacing = FramePacing::new(start, 7, 50.0);
        assert_eq!(pacing.wait_duration(start), Duration::ZERO);
        let now = start + Duration::from_millis(5);
        pacing.finish_batch(now, 7, 50.0, false);
        assert_eq!(pacing.wait_duration(now), Duration::ZERO);
        assert!(!pacing.frame_reached(7));
        assert!(pacing.frame_reached(8));
        assert!(pacing.frame_reached(9));
        assert_eq!(pacing.frame_deadline, start + Duration::from_millis(20));
    }

    #[test]
    fn completed_frames_keep_small_jitter_and_use_the_current_frame_rate() {
        let start = Instant::now();
        for (elapsed_ms, fps, wait_ms, next_ms) in [
            (10, 50.0, 10, 40),
            (21, 50.0, 0, 40),
            (40, 50.0, 0, 40), // Exactly one frame late is not discarded.
            (41, 50.0, 0, 61),
            (10, 100.0, 10, 30), // A changed rate applies to the next deadline.
        ] {
            let mut pacing = FramePacing::new(start, 7, 50.0);
            let now = start + Duration::from_millis(elapsed_ms);
            pacing.finish_batch(now, 9, fps, true);
            assert_eq!(pacing.wait_duration(now), Duration::from_millis(wait_ms));
            assert_eq!(
                pacing.wait_duration(now + Duration::from_secs(1)),
                Duration::ZERO
            );
            assert_eq!(
                pacing.frame_deadline,
                start + Duration::from_millis(next_ms)
            );
            assert_eq!(pacing.next_frame, 10);
        }
    }
}
