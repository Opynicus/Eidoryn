//! Host frame deadlines, independent of device execution and wall-clock reads.
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

    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn frame_finished(&self, frames: u64) -> bool {
        frames >= self.next_frame
    }

    pub(super) fn finish_batch(&mut self, now: Instant, frames: u64, fps: f64, finished: bool) {
        self.deadline = now;
        if !finished {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfinished_batches_continue_immediately_and_completed_frames_wait() {
        let now = Instant::now();
        let interval = Duration::from_millis(20);
        let mut pacing = FramePacing::new(now, 10, 50.0);
        assert!(!pacing.frame_finished(10));
        assert!(pacing.frame_finished(11));
        assert!(pacing.frame_finished(12));
        let partial = now + Duration::from_millis(4);
        pacing.finish_batch(partial, 10, 50.0, false);
        assert_eq!(pacing.deadline(), partial);
        pacing.finish_batch(now + Duration::from_millis(8), 11, 50.0, true);
        assert_eq!(pacing.deadline(), now + interval);
        assert!(!pacing.frame_finished(11));
        assert!(pacing.frame_finished(12));
        pacing.finish_batch(now + interval, 12, 50.0, true);
        assert_eq!(pacing.deadline(), now + interval * 2);
    }

    #[test]
    fn small_delays_keep_cadence_but_long_backlogs_start_a_new_deadline() {
        let now = Instant::now();
        for (late_ms, next_ms) in [(25, 40), (40, 40), (41, 61), (100, 120)] {
            let mut pacing = FramePacing::new(now, 0, 50.0);
            let late = now + Duration::from_millis(late_ms);
            pacing.finish_batch(late, 1, 50.0, true);
            assert_eq!(pacing.deadline(), late);
            pacing.finish_batch(late, 2, 50.0, true);
            assert_eq!(pacing.deadline(), now + Duration::from_millis(next_ms));
        }
    }

    #[test]
    fn demo_yields_and_video_mode_changes_use_the_same_frame_schedule() {
        let now = Instant::now();
        let mut pacing = FramePacing::new(now, 0, 60.0);
        let first = Duration::from_secs_f64(1.0 / 60.0);
        // The demo can yield before the hardware has counted a VBlank.
        pacing.finish_batch(now, 0, 50.0, true);
        assert_eq!(pacing.deadline(), now + first);
        assert!(pacing.frame_finished(1));
        pacing.finish_batch(now + first, 1, 50.0, true);
        assert_eq!(pacing.deadline(), now + first + Duration::from_millis(20));
    }
}
