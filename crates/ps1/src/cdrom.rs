//! Original CD-ROM data and audio controller with approximate command latency.
//! https://psx-spx.consoledev.net/cdromdrive/
//! Command latency is approximate; sector cadence uses 33,868,800 / 75 or / 150 clocks.
use crate::{
    cd_audio::CdAudio,
    disc::{DiscImage, DiscRegion, TrackKind, invalid, msf},
    spu::StereoFrame,
    timing::CPU_CLOCK_HZ,
};
use std::collections::VecDeque;

pub const COMMAND_CLOCKS: u32 = 2000;
pub const SECTOR_CLOCKS: u32 = CPU_CLOCK_HZ / 75;

#[derive(Clone)]
struct Command {
    opcode: u8,
    parameters: Vec<u8>,
    clocks: u32,
}
#[derive(Clone)]
struct Response {
    interrupt: u8,
    bytes: Vec<u8>,
}

#[derive(Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // Independent drive/status bits, not exclusive states.
pub struct CdRom {
    disc: Option<DiscImage>,
    region: Option<DiscRegion>,
    index: u8,
    parameters: Vec<u8>,
    response: VecDeque<u8>,
    irq_enable: u8,
    irq_flags: u8,
    command: Option<Command>,
    completion: Option<(u32, Response)>,
    play_completion: Option<Response>,
    mode: u8,
    filter: [u8; 2],
    location: u32,
    reported_location: u32,
    target: Option<u32>,
    motor: bool,
    reading: bool,
    playing: bool,
    play_track: u8,
    audio: CdAudio,
    muted: bool,
    read_clocks: u32,
    // Retained capacity; an empty buffer means no staged data sector.
    sector: Vec<u8>,
    data: VecDeque<u8>,
    last_header: Option<[u8; 8]>,
    trace: Option<VecDeque<String>>,
    pub sectors_read: u64,
    pub cdda_sectors: u64,
    pub xa_sectors: u64,
    pub filtered_sectors: u64,
    pub commands: u64,
    pub command_errors: u64,
    pub last_error_code: Option<u8>,
    pub last_command: Option<u8>,
    pub last_error: Option<String>,
}

impl CdRom {
    /// Opt-in, bounded diagnostic events; normal emulation does not format logs.
    pub fn enable_trace(&mut self, enabled: bool) {
        self.trace = enabled.then(VecDeque::new);
    }
    pub fn take_trace(&mut self) -> Vec<String> {
        self.trace
            .as_mut()
            .map_or_else(Vec::new, |trace| trace.drain(..).collect())
    }
    pub fn debug_state(&self) -> String {
        format!(
            "stat={:02x} mode={:02x} filter={:02x?} irq={:02x}/{:02x} lba={} reported={} target={:?} clock={} sector={} data={} xa={} read={} play={} cmd={:?} done={:?} errors={} last={:?}",
            self.status(),
            self.mode,
            self.filter,
            self.irq_flags,
            self.irq_enable,
            self.location,
            self.reported_location,
            self.target,
            self.read_clocks,
            self.sector.len(),
            self.data.len(),
            self.audio.xa_active(),
            self.sectors_read,
            self.cdda_sectors,
            self.command.as_ref().map(|c| (c.opcode, c.clocks)),
            self.completion.as_ref().map(|(n, r)| (*n, r.interrupt)),
            self.command_errors,
            self.last_error
        )
    }
    fn trace_event(&mut self, event: std::fmt::Arguments<'_>) {
        if self.trace.is_some() {
            let message = format!("{event} | {}", self.debug_state());
            if let Some(trace) = &mut self.trace {
                if trace.len() == 256 {
                    trace.pop_front();
                }
                trace.push_back(message);
            }
        }
    }
    pub fn insert(&mut self, disc: DiscImage) {
        let region = disc.license_region();
        *self = Self {
            region: region.as_ref().ok().copied().flatten(),
            last_error: region.err().map(|e| e.to_string()),
            disc: Some(disc),
            motor: true,
            ..Self::default()
        };
    }
    pub fn mounted(&self) -> bool {
        self.disc.is_some()
    }
    pub fn muted(&self) -> bool {
        self.muted
    }
    pub fn track_count(&self) -> usize {
        self.disc.as_ref().map_or(0, |disc| disc.tracks().count())
    }
    pub fn irq_pending(&self) -> bool {
        self.irq_flags & self.irq_enable & 0x1f != 0
    }
    pub fn data_available(&self) -> usize {
        self.data.len()
    }
    pub fn read_data(&mut self) -> u8 {
        self.data.pop_front().unwrap_or(0)
    }

    pub fn audio_sample(&mut self) -> StereoFrame {
        self.audio.sample(self.muted)
    }

    pub fn read(&mut self, register: u32) -> u8 {
        match register & 3 {
            0 => {
                self.index
                    | (u8::from(self.audio.xa_active()) << 2)
                    | (u8::from(self.parameters.is_empty()) << 3)
                    | (u8::from(self.parameters.len() < 16) << 4)
                    | (u8::from(!self.response.is_empty()) << 5)
                    | (u8::from(!self.data.is_empty()) << 6)
                    | (u8::from(self.command.is_some()) << 7)
            }
            1 => self.response.pop_front().unwrap_or(0),
            2 => self.read_data(),
            _ => {
                0xe0 | if self.index & 1 == 0 {
                    self.irq_enable
                } else {
                    self.irq_flags
                }
            }
        }
    }

    pub fn write(&mut self, register: u32, value: u8) {
        if register & 3 == 0 {
            self.index = value & 3;
            return;
        }
        match (self.index, register & 3) {
            (0, 1) if self.command.is_none() => {
                self.command = Some(Command {
                    opcode: value,
                    parameters: std::mem::take(&mut self.parameters),
                    clocks: COMMAND_CLOCKS,
                });
            }
            (0, 2) if self.parameters.len() < 16 => self.parameters.push(value),
            (0, 3) => {
                if value & 0x80 == 0 {
                    self.data.clear();
                } else if self.data.is_empty() {
                    self.data.extend(self.sector.drain(..));
                }
            }
            (1, 2) => self.irq_enable = value & 0x1f,
            (1, 3) => {
                self.irq_flags &= !(value & 0x1f);
                if value & 0x20 != 0 {
                    self.audio.clear();
                }
                if self.irq_flags == 0 && value & 7 != 0 {
                    self.response.clear();
                }
                if value & 0x40 != 0 {
                    self.parameters.clear();
                }
                self.trace_event(format_args!("ack {value:02x}"));
            }
            _ => self.audio.write(self.index, register & 3, value),
        }
    }

    fn status(&self) -> u8 {
        (u8::from(self.motor) << 1)
            | (u8::from(self.disc.is_none()) << 4)
            | (u8::from(self.reading) << 5)
            | (u8::from(self.playing) << 7)
    }
    fn respond(&mut self, interrupt: u8, bytes: &[u8]) {
        self.trace_event(format_args!("response INT{interrupt} {bytes:02x?}"));
        self.response.clear();
        self.response.extend(bytes);
        self.irq_flags = interrupt;
    }
    fn error(&mut self, code: u8) {
        self.command_errors += 1;
        self.last_error_code = Some(code);
        self.respond(5, &[self.status() | 1, code]);
    }
    fn finish_later(&mut self, interrupt: u8, bytes: Vec<u8>) {
        self.completion = Some((COMMAND_CLOCKS * 2, Response { interrupt, bytes }));
    }

    #[inline]
    pub fn tick(&mut self, clocks: u32) {
        if self.reading || self.playing {
            self.read_clocks = self.read_clocks.saturating_sub(clocks);
        }
        if let Some(command) = &mut self.command {
            command.clocks = command.clocks.saturating_sub(clocks);
        }
        if let Some((remaining, _)) = &mut self.completion {
            *remaining = remaining.saturating_sub(clocks);
        }
        if self.irq_flags == 0 {
            if self.command.as_ref().is_some_and(|c| c.clocks == 0) {
                let command = self.command.take().expect("pending command");
                self.execute(command.opcode, &command.parameters);
            } else if self.completion.as_ref().is_some_and(|(n, _)| *n == 0) {
                let (_, response) = self.completion.take().expect("pending completion");
                self.respond(response.interrupt, &response.bytes);
            } else if let Some(response) = self.play_completion.take() {
                self.respond(response.interrupt, &response.bytes);
            }
        }
        if self.playing {
            if self.read_clocks == 0 {
                self.receive_audio_sector();
            }
            return;
        }
        if !self.reading {
            return;
        }
        // Bounded single-sector staging. Overrun mechanics and precise seek timing are not modeled.
        if self.read_clocks == 0 && self.irq_flags == 0 && self.sector.is_empty() {
            self.receive_sector();
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, opcode: u8, parameters: &[u8]) {
        self.trace_event(format_args!("command {opcode:02x} {parameters:02x?}"));
        self.commands += 1;
        self.last_command = Some(opcode);
        let expected = match opcode {
            2 => 3,
            0x0d => 2,
            0x0e | 0x14 | 0x19 => 1,
            _ => 0,
        };
        if if opcode == 3 {
            parameters.len() > 1
        } else {
            parameters.len() != expected
        } {
            self.error(0x20);
            return;
        }
        if self.disc.is_none() && !matches!(opcode, 1 | 0x0a | 0x0e | 0x0f | 0x19 | 0x1a) {
            self.error(0x80);
            return;
        }
        match opcode {
            1 => self.respond(3, &[self.status()]),
            2 => {
                let number = |b: u8| {
                    (b & 15 < 10 && b >> 4 < 10)
                        .then_some(u32::from(b >> 4) * 10 + u32::from(b & 15))
                };
                let position = number(parameters[0])
                    .zip(number(parameters[1]))
                    .zip(number(parameters[2]))
                    .filter(|((_, s), f)| *s < 60 && *f < 75)
                    .and_then(|((m, s), f)| (m * 4500 + s * 75 + f).checked_sub(150));
                if let Some(lba) = position {
                    self.target = Some(lba);
                    self.respond(3, &[self.status()]);
                } else {
                    self.error(0x10);
                }
            }
            3 => {
                let track = parameters.first().copied().unwrap_or(0);
                if track != 0 {
                    if track & 15 > 9 || track >> 4 > 9 {
                        self.error(0x10);
                        return;
                    }
                    let number = (track >> 4) * 10 + (track & 15);
                    let Some(start) = self.disc.as_ref().and_then(|disc| {
                        disc.tracks()
                            .find(|t| t.number == number)
                            .map(|t| t.start_lba)
                    }) else {
                        self.error(0x10);
                        return;
                    };
                    self.target = Some(start);
                }
                if let Some(target) = self.target.take() {
                    self.location = target;
                    self.reported_location = target;
                }
                self.play_track = self
                    .disc
                    .as_ref()
                    .and_then(|d| d.track_at(self.location))
                    .map_or(0, |t| t.number);
                self.audio.clear();
                self.play_completion = None;
                self.reading = false;
                self.playing = true;
                self.motor = true;
                self.read_clocks = SECTOR_CLOCKS;
                self.respond(3, &[self.status()]);
            }
            6 | 0x1b => {
                self.last_error = None;
                self.playing = false;
                self.play_completion = None;
                if let Some(target) = self.target.take() {
                    self.audio.clear();
                    self.location = target;
                    self.reported_location = target;
                }
                self.motor = true;
                self.respond(3, &[self.status()]);
                self.reading = true;
                self.read_clocks = if self.mode & 0x80 != 0 {
                    SECTOR_CLOCKS / 2
                } else {
                    SECTOR_CLOCKS
                };
            }
            7..=0x0a | 0x15 | 0x16 | 0x1e => {
                self.respond(3, &[self.status()]);
                self.reading = false;
                self.playing = false;
                self.play_completion = None;
                self.audio.clear();
                if opcode == 8 {
                    self.motor = false;
                }
                if matches!(opcode, 7 | 0x0a) {
                    self.motor = self.disc.is_some();
                }
                if opcode == 0x0a {
                    self.mode = 0x20;
                    self.muted = false;
                    self.filter = [0; 2];
                    self.sector.clear();
                    self.data.clear();
                }
                if matches!(opcode, 0x15 | 0x16)
                    && let Some(target) = self.target.take()
                {
                    self.location = target;
                    self.reported_location = target;
                }
                self.finish_later(2, vec![self.status()]);
            }
            0x0b | 0x0c => {
                self.muted = opcode == 0x0b;
                self.respond(3, &[self.status()]);
            }
            0x0d => {
                if self.filter != parameters {
                    self.audio.clear();
                }
                self.filter.copy_from_slice(parameters);
                self.respond(3, &[self.status()]);
            }
            0x0e => {
                self.mode = if parameters[0] & 0x10 != 0 {
                    (parameters[0] & !0x20) | (self.mode & 0x20)
                } else {
                    parameters[0]
                };
                self.respond(3, &[self.status()]);
            }
            0x0f => self.respond(
                3,
                &[self.status(), self.mode, 0, self.filter[0], self.filter[1]],
            ),
            0x10 => {
                if let Some(header) = self.last_header {
                    self.respond(3, &header);
                } else {
                    self.error(0x80);
                }
            }
            0x11 => {
                let position = self.reported_location;
                let Some(track) = self.disc.as_ref().and_then(|d| d.track_at(position)) else {
                    self.error(0x80);
                    return;
                };
                let mut response = [0; 8];
                response[0] = bcd(track.number);
                response[1] = u8::from(position >= track.start_lba);
                response[2..5].copy_from_slice(&msf(position.abs_diff(track.start_lba)));
                response[5..8].copy_from_slice(&msf(position + 150));
                self.respond(3, &response);
            }
            0x13 => self.respond(3, &[self.status(), 1, bcd(self.track_count() as u8)]),
            0x14 => {
                let disc = self
                    .disc
                    .as_ref()
                    .expect("disc command requires mounted media");
                let position = if parameters[0] == 0 {
                    Some(disc.sectors + 150)
                } else {
                    disc.tracks()
                        .find(|t| bcd(t.number) == parameters[0])
                        .map(|t| t.start_lba + 150)
                };
                let Some(position) = position else {
                    self.error(0x10);
                    return;
                };
                let time = msf(position);
                self.respond(3, &[self.status(), time[0], time[1]]);
            }
            0x19 if parameters == [0x20] => self.respond(3, &[0x95, 0x05, 0x16, 0xc1]),
            0x19 => self.error(0x10),
            0x1a => {
                self.respond(3, &[self.status()]);
                if let Some(region) = self.region {
                    self.finish_later(
                        2,
                        vec![
                            self.status(),
                            0,
                            0x20,
                            0,
                            b'S',
                            b'C',
                            b'E',
                            region.sce_letter(),
                        ],
                    );
                } else if let Some(disc) = &self.disc {
                    let audio = disc.tracks().any(|t| t.kind == TrackKind::Audio);
                    let mode2 = disc.tracks().any(|t| t.kind == TrackKind::Mode2);
                    self.finish_later(
                        5,
                        vec![
                            self.status() | 8,
                            0x80 | (u8::from(audio) << 4),
                            u8::from(mode2) << 5,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ],
                    );
                } else {
                    self.finish_later(5, vec![8, 0x40, 0, 0, 0, 0, 0, 0]);
                }
            }
            _ => self.error(0x40),
        }
    }

    fn receive_audio_sector(&mut self) {
        let track = self
            .disc
            .as_ref()
            .and_then(|disc| disc.track_at(self.location));
        let Some(track) = track else {
            self.playing = false;
            self.motor = false;
            self.play_completion = Some(Response {
                interrupt: 4,
                bytes: vec![self.status()],
            });
            return;
        };
        if self.mode & 2 != 0 && track.number != self.play_track {
            self.playing = false;
            self.play_completion = Some(Response {
                interrupt: 4,
                bytes: vec![self.status()],
            });
            return;
        }
        let result = self
            .disc
            .as_ref()
            .expect("playing disc")
            .read_sector(self.location);
        match result {
            Ok(raw) => {
                if track.kind == TrackKind::Audio {
                    self.audio.cdda(&raw);
                } else {
                    self.audio.silence_sector();
                }
                self.cdda_sectors += 1;
                self.reported_location = self.location;
                self.location += 1;
                self.read_clocks = if self.mode & 0x80 != 0 {
                    SECTOR_CLOCKS / 2
                } else {
                    SECTOR_CLOCKS
                };
                if self.mode & 4 != 0
                    && self.irq_flags == 0
                    && ((self.reported_location + 150) % 75).is_multiple_of(10)
                {
                    let absolute = msf(self.reported_location + 150);
                    let relative = msf(self.reported_location.saturating_sub(track.start_lba));
                    let time = if (absolute[2] & 0x10) == 0 {
                        absolute
                    } else {
                        [relative[0], relative[1] | 0x80, relative[2]]
                    };
                    self.respond(
                        1,
                        &[
                            self.status(),
                            bcd(track.number),
                            1,
                            time[0],
                            time[1],
                            time[2],
                            0,
                            0,
                        ],
                    );
                }
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.playing = false;
                self.command_errors += 1;
                self.last_error_code = Some(0x80);
                self.play_completion = Some(Response {
                    interrupt: 5,
                    bytes: vec![self.status() | 1, 0x80],
                });
            }
        }
    }

    fn receive_sector(&mut self) {
        let result = self
            .disc
            .as_ref()
            .expect("reading requires a disc")
            .read_sector(self.location)
            .and_then(|raw| {
                if self
                    .disc
                    .as_ref()
                    .and_then(|d| d.track_at(self.location))
                    .is_some_and(|t| t.kind == TrackKind::Audio)
                {
                    return Err(invalid("CD audio tracks require Play; ReadN/ReadS CDDA data mode is not implemented"));
                }
                Ok(raw)
            });
        match result {
            Ok(raw) => {
                self.reported_location = self.location;
                self.location += 1;
                self.sectors_read += 1;
                self.read_clocks = if self.mode & 0x80 != 0 {
                    SECTOR_CLOCKS / 2
                } else {
                    SECTOR_CLOCKS
                };
                // GetlocL reports the most recently decoded header, including
                // XA audio and filtered sectors that never raise a data IRQ.
                // Games save this position before Pause and seek back to it
                // when leaving menus; retaining an older data header sends
                // the resumed audio stream to an unrelated resource sector.
                self.last_header = Some(raw[12..20].try_into().expect("header bounds"));
                let audio = raw[15] == 2 && raw[18] & 0x44 == 0x44;
                if audio && self.mode & 0x48 != 0 {
                    if self.mode & 0x40 != 0 && (self.mode & 8 == 0 || raw[16..18] == self.filter) {
                        // Audio must not become a data IRQ or stop interleaved video reads.
                        self.xa_sectors += 1;
                        self.audio.xa(&raw);
                    } else {
                        self.filtered_sectors += 1;
                    }
                    return;
                }
                let data = if self.mode & 0x20 != 0 {
                    &raw[12..]
                } else {
                    let offset = if raw[15] == 2 { 24 } else { 16 };
                    // The controller can deliver Form 2 too; strict ISO file reads still reject it.
                    &raw[offset..offset + 2048]
                };
                self.sector.extend_from_slice(data);
                self.respond(1, &[self.status()]);
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.reading = false;
                self.error(0x80);
            }
        }
    }
}

fn bcd(value: u8) -> u8 {
    (value / 10) << 4 | (value % 10)
}
