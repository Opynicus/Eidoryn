//! Bounded in-memory history, sampled on guest time and replayed on host time.
use super::Worker;
use crate::systems::ps1::{
    input::PadInput,
    runner::{DEFAULT_REWIND_SECONDS, MAX_REWIND_SECONDS, MIN_REWIND_SECONDS},
    timing::CPU_CLOCK_HZ,
};
use eidoryn_ps1::RewindState;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const SAVE_CYCLES: u64 = CPU_CLOCK_HZ as u64 / 4;
const STEP_TIME: Duration = Duration::from_millis(100);
const MAX_BYTES: usize = 256 * 1024 * 1024;

struct Checkpoint {
    state: RewindState,
    input: [u8; 69],
    cycles: u64,
    bytes: usize,
}

pub(super) struct ActiveRewind {
    resume: bool,
    pub(super) held: u16,
    next: Instant,
}

pub(super) struct RewindBuffer {
    pub(super) enabled: bool,
    pub(super) max_seconds: u16,
    pub(super) active: Option<ActiveRewind>,
    states: VecDeque<Checkpoint>,
    bytes: usize,
    last_cycles: Option<u64>,
}

impl Default for RewindBuffer {
    fn default() -> Self {
        Self {
            enabled: false,
            max_seconds: DEFAULT_REWIND_SECONDS,
            active: None,
            states: VecDeque::new(),
            bytes: 0,
            last_cycles: None,
        }
    }
}

impl RewindBuffer {
    fn history_cycles(&self) -> u64 {
        u64::from(CPU_CLOCK_HZ) * u64::from(self.max_seconds)
    }

    fn max_states(&self) -> usize {
        usize::from(self.max_seconds) * 4
    }

    fn trim(&mut self, cycles: u64) {
        while self.states.front().is_some_and(|oldest| {
            self.states.len() > self.max_states()
                || cycles.saturating_sub(oldest.cycles) > self.history_cycles()
        }) {
            self.bytes -= self.states.pop_front().expect("history has a front").bytes;
        }
    }

    pub(super) fn wait_duration(&self) -> Option<Duration> {
        self.active
            .as_ref()
            .map(|active| active.next.saturating_duration_since(Instant::now()))
    }

    pub(super) fn available(&self, cycles: u64) -> (usize, f64) {
        let count = self
            .states
            .iter()
            .filter(|state| state.cycles < cycles)
            .count();
        let seconds = self.states.front().map_or(0.0, |state| {
            cycles.saturating_sub(state.cycles) as f64 / f64::from(CPU_CLOCK_HZ)
        });
        (count, seconds)
    }

    fn clear(&mut self) {
        self.states.clear();
        self.bytes = 0;
        self.last_cycles = None;
    }

    fn push(
        &mut self,
        state: Checkpoint,
        max_states: usize,
        max_bytes: usize,
    ) -> Result<(), String> {
        if state.bytes > max_bytes {
            return Err("Rewind checkpoint exceeds its memory budget".into());
        }
        while self.states.front().is_some_and(|oldest| {
            self.states.len() >= max_states
                || self.bytes > max_bytes - state.bytes
                || state.cycles.saturating_sub(oldest.cycles) > self.history_cycles()
        }) {
            self.bytes -= self.states.pop_front().expect("history has a front").bytes;
        }
        self.last_cycles = Some(state.cycles);
        self.bytes += state.bytes;
        self.states.push_back(state);
        Ok(())
    }

    fn pop_before(&mut self, cycles: u64) -> Option<Checkpoint> {
        while let Some(state) = self.states.pop_back() {
            self.bytes -= state.bytes;
            if state.cycles < cycles {
                return Some(state);
            }
        }
        None
    }
}

impl Worker {
    pub(super) fn set_rewind_seconds(&mut self, seconds: u16) {
        if !(MIN_REWIND_SECONDS..=MAX_REWIND_SECONDS).contains(&seconds) {
            self.message = Some(format!(
                "Rewind duration must be between {MIN_REWIND_SECONDS} and {MAX_REWIND_SECONDS} seconds"
            ));
            self.success_message = None;
            return;
        }
        self.message = None;
        self.success_message = None;
        self.end_rewind();
        self.rewind.max_seconds = seconds;
        self.rewind.trim(self.machine.elapsed_cycles());
    }

    pub(super) fn reset_rewind(&mut self) {
        self.rewind.clear();
        self.capture_rewind();
    }

    pub(super) fn capture_rewind(&mut self) {
        let cycles = self.machine.elapsed_cycles();
        if !self.rewind.enabled || self.rewind.active.is_some() || self.machine.halted.is_some() {
            return;
        }
        self.rewind.trim(cycles);
        if self
            .rewind
            .last_cycles
            .is_some_and(|last| cycles.saturating_sub(last) < SAVE_CYCLES)
        {
            return;
        }
        let capture = self.machine.rewind_state().and_then(|state| {
            let bytes = state
                .memory_size()
                .saturating_add(size_of::<Checkpoint>() - size_of::<RewindState>());
            self.rewind.push(
                Checkpoint {
                    state,
                    input: self.input.save_state(),
                    cycles,
                    bytes,
                },
                self.rewind.max_states(),
                MAX_BYTES,
            )
        });
        if let Err(error) = capture {
            self.rewind.enabled = false;
            self.rewind.clear();
            self.message = Some(format!("Rewind recording stopped: {error}"));
            self.success_message = None;
        }
    }

    pub(super) fn begin_rewind(&mut self) {
        if self.rewind.active.is_some() {
            return;
        }
        if !self.rewind.enabled {
            self.message = Some("Enable rewind recording before rewinding".into());
            self.success_message = None;
            return;
        }
        if self.rewind.available(self.machine.elapsed_cycles()).0 == 0 {
            self.message = Some("No rewind history yet; play for a moment first".into());
            self.success_message = None;
            return;
        }
        self.rewind.active = Some(ActiveRewind {
            resume: self.running,
            held: self.input.held_buttons(),
            next: Instant::now(),
        });
        self.running = false;
        self.message = None;
        self.success_message = None;
        if let Some(audio) = &self.audio {
            audio.clear();
            audio.set_playing(false);
        }
        self.rewind_one();
    }

    pub(super) fn rewind_one(&mut self) -> bool {
        let Some(active) = &mut self.rewind.active else {
            return false;
        };
        active.next = Instant::now() + STEP_TIME;
        let Some(state) = self.rewind.pop_before(self.machine.elapsed_cycles()) else {
            return false;
        };
        self.input = PadInput::load_state(&state.input).expect("trusted in-memory pad state");
        self.machine = state.state.into_machine();
        self.store.invalidate();
        self.reset_pacing();
        true
    }

    pub(super) fn end_rewind(&mut self) {
        let Some(active) = self.rewind.active.take() else {
            return;
        };
        self.input
            .set_buttons(active.held, self.machine.elapsed_cycles());
        self.update_input();
        let restore = self
            .machine
            .set_cpu_mode(self.cpu_mode)
            .map_err(|error| format!("Cannot resume CPU after rewind: {error}"))
            .and_then(|()| {
                self.machine
                    .bus
                    .gpu
                    .set_renderer(self.renderer)
                    .map_err(|error| format!("Cannot resume renderer after rewind: {error}"))
            });
        match restore {
            Ok(()) => self.running = active.resume,
            Err(error) => {
                self.running = false;
                self.message = Some(error);
                self.success_message = None;
            }
        }
        if let Some(audio) = &self.audio {
            audio.clear();
            audio.set_playing(self.running);
        }
        self.reset_pacing();
        self.rewind.last_cycles = None;
        self.capture_rewind();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::ps1::{
        card_file::CardStore,
        gpu::RendererMode,
        machine::Machine,
        memory_card::MemoryCard,
        runner::{Command, Runner, Snapshot, SnapshotMode},
        state_file::StateStore,
    };
    use eidoryn_host::audio::AudioBuffer;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "eidoryn-rewind-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn checkpoint(cycles: u64) -> Checkpoint {
        let state = Machine::demo().rewind_state().unwrap();
        let bytes = state.memory_size();
        Checkpoint {
            state,
            input: PadInput::default().save_state(),
            cycles,
            bytes,
        }
    }

    fn wait_for_snapshot(runner: &Runner, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(snapshot) = runner.take_snapshot()
                && predicate(&snapshot)
            {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not publish expected state"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn recording_defaults_off_and_duration_changes_preserve_execution() {
        let mut worker = Worker::new(
            Machine::demo(),
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        assert!(!worker.rewind.enabled);
        assert_eq!(worker.rewind.max_seconds, DEFAULT_REWIND_SECONDS);
        worker.machine.bus.tick(1).unwrap();
        worker.capture_rewind();
        assert!(worker.rewind.states.is_empty());
        assert_eq!(worker.rewind.bytes, 0);
        worker.apply(Command::SetRewindSeconds(MAX_REWIND_SECONDS));
        assert_eq!(worker.rewind.max_seconds, MAX_REWIND_SECONDS);
        assert!(!worker.rewind.enabled);
        assert!(worker.rewind.states.is_empty());
        for seconds in [0, MIN_REWIND_SECONDS - 1, MAX_REWIND_SECONDS + 1, u16::MAX] {
            worker.apply(Command::SetRewindSeconds(seconds));
            assert!(worker.message.is_some());
            assert_eq!(worker.rewind.max_seconds, MAX_REWIND_SECONDS);
        }
        worker.apply(Command::SetRewindEnabled(true));
        for (running, seconds) in [(false, MIN_REWIND_SECONDS), (true, MAX_REWIND_SECONDS)] {
            worker.running = running;
            worker.machine.bus.tick(SAVE_CYCLES as u32).unwrap();
            worker.capture_rewind();
            worker.apply(Command::SetRewinding(true));
            assert!(worker.rewind.active.is_some());
            worker.apply(Command::SetRewindSeconds(seconds));
            assert!(worker.rewind.active.is_none());
            assert_eq!(worker.running, running);
            assert_eq!(worker.rewind.max_seconds, seconds);
            assert!(worker.rewind.enabled);
        }
        worker.apply(Command::SetRewindEnabled(false));
        worker.apply(Command::SetRewindSeconds(MIN_REWIND_SECONDS));
        let output = std::sync::Arc::new(std::sync::Mutex::new(None));
        assert!(worker.publish(&output, false));
        let snapshot = output.lock().unwrap().take().unwrap();
        assert!(!snapshot.rewind_enabled);
        assert_eq!(snapshot.rewind_max_seconds, MIN_REWIND_SECONDS);
        assert_eq!(snapshot.rewind_states, 0);
    }

    #[test]
    fn shortening_duration_discards_expired_points_and_updates_capacity() {
        let mut history = RewindBuffer::default();
        let second = u64::from(CPU_CLOCK_HZ);
        for cycles in [0, second, 6 * second] {
            history
                .push(checkpoint(cycles), history.max_states(), usize::MAX)
                .unwrap();
        }
        assert_eq!(history.states.len(), 3);
        let old_bytes = history.bytes;
        history.max_seconds = MIN_REWIND_SECONDS;
        history.trim(6 * second + 1);
        assert_eq!(history.states.len(), 1);
        assert_eq!(history.states.front().unwrap().cycles, 6 * second);
        assert!(history.bytes < old_bytes);
        assert_eq!(history.max_states(), usize::from(MIN_REWIND_SECONDS) * 4);
        history.max_seconds = MAX_REWIND_SECONDS;
        history.trim(6 * second + 1);
        assert_eq!(history.states.len(), 1);
        assert_eq!(history.max_states(), usize::from(MAX_REWIND_SECONDS) * 4);
    }

    #[test]
    fn paused_rewind_changes_flush_cards_without_another_command() {
        for finish in [
            Command::SetRewinding(false),
            Command::SetRewindEnabled(false),
            Command::SetRewindSeconds(MIN_REWIND_SECONDS),
        ] {
            let root = temporary("paused-flush");
            let card_path = root.join("card.mcd");
            let (store, cards) = CardStore::open([Some(card_path.clone()), None]).unwrap();
            let original = cards[0].as_ref().unwrap().bytes().to_vec();
            let mut machine = Machine::demo();
            machine.bus.sio.cards = cards;
            let runner = Runner::with_card_store(machine, false, store).unwrap();
            runner.send(Command::SetRewindEnabled(true)).unwrap();
            runner.send(Command::Slice).unwrap();
            let snapshot = wait_for_snapshot(&runner, |snapshot| snapshot.slices > 0);
            assert!(!snapshot.running);
            assert!(snapshot.rewind_states > 0);

            // Give the disk a distinct later value so committing the rewind is observable.
            let mut later = original.clone();
            later[1024] ^= 0xff;
            fs::write(&card_path, &later).unwrap();
            runner.send(Command::SetRewinding(true)).unwrap();
            let snapshot = wait_for_snapshot(&runner, |snapshot| snapshot.rewinding);
            assert!(snapshot.cards[0].as_ref().unwrap().pending);
            assert_eq!(fs::read(&card_path).unwrap(), later);

            runner.send(finish).unwrap();
            let snapshot = wait_for_snapshot(&runner, |snapshot| !snapshot.rewinding);
            assert!(!snapshot.running);
            assert!(!snapshot.cards[0].as_ref().unwrap().pending);
            assert_eq!(fs::read(&card_path).unwrap(), original);
            drop(runner);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn history_evicts_by_count_memory_and_guest_age() {
        let mut history = RewindBuffer::default();
        let max_states = history.max_states();
        let history_cycles = history.history_cycles();
        let first = checkpoint(0);
        let bytes = first.bytes;
        history.push(first, 2, usize::MAX).unwrap();
        history.push(checkpoint(1), 2, usize::MAX).unwrap();
        history.push(checkpoint(2), 2, usize::MAX).unwrap();
        assert_eq!(history.states.len(), 2);
        assert_eq!(history.states.front().unwrap().cycles, 1);
        history.push(checkpoint(3), max_states, bytes).unwrap();
        assert_eq!(history.states.len(), 1);
        assert_eq!(history.bytes, bytes);
        assert!(history.push(checkpoint(4), max_states, bytes - 1).is_err());
        assert_eq!(history.states.back().unwrap().cycles, 3);
        history
            .push(checkpoint(history_cycles + 4), max_states, usize::MAX)
            .unwrap();
        assert_eq!(history.states.len(), 1);
        assert!(history.pop_before(history_cycles + 4).is_none());
        assert_eq!(history.bytes, 0);
    }

    #[test]
    fn rewind_holds_the_oldest_state_and_defers_card_writes_until_release() {
        let root = temporary("cards");
        let card_path = root.join("card.mcd");
        let (store, cards) = CardStore::open([Some(card_path.clone()), None]).unwrap();
        let initial_card_byte = cards[0].as_ref().unwrap().bytes()[1024];
        let mut machine = Machine::demo();
        machine.bus.sio.cards = cards;
        machine
            .bus
            .gpu
            .set_renderer(RendererMode::Threaded)
            .unwrap();
        let audio = AudioBuffer::default();
        let mut worker = Worker::new(
            machine,
            true,
            store,
            Some(audio.clone()),
            SnapshotMode::Display,
        );
        worker.apply(Command::SetRewindEnabled(true));
        worker.apply(Command::SetRewinding(true));
        assert!(worker.rewind.active.is_none());
        assert!(worker.running);
        worker.apply(Command::SetButtons(0x8000));
        for marker in 1..=3 {
            worker.machine.cpu.registers[7] = marker;
            let mut bytes = worker.machine.bus.sio.cards[0]
                .as_ref()
                .unwrap()
                .bytes()
                .to_vec();
            bytes[1024] = marker as u8;
            worker.machine.bus.sio.cards[0] = Some(MemoryCard::from_bytes(bytes).unwrap());
            worker.store.invalidate();
            assert!(worker.flush_cards(true));
            worker.machine.bus.tick(SAVE_CYCLES as u32).unwrap();
            worker.capture_rewind();
        }
        worker.apply(Command::SetButtons(0x4000));
        audio.set_playing(true);
        audio.push(&[[100, 100]; 8]);
        assert_eq!(audio.stats().queued_frames, 8);
        worker.apply(Command::SetRewinding(true));
        assert!(!worker.running);
        assert_eq!(worker.machine.cpu.registers[7], 2);
        assert_eq!(worker.machine.bus.gpu.renderer_mode(), RendererMode::Serial);
        worker.publish_audio();
        assert_eq!(audio.stats().queued_frames, 0);
        assert!(worker.rewind.wait_duration().is_some());
        worker.apply(Command::SetButtons(0));
        assert_eq!(worker.input.held_buttons(), 0x8000);
        assert!(worker.flush_cards(true));
        assert_eq!(fs::read(&card_path).unwrap()[1024], 3);
        while worker.rewind_one() {}
        let oldest = worker.machine.elapsed_cycles();
        assert_eq!(oldest, 0);
        assert!(worker.rewind.active.is_some());
        assert!(!worker.rewind_one());
        assert_eq!(worker.machine.elapsed_cycles(), oldest);
        let output = std::sync::Arc::new(std::sync::Mutex::new(None));
        assert!(worker.publish(&output, false));
        let snapshot = output.lock().unwrap().take().unwrap();
        assert!(snapshot.rewinding);
        assert_eq!(snapshot.rewind_states, 0);
        assert_eq!(snapshot.renderer, RendererMode::Threaded);
        worker.apply(Command::SetRewinding(false));
        assert!(worker.running);
        assert_eq!(worker.input.held_buttons(), 0);
        assert_eq!(
            worker.machine.bus.gpu.renderer_mode(),
            RendererMode::Threaded
        );
        audio.push(&[[100, 100]; 8]);
        assert_eq!(audio.stats().queued_frames, 8);
        assert!(worker.flush_cards(true));
        assert_eq!(fs::read(&card_path).unwrap()[1024], initial_card_byte);

        worker.machine.bus.tick(SAVE_CYCLES as u32).unwrap();
        worker.capture_rewind();
        worker.running = false;
        worker.apply(Command::SetRewinding(true));
        worker.apply(Command::SetRewinding(false));
        assert!(!worker.running);
        audio.push(&[[100, 100]; 8]);
        assert_eq!(audio.stats().queued_frames, 0);
        drop(worker);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sampling_uses_guest_time_and_successful_jumps_clear_history() {
        let root = temporary("jumps");
        let mut worker = Worker::new(
            Machine::demo(),
            false,
            CardStore::default(),
            None,
            SnapshotMode::Display,
        );
        worker.apply(Command::SetRewindEnabled(true));
        worker.states = StateStore::new(&root, &worker.machine);
        assert_eq!(worker.rewind.states.len(), 1);
        worker.machine.bus.tick((SAVE_CYCLES - 1) as u32).unwrap();
        worker.capture_rewind();
        assert_eq!(worker.rewind.states.len(), 1);
        worker.machine.bus.tick(1).unwrap();
        worker.capture_rewind();
        assert_eq!(worker.rewind.states.len(), 2);
        worker.apply(Command::QuickSave);
        worker.apply(Command::LoadState {
            path: root.join("missing.ps1state"),
        });
        assert_eq!(worker.rewind.states.len(), 2);
        worker.apply(Command::Load {
            path: root.join("missing.exe"),
            bios: false,
        });
        assert_eq!(worker.rewind.states.len(), 2);
        worker.apply(Command::QuickLoad);
        assert_eq!(worker.rewind.states.len(), 1);
        assert_eq!(
            worker.rewind.available(worker.machine.elapsed_cycles()).0,
            0
        );
        worker.machine.bus.tick(SAVE_CYCLES as u32).unwrap();
        worker.capture_rewind();
        worker.apply(Command::Reset);
        assert_eq!(worker.rewind.states.len(), 1);
        assert_eq!(worker.machine.elapsed_cycles(), 0);
        worker.machine.bus.tick(SAVE_CYCLES as u32).unwrap();
        worker.capture_rewind();
        let mut other = Machine::demo();
        other.title = "Other game".into();
        assert!(worker.replace_machine(other));
        assert_eq!(worker.rewind.states.len(), 1);
        worker.apply(Command::SetRewindEnabled(false));
        assert_eq!(worker.rewind.bytes, 0);
        assert!(worker.rewind.states.is_empty());
        worker.apply(Command::SetRewindEnabled(true));
        assert_eq!(worker.rewind.states.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
