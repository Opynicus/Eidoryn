//! Record every 15 simulated frames. Holding rewind consumes older checkpoints on host time.
use super::Worker;
use crate::systems::checkpoints::{MAX_REWIND_SECONDS, MIN_REWIND_SECONDS};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

const CAPTURE_FRAMES: u64 = 15;
const STEP: Duration = Duration::from_millis(100);
const MAX_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Rewind {
    history: VecDeque<(u64, Box<[u8]>)>,
    bytes: usize,
    frame: u64,
    resume: bool,
    next: Option<Instant>,
}

impl Rewind {
    pub(super) fn wait_duration(&self) -> Duration {
        self.next
            .map_or(STEP, |next| next.saturating_duration_since(Instant::now()))
    }

    fn trim(&mut self, seconds: u16, max_bytes: usize) {
        let frames = (f64::from(seconds) * eidoryn_gba::FRAME_RATE) as u64;
        while self.history.front().is_some_and(|(frame, _)| {
            self.bytes > max_bytes || self.frame.saturating_sub(*frame) > frames
        }) {
            self.bytes -= self.history.pop_front().expect("checked front").1.len();
        }
    }
}

impl Worker {
    fn publish_history(&mut self) {
        let ring = &self.rewind;
        self.snapshot.rewind_states = ring.history.len()
            - usize::from(
                ring.history
                    .back()
                    .is_some_and(|(frame, _)| *frame == ring.frame),
            );
        self.snapshot.rewind_seconds = ring.history.front().map_or(0., |(frame, _)| {
            ring.frame.saturating_sub(*frame) as f64 / eidoryn_gba::FRAME_RATE
        });
    }

    pub(super) fn set_rewind_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if self.snapshot.rewind_enabled != enabled {
            self.snapshot.rewind_enabled = enabled;
            self.reset_rewind()?;
        }
        Ok(())
    }

    pub(super) fn set_rewind_seconds(&mut self, seconds: u16) -> Result<(), String> {
        if !(MIN_REWIND_SECONDS..=MAX_REWIND_SECONDS).contains(&seconds) {
            return Err(format!(
                "Rewind duration must be between {MIN_REWIND_SECONDS} and {MAX_REWIND_SECONDS} seconds"
            ));
        }
        self.snapshot.rewind_max_seconds = seconds;
        self.rewind.trim(seconds, MAX_BYTES);
        self.publish_history();
        Ok(())
    }

    pub(super) fn reset_rewind(&mut self) -> Result<(), String> {
        self.rewind = Rewind::default();
        self.snapshot.rewinding = false;
        self.capture_rewind()
    }

    pub(super) fn completed_rewind_frame(&mut self) -> Result<(), String> {
        if !self.snapshot.rewind_enabled {
            return Ok(());
        }
        self.rewind.frame += 1;
        self.capture_rewind()
    }

    fn capture_rewind(&mut self) -> Result<(), String> {
        let ring = &mut self.rewind;
        if self.snapshot.rewind_enabled
            && let Some(cart) = &self.cartridge
            && ring
                .history
                .back()
                .is_none_or(|(frame, _)| ring.frame - frame >= CAPTURE_FRAMES)
        {
            let bytes = cart
                .machine
                .save_state()
                .map_err(|e| e.to_string())?
                .into_boxed_slice();
            ring.bytes += bytes.len();
            ring.history.push_back((ring.frame, bytes));
        }
        ring.trim(self.snapshot.rewind_max_seconds, MAX_BYTES);
        self.publish_history();
        Ok(())
    }

    pub(super) fn begin_rewind(&mut self) -> Result<(), String> {
        if self.snapshot.rewinding {
            return Ok(());
        }
        if !self.snapshot.rewind_enabled || self.snapshot.rewind_states == 0 {
            return Err("No rewind history available".into());
        }
        self.rewind.resume = self.snapshot.running;
        self.snapshot.running = false;
        self.snapshot.rewinding = true;
        self.reset_frame_rate();
        if let Some(tracker) = &mut self.tracker {
            tracker.sample(false);
        }
        self.rewind_one().map(|_| ())
    }

    pub(super) fn rewind_one(&mut self) -> Result<bool, String> {
        self.rewind.next = Some(Instant::now() + STEP);
        while let Some((frame, bytes)) = self.rewind.history.pop_back() {
            self.rewind.bytes -= bytes.len();
            if frame < self.rewind.frame {
                let cart = self.cartridge.as_mut().ok_or("Load a cartridge first")?;
                cart.machine.load_state(&bytes).map_err(|e| e.to_string())?;
                self.snapshot.rgb = Arc::from(cart.machine.pixels());
                self.rewind.frame = frame;
                self.publish_history();
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn end_rewind(&mut self) {
        if !self.snapshot.rewinding {
            return;
        }
        self.snapshot.rewinding = false;
        self.snapshot.running = self.rewind.resume;
        self.rewind.next = None;
        if let Some(cart) = &mut self.cartridge {
            cart.machine.set_buttons(self.held);
        }
        self.reset_frame_rate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_evicts_oldest_checkpoints_for_either_limit() {
        let mut ring = Rewind::default();
        for frame in [0, 15, 30, 45] {
            ring.history.push_back((frame, Box::new([0; 4])));
            ring.bytes += 4;
        }
        ring.frame = 45;
        ring.trim(5, 8);
        assert_eq!(ring.history.front().unwrap().0, 30);
        assert_eq!(ring.bytes, 8);
        ring.frame = 335;
        ring.trim(5, MAX_BYTES);
        assert_eq!(ring.history.front().unwrap().0, 45);
        assert_eq!(ring.bytes, 4);
    }
}
