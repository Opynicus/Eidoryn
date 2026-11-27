use crate::audio::Audio;
use crate::backup::Backup;
use crate::cpu::Bus;
use crate::video::{Video, half};
use crate::{BIOS_LEN, HEIGHT};

pub(super) const SCANLINE: u32 = 1232;
const DRAW_END: u32 = 960;
pub(super) const HBLANK: u32 = 1006;
pub(super) const LINES: u16 = 228;

const DMA_BASE: usize = 0xb0;
const TIMER_BASE: usize = 0x100;
const TIMER_DIVISORS: [u32; 4] = [1, 64, 256, 1024];

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Timer {
    count: u16,
    fraction: u32,
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Dma {
    src: u32,
    dst: u32,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct InstructionFetch {
    in_bios: bool,
    bios_latch: [u8; 4],
    cartridge_started: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Memory {
    #[serde(skip)]
    rom: Vec<u8>,
    #[serde(skip)]
    bios: Option<Box<[u8]>>,
    fetch: InstructionFetch,
    #[serde(with = "serde_bytes")]
    ewram: Box<[u8]>,
    #[serde(with = "serde_bytes")]
    iwram: Box<[u8]>,
    pub video: Video,
    #[serde(with = "serde_bytes")]
    io: [u8; 1024],
    pub(super) backup: Backup,
    #[serde(skip)] // Version 3 appends guest audio state after the cartridge clock.
    pub(super) audio: Audio,
    #[serde(skip)]
    pub(super) rtc: Option<crate::rtc::Rtc>,
    pub line: u16,
    pub cycle: u32,
    pub halted: bool,
    timers: [Timer; 4],
    dma: [Dma; 4],
    dma_busy: bool,
    enabled_timers: u8,
}

impl Memory {
    pub(super) fn valid_snapshot_for(&self, live: &Self) -> bool {
        self.ewram.len() == live.ewram.len()
            && self.iwram.len() == live.iwram.len()
            && self.video.vram.len() == live.video.vram.len()
            && self.video.palette.len() == live.video.palette.len()
            && self.video.oam.len() == live.video.oam.len()
            && self.line < LINES
            && self.cycle < SCANLINE
            && !self.dma_busy
            && self.timers.iter().enumerate().all(|(i, timer)| {
                let control = self.io[TIMER_BASE + 4 * i + 2];
                timer.fraction < TIMER_DIVISORS[usize::from(control & 3)]
                    && (self.enabled_timers & (1 << i) != 0) == (control & 0x80 != 0)
            })
            && self.backup.valid_for(&live.backup)
    }

    /// Older snapshots stored sound register writes, but had no timed sound engine.
    pub(super) fn restore_legacy_audio(&mut self, live: &Self) {
        // Direct boot in v1/v2 left the BIOS bias at zero. Games could set the PWM
        // resolution while keeping that missing midpoint, clipping negative PCM.
        if !live.has_bios() && half(&self.io, 0x88) & 0x03fe == 0 {
            self.io[0x89] |= 2;
        }
        for offset in 0x90..0xa0 {
            let byte = self.io[offset];
            self.audio.write(&mut self.io, offset, byte);
        }
        for offset in [0x65, 0x6d, 0x75, 0x7d] {
            let byte = self.io[offset];
            self.audio.write(&mut self.io, offset, byte);
        }
    }

    /// Attach the live cartridge, firmware and presentation backend after validation.
    pub(super) fn attach_session(&mut self, live: &mut Self) {
        self.rom = std::mem::take(&mut live.rom);
        self.bios = live.bios.take();
        self.backup.reconcile_dirty(&live.backup);
        #[cfg(feature = "gpu")]
        self.video.retain_renderer(&mut live.video);
        self.video.graphics_error = live.video.graphics_error.take();
    }

    pub fn new(rom: Vec<u8>) -> Self {
        let mut io = [0; 1024];
        io[0] = 0x80;
        io[0x89] = 2; // BIOS default PWM bias 0x200.
        io[4] = 4; // VCOUNT matches zero at startup.
        for addr in [0x20, 0x26, 0x30, 0x36] {
            io[addr + 1] = 1;
        }
        io[0x130..0x132].copy_from_slice(&0x3ff_u16.to_le_bytes());
        io[0x300] = 1; // Direct cartridge entry occurs after POST.
        let backup = Backup::detect(&rom);
        let rtc = crate::rtc::Rtc::detect(&rom);
        Self {
            rom,
            bios: None,
            fetch: InstructionFetch::default(),
            ewram: vec![0; 256 * 1024].into_boxed_slice(),
            iwram: vec![0; 32 * 1024].into_boxed_slice(),
            video: Video::new(),
            io,
            backup,
            audio: Audio::default(),
            rtc,
            line: 0,
            cycle: 0,
            halted: false,
            timers: [Timer::default(); 4],
            dma: [Dma::default(); 4],
            dma_busy: false,
            enabled_timers: 0,
        }
    }

    pub fn install_bios(&mut self, image: Box<[u8]>) {
        self.bios = Some(image);
        self.io[0x300] = 0; // The firmware sets POSTFLG after a cold boot.
    }

    // Inspect firmware directly even when CPU data reads see the protected prefetch latch.
    fn bios_bytes<const N: usize>(&self, aligned: u32) -> Option<[u8; N]> {
        self.bios
            .as_ref()?
            .get(aligned as usize..)?
            .first_chunk::<N>()
            .copied()
    }

    pub fn peek16(&self, addr: u32) -> u16 {
        self.bios_bytes(addr & !1)
            .map_or_else(|| self.read16(addr), u16::from_le_bytes)
    }

    pub fn peek32(&self, addr: u32) -> u32 {
        self.bios_bytes(addr & !3)
            .map_or_else(|| self.read32(addr), u32::from_le_bytes)
    }

    pub fn has_bios(&self) -> bool {
        self.bios.is_some()
    }

    pub fn cartridge_started(&self) -> bool {
        self.fetch.cartridge_started
    }

    pub fn reset(&mut self) {
        let mut fresh = Self::new(std::mem::take(&mut self.rom));
        if let Some(bios) = self.bios.take() {
            fresh.install_bios(bios);
        }
        if let Some(rtc) = &mut self.rtc {
            rtc.reset_bus();
        }
        std::mem::swap(&mut self.rtc, &mut fresh.rtc);
        self.backup.reset_protocol();
        std::mem::swap(&mut self.backup, &mut fresh.backup);
        #[cfg(feature = "gpu")]
        fresh.video.retain_renderer(&mut self.video);
        *self = fresh;
    }

    pub fn set_buttons(&mut self, pressed: u16) {
        let mask = pressed & 0x3ff;
        self.io[0x130..0x132].copy_from_slice(&(mask ^ 0x3ff).to_le_bytes());
        let control = half(&self.io, 0x132);
        let keys = control & 0x3ff;
        if control & 0x4000 != 0
            && if control & 0x8000 != 0 {
                mask & keys == keys
            } else {
                mask & keys != 0
            }
        {
            self.interrupt(12);
        }
    }

    fn interrupt(&mut self, bit: u8) {
        self.io[0x202 + usize::from(bit / 8)] |= 1 << (bit % 8);
    }

    pub fn pending(&self) -> u16 {
        half(&self.io, 0x200) & half(&self.io, 0x202)
    }

    pub fn irq_pending(&self) -> bool {
        self.io[0x208] & 1 != 0 && self.pending() != 0
    }

    pub fn cycles_to_event(&self) -> u32 {
        let edge = if self.cycle < HBLANK {
            HBLANK
        } else {
            SCANLINE
        };
        let mut until = edge - self.cycle;
        if self.enabled_timers != 0 {
            for (index, timer) in self.timers.iter().enumerate() {
                let control = self.io[TIMER_BASE + index * 4 + 2];
                if control & 0x80 != 0 && (index == 0 || control & 4 == 0) {
                    let scale = TIMER_DIVISORS[usize::from(control & 3)];
                    until = until.min((0x10000 - u32::from(timer.count)) * scale - timer.fraction);
                }
            }
        }
        until
    }

    pub fn tick(&mut self, cycles: u32) {
        self.backup.tick(cycles);
        if self.enabled_timers == 0 {
            self.audio.advance(&mut self.io, cycles);
        } else {
            // A halted CPU may skip across several samples. Latch FIFO data at each
            // timer overflow before mixing the sample on a coincident PWM edge.
            let mut remaining = cycles;
            while remaining != 0 {
                let span = remaining.min(self.audio.until_pwm(&self.io));
                self.tick_timers(span);
                self.audio.advance(&mut self.io, span);
                remaining -= span;
            }
        }
        let previous = self.cycle;
        self.cycle += cycles;
        if previous < DRAW_END && self.cycle >= DRAW_END && self.line < HEIGHT as u16 {
            self.video.render_line(usize::from(self.line), &self.io);
        }
        if previous < HBLANK && self.cycle >= HBLANK {
            self.io[4] |= 2;
            if self.io[4] & 0x10 != 0 {
                self.interrupt(1);
            }
            if self.line < HEIGHT as u16 {
                self.video.advance_reference(&self.io);
                self.trigger_dma(2);
            }
        }
        if self.cycle >= SCANLINE {
            self.cycle -= SCANLINE;
            self.line = (self.line + 1) % LINES;
            self.io[6..8].copy_from_slice(&self.line.to_le_bytes());
            self.io[4] &= !2;
            if self.line == 0 {
                for offset in [0x28, 0x2c, 0x38, 0x3c] {
                    self.video.reload_reference(&self.io, offset);
                }
            }
            if self.line == HEIGHT as u16 {
                self.io[4] |= 1;
                if self.io[4] & 8 != 0 {
                    self.interrupt(0);
                }
                self.trigger_dma(1);
            } else if self.line == LINES - 1 {
                self.io[4] &= !1;
            }
            self.update_vcount_match();
        }
    }

    fn update_vcount_match(&mut self) {
        if self.line == u16::from(self.io[5]) {
            if self.io[4] & 0x24 == 0x20 {
                self.interrupt(2);
            }
            self.io[4] |= 4;
        } else {
            self.io[4] &= !4;
        }
    }

    fn tick_timers(&mut self, cycles: u32) {
        let mut overflows = 0;
        for index in 0..4 {
            let base = TIMER_BASE + index * 4;
            let control = self.io[base + 2];
            if control & 0x80 == 0 {
                overflows = 0;
                continue;
            }
            let timer = &mut self.timers[index];
            let ticks = if index != 0 && control & 4 != 0 {
                overflows
            } else {
                let scale = TIMER_DIVISORS[usize::from(control & 3)];
                timer.fraction += cycles;
                let ticks = timer.fraction / scale;
                timer.fraction %= scale;
                ticks
            };
            let next = u32::from(timer.count) + ticks;
            overflows = 0;
            if next > 0xffff {
                let reload = u32::from(half(&self.io, base));
                let period = 0x10000 - reload;
                overflows = 1 + (next - 0x10000) / period;
                timer.count = (reload + (next - 0x10000) % period) as u16;
                if index < 2 {
                    for _ in 0..overflows {
                        let requests = self.audio.timer_overflow(&self.io, index);
                        if requests != 0 {
                            self.fifo_dma(requests);
                        }
                    }
                }
                if control & 0x40 != 0 {
                    self.interrupt(3 + index as u8);
                }
            } else {
                timer.count = next as u16;
            }
        }
    }

    fn fifo_dma(&mut self, requests: u8) {
        for channel in 1..=2 {
            if self.io[DMA_BASE + channel * 12 + 11] & 0xb0 != 0xb0 {
                continue;
            }
            let fifo = match self.dma[channel].dst & !3 {
                0x0400_00a0 => 1,
                0x0400_00a4 => 2,
                _ => continue,
            };
            if requests & fifo != 0 {
                self.transfer_dma(channel);
            }
        }
    }

    fn trigger_dma(&mut self, timing: u8) {
        for channel in 0..4 {
            let control = self.io[DMA_BASE + channel * 12 + 11];
            if control & 0x80 != 0 && (control >> 4) & 3 == timing {
                self.transfer_dma(channel);
            }
        }
    }

    fn transfer_dma(&mut self, channel: usize) {
        if self.dma_busy {
            return;
        }
        self.dma_busy = true;
        let base = DMA_BASE + channel * 12;
        let control = half(&self.io, base + 10);
        let mask = if channel == 3 { 0xffff } else { 0x3fff };
        let fifo = matches!(channel, 1 | 2) && control & 0x3000 == 0x3000;
        let count = if fifo {
            4
        } else {
            u32::from(half(&self.io, base + 8)) & mask
        };
        let width = if fifo || control & (1 << 10) != 0 {
            4
        } else {
            2
        };
        let step = |mode| match mode {
            1 => 0u32.wrapping_sub(width),
            2 => 0,
            _ => width,
        };
        let src_step = step((control >> 7) & 3);
        let dst_mode = (control >> 5) & 3;
        let dst_step = if fifo { 0 } else { step(dst_mode) };
        let mut latch = self.dma[channel];
        if channel == 3
            && width == 2
            && self.eeprom_port(latch.dst)
            && matches!(count, 9 | 17 | 73 | 81)
        {
            let mut bits = [0; 81];
            for bit in &mut bits[..count as usize] {
                *bit = (self.read16(latch.src) & 1) as u8;
                latch.src = latch.src.wrapping_add(src_step);
                latch.dst = latch.dst.wrapping_add(dst_step);
            }
            self.backup.eeprom_command(&bits[..count as usize]);
        } else {
            let eeprom_read = channel == 3 && width == 2 && self.eeprom_port(latch.src);
            for _ in 0..if count == 0 { mask + 1 } else { count } {
                if width == 4 {
                    self.write32(latch.dst, self.read32(latch.src));
                } else {
                    let value = if eeprom_read {
                        self.backup.read_eeprom_bit()
                    } else {
                        self.read16(latch.src)
                    };
                    self.write16(latch.dst, value);
                }
                latch.dst = latch.dst.wrapping_add(dst_step);
                latch.src = latch.src.wrapping_add(src_step);
            }
        }
        if dst_mode == 3 {
            latch.dst = self.dma[channel].dst;
        }
        self.dma[channel] = latch;
        if control & 0x3000 == 0 || control & (1 << 9) == 0 {
            self.io[base + 11] &= !0x80;
        }
        if control & (1 << 14) != 0 {
            self.interrupt(8 + channel as u8);
        }
        self.dma_busy = false;
    }

    fn io_byte(&mut self, offset: usize, value: u8) {
        match offset {
            4 => self.io[4] = (self.io[4] & 7) | (value & 0x38),
            5 => {
                self.io[5] = value;
                self.update_vcount_match();
            }
            6 | 7 | 0x130 | 0x131 => {}
            0x28..=0x2f | 0x38..=0x3f => {
                self.io[offset] = value;
                self.video.reload_reference(&self.io, offset);
            }
            0x60..=0xa7 => self.audio.write(&mut self.io, offset, value),
            0x202 | 0x203 => self.io[offset] &= !value,
            0x300 | 0x301 if self.has_bios() && (!self.fetch.in_bios || self.dma_busy) => {}
            0x300 => self.io[0x300] = value & 1,
            0x301 => self.halted = true,
            0xba..=0xdf if (offset - DMA_BASE) % 12 == 11 => {
                let old = self.io[offset];
                self.io[offset] = value;
                if old & 0x80 == 0 && value & 0x80 != 0 {
                    let channel = (offset - DMA_BASE) / 12;
                    let base = DMA_BASE + channel * 12;
                    let io_word = |offset| {
                        u32::from(half(&self.io, offset))
                            | (u32::from(half(&self.io, offset + 2)) << 16)
                    };
                    self.dma[channel] = Dma {
                        src: io_word(base)
                            & if channel == 0 {
                                0x07ff_ffff
                            } else {
                                0x0fff_ffff
                            },
                        dst: io_word(base + 4)
                            & if channel == 3 {
                                0x0fff_ffff
                            } else {
                                0x07ff_ffff
                            },
                    };
                    if value & 0x30 == 0 {
                        self.transfer_dma(channel);
                    }
                }
            }
            0x102 | 0x106 | 0x10a | 0x10e => {
                let timer = &mut self.timers[(offset - TIMER_BASE) / 4];
                if self.io[offset] & 0x80 == 0 && value & 0x80 != 0 {
                    *timer = Timer {
                        count: half(&self.io, offset - 2),
                        fraction: 0,
                    };
                } else if value & 7 != self.io[offset] & 7 {
                    timer.fraction = 0;
                }
                self.io[offset] = value;
                let bit = 1 << ((offset - TIMER_BASE) / 4);
                if value & 0x80 == 0 {
                    self.enabled_timers &= !bit;
                } else {
                    self.enabled_timers |= bit;
                }
            }
            _ => self.io[offset] = value,
        }
    }

    pub fn reset_ram(&mut self, flags: u32) {
        if flags & 1 != 0 {
            self.ewram.fill(0);
        }
        if flags & 2 != 0 {
            self.iwram[..0x7e00].fill(0);
        }
        if flags & 4 != 0 {
            self.video.palette.fill(0);
        }
        if flags & 8 != 0 {
            self.video.vram.fill(0);
        }
        if flags & 16 != 0 {
            self.video.oam.fill(0);
        }
        self.io[0x120..0x122].fill(0);
        if flags & 0x20 != 0 {
            self.io[0x120..0x12c].fill(0);
            self.io[0x134..0x15c].fill(0);
            self.io[0x135] = 0x80;
        }
        if flags & 0x40 != 0 {
            self.io[0x60..0xb0].fill(0);
            self.io[0x89] = 2;
            self.audio.reset_registers();
        }
        if flags & 0x80 != 0 {
            self.io[1..4].fill(0);
            self.io[4] &= 3;
            self.io[5] = 0;
            self.update_vcount_match();
            self.io[8..0x60].fill(0);
            self.io[DMA_BASE..0x110].fill(0);
            self.io[0x132..0x134].fill(0);
            self.io[0x200..0x20c].fill(0);
            self.timers = [Timer::default(); 4];
            self.enabled_timers = 0;
            self.dma = [Dma::default(); 4];
            for offset in [0x28, 0x2c, 0x38, 0x3c] {
                self.video.reload_reference(&self.io, offset);
            }
            for n in [0x20, 0x26, 0x30, 0x36] {
                self.io[n..n + 2].copy_from_slice(&0x100u16.to_le_bytes());
            }
        }
        self.io[0] = 0x80;
    }

    fn eeprom_port(&self, addr: u32) -> bool {
        addr >> 24 == 13
            && (self.rom.len() <= 0x100_0000 || addr >= 0x0dff_ff00)
            && self.backup.is_eeprom()
    }

    fn store(&mut self, address: u32, value: u8) {
        let addr = address as usize;
        match addr >> 24 {
            2 => self.ewram[addr & 0x3ffff] = value,
            3 => self.iwram[addr & 0x7fff] = value,
            4 if addr & 0xff_ffff < self.io.len() => self.io_byte(addr & 1023, value),
            5 => self.video.palette[addr & 1023] = value,
            6 => self.video.vram[vram_offset(addr)] = value,
            7 => self.video.oam[addr & 1023] = value,
            14 | 15 => self.backup.write_parallel(address as u16, value),
            _ => {}
        }
    }
}

fn vram_offset(addr: usize) -> usize {
    let offset = addr & 0x1ffff;
    if offset >= 0x18000 {
        offset - 0x8000
    } else {
        offset
    }
}

impl Bus for Memory {
    fn begin_instruction(&mut self, pc: u32, width: u32) {
        self.fetch.cartridge_started |= matches!(pc >> 24, 8..=13);
        self.fetch.in_bios = pc < BIOS_LEN as u32 && self.bios.is_some();
        if self.fetch.in_bios
            && let Some(bios) = &self.bios
        {
            // The ARM7 pipeline has already fetched two instructions ahead, even on a branch.
            let fetch = ((pc + 2 * width) as usize) & !3;
            if let Some(word) = bios.get(fetch..fetch + 4) {
                self.fetch.bios_latch.copy_from_slice(word);
            }
        }
    }

    fn read8(&self, address: u32) -> u8 {
        let addr = address as usize;
        match addr >> 24 {
            0 if addr < BIOS_LEN && !self.dma_busy => {
                if self.fetch.in_bios
                    && let Some(bios) = &self.bios
                {
                    bios[addr]
                } else {
                    self.fetch.bios_latch[addr & 3]
                }
            }
            2 => self.ewram[addr & 0x3ffff],
            3 => self.iwram[addr & 0x7fff],
            4 if addr & 0xff_ffff < self.io.len() => {
                let offset = addr & 1023;
                if (0x60..=0xa7).contains(&offset) {
                    self.audio.read(&self.io, offset)
                } else if (TIMER_BASE..TIMER_BASE + 16).contains(&offset) && offset & 2 == 0 {
                    self.timers[(offset - TIMER_BASE) / 4].count.to_le_bytes()[offset & 1]
                } else {
                    self.io[offset]
                }
            }
            5 => self.video.palette[addr & 1023],
            6 => self.video.vram[vram_offset(addr)],
            7 => self.video.oam[addr & 1023],
            13 if self.eeprom_port(address) => {
                (self.backup.peek_eeprom_bit() >> ((addr & 1) * 8)) as u8
            }
            8..=13 => {
                let offset = addr & 0x01ff_ffff;
                if (0xc4..=0xc9).contains(&offset)
                    && let Some(rtc) = &self.rtc
                    && rtc.read_enabled()
                {
                    return rtc.read(offset);
                }
                self.rom
                    .get(offset)
                    .copied()
                    .unwrap_or_else(|| ((addr >> 1) as u16).to_le_bytes()[addr & 1])
            }
            14 | 15 => self.backup.read_parallel(address as u16),
            _ => 0,
        }
    }

    fn read16(&self, addr: u32) -> u16 {
        if matches!(addr >> 24, 14 | 15) {
            return u16::from(self.read8(addr)) * 0x101;
        }
        let addr = addr & !1;
        u16::from_le_bytes([self.read8(addr), self.read8(addr + 1)])
    }

    fn read32(&self, addr: u32) -> u32 {
        if matches!(addr >> 24, 14 | 15) {
            return u32::from(self.read8(addr)) * 0x0101_0101;
        }
        let addr = addr & !3;
        u32::from(self.read16(addr)) | (u32::from(self.read16(addr + 2)) << 16)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        match addr >> 24 {
            6 if vram_offset(addr as usize)
                >= if self.io[0] & 7 < 3 { 0x10000 } else { 0x14000 } => {}
            5 | 6 => self.write16(addr, u16::from(value) * 0x101),
            7 => {}
            _ => self.store(addr, value),
        }
    }

    fn write16(&mut self, addr: u32, value: u16) {
        if matches!(addr >> 24, 8..=13) && (0xc4..=0xc9).contains(&(addr & 0x01ff_ffff)) {
            if let Some(rtc) = &mut self.rtc {
                rtc.write(addr as usize & 0x01ff_fffe, value as u8);
            }
        } else if addr & !1 == 0x0400_0300 && self.has_bios() {
            // A wide write setting POSTFLG on cold boot does not also enter HALT.
            if self.fetch.in_bios && !self.dma_busy {
                self.halted |= self.io[0x300] != 0;
                self.io[0x300] = value as u8 & 1;
            }
        } else if matches!(addr >> 24, 14 | 15) {
            self.store(addr, value.to_le_bytes()[addr as usize & 1]);
        } else {
            let addr = addr & !1;
            let bytes = value.to_le_bytes();
            self.store(addr, bytes[0]);
            self.store(addr + 1, bytes[1]);
        }
    }

    fn write32(&mut self, addr: u32, value: u32) {
        if matches!(addr >> 24, 14 | 15) {
            self.store(addr, value.to_le_bytes()[addr as usize & 3]);
        } else {
            let addr = addr & !3;
            self.write16(addr, value as u16);
            self.write16(addr + 2, (value >> 16) as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bios_reads_outside_firmware_return_the_last_prefetched_word_by_byte_lane() {
        let mut firmware = vec![0; BIOS_LEN];
        firmware[8..12].copy_from_slice(&0x1122_3344u32.to_le_bytes());
        firmware[12..16].copy_from_slice(&0x5566_7788u32.to_le_bytes());
        let mut bus = Memory::new(vec![0; 192]);
        bus.install_bios(firmware.into_boxed_slice());

        bus.begin_instruction(4, 4);
        assert_eq!(bus.read32(8), 0x1122_3344);
        bus.write32(8, 0);
        assert_eq!(bus.read32(8), 0x1122_3344);
        // Even a firmware-initiated DMA cannot copy the protected image.
        bus.write32(0x0400_00d4, 8);
        bus.write32(0x0400_00d8, 0x0200_0000);
        bus.write32(0x0400_00dc, 0x8400_0001);
        assert_eq!(bus.read32(0x0200_0000), 0);
        bus.begin_instruction(0x0300_0000, 4);
        assert_eq!(bus.read32(8), 0x5566_7788);
        assert_eq!(bus.read16(2), 0x5566);
        assert_eq!(bus.read8(1), 0x77);
        assert!(!bus.cartridge_started());

        bus.begin_instruction(6, 2);
        bus.begin_instruction(0x0a00_0000, 2);
        assert_eq!(bus.read32(12), 0x1122_3344);
        assert!(bus.cartridge_started());
        bus.reset();
        assert_eq!(bus.read32(8), 0);
        assert!(!bus.cartridge_started());
    }

    #[test]
    fn only_firmware_can_set_post_and_halt_after_power_on() {
        let mut bus = Memory::new(vec![]);
        bus.install_bios(vec![1; BIOS_LEN].into_boxed_slice());
        bus.begin_instruction(0, 4);
        assert_eq!(bus.read8(0x0400_0300), 0);
        bus.write32(0x0400_0300, 1);
        assert_eq!(bus.read8(0x0400_0300), 1);
        assert!(!bus.halted);
        bus.begin_instruction(0x0800_0000, 4);
        bus.write8(0x0400_0300, 0);
        bus.write8(0x0400_0301, 0);
        assert_eq!(bus.read8(0x0400_0300), 1);
        assert!(!bus.halted);
        bus.begin_instruction(8, 4);
        bus.write8(0x0400_0301, 0);
        assert!(bus.halted);
    }

    #[test]
    fn firmware_cartridge_data_reads_do_not_mark_the_game_started() {
        let mut bios = vec![0; BIOS_LEN];
        for (chunk, word) in bios.as_chunks_mut::<4>().0.iter_mut().zip([
            0xe3a0_0302_u32, // mov r0, #0x08000000
            0xe590_1000,     // ldr r1, [r0]
            0xeaff_fffe,     // b .
        ]) {
            *chunk = word.to_le_bytes();
        }
        let mut machine = crate::Machine::with_bios(vec![0x42; crate::HEADER_LEN], bios).unwrap();
        machine.run_frame().unwrap();
        assert_eq!(machine.cpu.regs[1], 0x4242_4242);
        assert!(!machine.cartridge_started());
    }

    #[test]
    fn palette_vram_and_oam_observe_bus_width_and_mirrors() {
        let mut bus = Memory::new(vec![]);
        bus.write8(0x0500_0001, 0xab);
        assert_eq!(bus.read16(0x0500_0400), 0xabab);
        bus.write16(0x0601_8000, 0x1357);
        bus.write8(0x0601_0000, 0xee);
        assert_eq!(bus.read16(0x0601_0000), 0x1357);
        bus.write8(0x0700_0000, 0x23);
        assert_eq!(bus.read16(0x0700_0000), 0);
        bus.write16(0x0700_0000, 0x1234);
        assert_eq!(bus.read16(0x0700_0400), 0x1234);
    }

    #[test]
    fn vblank_dma_reloads_destination_and_preserves_source_progress() {
        let mut bus = Memory::new(vec![]);
        bus.write32(0x0200_0000, 0x5678_1234);
        bus.write32(0x0400_00d4, 0x0200_0000);
        bus.write32(0x0400_00d8, 0x0600_0000);
        bus.write32(0x0400_00dc, 0xd260_0001);
        assert_eq!(bus.read16(0x0600_0000), 0);
        bus.line = 159;
        bus.tick(SCANLINE);
        assert_eq!(bus.read16(0x0400_0006), 160);
        assert_eq!(bus.read16(0x0600_0000), 0x1234);
        assert_eq!(bus.read16(0x0400_0202) & 0x800, 0x800);
        bus.write16(0x0400_0202, 0x800);
        bus.line = 159;
        bus.tick(SCANLINE);
        assert_eq!(bus.read16(0x0600_0000), 0x5678);
        assert_eq!(bus.read16(0x0600_0002), 0);
    }

    #[test]
    fn prescaled_and_cascaded_timers_reload_and_request_irq() {
        let mut bus = Memory::new(vec![]);
        bus.write32(0x0400_0100, 0x0081_fffe);
        bus.write32(0x0400_0104, 0x00c4_ffff);
        bus.tick(127);
        assert_eq!(bus.read16(0x0400_0100), 0xffff);
        assert_eq!(bus.read16(0x0400_0202), 0);
        bus.tick(1);
        assert_eq!(bus.read16(0x0400_0100), 0xfffe);
        assert_eq!(bus.read16(0x0400_0104), 0xffff);
        assert_eq!(bus.read16(0x0400_0202), 1 << 4);
    }

    #[test]
    fn bitmap_page_flip_and_text_tile_flip_change_the_first_pixel() {
        let mut bus = Memory::new(vec![]);
        bus.write16(0x0400_0000, 0x0414); // mode 4, page 1
        bus.write16(0x0600_a000, 0x0102);
        bus.write16(0x0500_0004, 0x03e0);
        bus.tick(HBLANK);
        assert_eq!(&bus.video.pixels[..3], &[0, 255, 0]);
        bus.write16(0x0400_0000, 0x0100);
        bus.write16(0x0400_0008, 0x0800); // BG0 map at 0x4000
        bus.write16(0x0600_4000, 0x0401); // flipped 4bpp tile 1
        bus.write16(0x0600_0022, 0x3000);
        bus.write16(0x0500_0006, 0x001f);
        bus.video.render_line(0, &bus.io);
        assert_eq!(&bus.video.pixels[..3], &[255, 0, 0]);
    }

    #[test]
    fn affine_hblank_writes_apply_to_next_scanline_and_reset_on_new_frame() {
        let mut bus = Memory::new(vec![]);
        bus.write16(0x0400_0000, 0x0403);
        bus.write32(0x0600_0000, 0x7c00_001f);
        bus.tick(DRAW_END);
        assert_eq!(&bus.video.pixels[..3], &[255, 0, 0]);
        assert_eq!(bus.read16(0x0400_0004) & 2, 0);
        bus.tick(HBLANK - DRAW_END);
        assert_ne!(bus.read16(0x0400_0004) & 2, 0);
        bus.write32(0x0400_0028, 256);
        bus.write32(0x0400_002c, 0);
        bus.tick(SCANLINE - HBLANK);
        bus.tick(HBLANK);
        let pixel = crate::WIDTH * 3;
        assert_eq!(&bus.video.pixels[pixel..pixel + 3], &[0, 0, 255]);
        bus.line = LINES - 1;
        bus.tick(SCANLINE - HBLANK);
        bus.tick(HBLANK);
        assert_eq!(&bus.video.pixels[..3], &[0, 0, 255]);
    }

    #[test]
    fn lower_oam_index_wins_before_comparing_with_background_priority() {
        let mut bus = Memory::new(vec![]);
        bus.write16(0x0400_0000, 0x1000);
        // OBJ 0 has lower BG priority and uses tile 0. OBJ 1 uses tile 1.
        bus.write16(0x0700_0004, 3 << 10);
        bus.write16(0x0700_000c, 1);
        bus.write16(0x0601_0000, 1);
        bus.write16(0x0601_0020, 2);
        bus.write16(0x0500_0202, 0x03e0);
        bus.write16(0x0500_0204, 0x001f);
        bus.tick(DRAW_END);
        assert_eq!(&bus.video.pixels[..3], &[0, 255, 0]);
    }

    #[test]
    fn sleeping_cpu_wakes_on_next_timer_edge_after_a_prescaler_change() {
        let mut bus = Memory::new(vec![]);
        bus.write32(0x0400_0100, 0x00c3_ffff);
        bus.tick(100);
        bus.write16(0x0400_0102, 0x00c0);
        assert_eq!(bus.cycles_to_event(), 1);
        bus.tick(1);
        assert_eq!(bus.read16(0x0400_0202), 8);
    }
}
