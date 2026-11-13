//! Host frame deadlines, independent of emulation, audio and snapshot publication.
use std::time::{Duration, Instant};

pub(super) struct Pacing {
    pub(super) deadline: Instant,
    pub(super) frame_deadline: Instant,
    pub(super) next_frame: u64,
}

impl Pacing {
    pub(super) fn new(now: Instant, frames: u64, fps: f64) -> Self {
        Self {
            deadline: now,
            frame_deadline: now + Duration::from_secs_f64(1.0 / fps),
            next_frame: frames + 1,
        }
    }

    pub(super) fn finish_batch(
        &mut self,
        now: Instant,
        frame_finished: bool,
        frames: u64,
        fps: f64,
    ) {
        self.deadline = now;
        if !frame_finished {
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
    fn unfinished_batches_run_immediately_without_advancing_the_frame_target() {
        let now = Instant::now();
        let mut pacing = Pacing::new(now, 7, 50.0);
        let next_batch = now + Duration::from_millis(4);
        pacing.finish_batch(next_batch, false, 7, 50.0);
        assert_eq!(pacing.deadline, next_batch);
        assert_eq!(pacing.frame_deadline, now + Duration::from_millis(20));
        assert_eq!(pacing.next_frame, 8);
    }

    #[test]
    fn completed_frames_wait_until_the_deadline_and_preserve_small_jitter() {
        let now = Instant::now();
        let mut pacing = Pacing::new(now, 7, 50.0);
        pacing.finish_batch(now + Duration::from_millis(4), true, 8, 50.0);
        assert_eq!(pacing.deadline, now + Duration::from_millis(20));
        assert_eq!(pacing.frame_deadline, now + Duration::from_millis(40));
        assert_eq!(pacing.next_frame, 9);
        pacing.finish_batch(now + Duration::from_millis(43), true, 9, 50.0);
        assert_eq!(pacing.deadline, now + Duration::from_millis(43));
        assert_eq!(pacing.frame_deadline, now + Duration::from_millis(60));
        assert_eq!(pacing.next_frame, 10);
    }

    #[test]
    fn only_backlogs_longer_than_one_frame_reset_the_schedule() {
        let now = Instant::now();
        for (late, expected_next) in [(40, 40), (41, 61)] {
            let mut pacing = Pacing::new(now, 0, 50.0);
            pacing.finish_batch(now + Duration::from_millis(late), true, 1, 50.0);
            assert_eq!(pacing.deadline, now + Duration::from_millis(late));
            assert_eq!(
                pacing.frame_deadline,
                now + Duration::from_millis(expected_next)
            );
        }
    }

    #[test]
    fn each_completed_frame_uses_the_current_video_rate() {
        let now = Instant::now();
        let mut pacing = Pacing::new(now, 0, 50.0);
        pacing.finish_batch(now, true, 1, 60.0);
        assert_eq!(pacing.deadline, now + Duration::from_millis(20));
        assert_eq!(
            pacing.frame_deadline,
            now + Duration::from_millis(20) + Duration::from_secs_f64(1.0 / 60.0)
        );
    }
}
