//! ARM9 and ARM7 views of the DS memory map, clocks and system registers.
use crate::audio::Audio;
use crate::devices::{Card, Ipc, Spi, Timers, backup::Backup, wifi::Wifi};
use crate::display::Display;
use eidoryn_arm::Bus;
use serde::{Deserialize, Serialize};
use std::cell::Cell;

const MAIN: usize = 4 * 1024 * 1024;
const WRAM: usize = 32 * 1024;
const PRIVATE: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
pub(crate) struct Memory {
    #[serde(with = "serde_bytes")]
    main: Vec<u8>,
    #[serde(with = "serde_bytes")]
    shared: Vec<u8>,
    #[serde(with = "serde_bytes")]
    arm7: Vec<u8>,
    #[serde(with = "serde_bytes")]
    itcm: Vec<u8>,
    #[serde(with = "serde_bytes")]
    dtcm: Vec<u8>,
    pub(crate) display: Display,
    io: [Vec<u8>; 2],
    cp15: [u32; 3],
    wramcnt: u8,
    keys: u16,
    pub(crate) pen: Option<(u8, u8)>,
    pub(crate) line: u16,
    pub(crate) dot: u16,
    pub(crate) pending: [Cell<u32>; 2],
    pub(crate) halted: [bool; 2],
    timers: [Timers; 2],
    ipc: Ipc,
    spi: Spi,
    card: Card,
    // Stored after the original machine payload to preserve v1 snapshot compatibility.
    #[serde(skip)]
    pub(crate) backup: Backup,
    #[serde(skip)]
    pub(crate) wifi: Wifi,
    #[serde(skip)]
    pub(crate) audio: Audio,
    dma_ready: [Cell<u8>; 2],
}

impl Default for Memory {
    fn default() -> Self {
        let mut mem = Self {
            main: vec![0; MAIN],
            shared: vec![0; WRAM],
            arm7: vec![0; PRIVATE],
            itcm: vec![0; 32768],
            dtcm: vec![0; 16384],
            display: Display::default(),
            io: std::array::from_fn(|_| vec![0; 0x400]),
            cp15: [0x0005_707d, 0x0300_000a, 0x20],
            wramcnt: 3,
            keys: 0,
            pen: None,
            line: 0,
            dot: 0,
            pending: Default::default(),
            halted: [false; 2],
            timers: Default::default(),
            ipc: Ipc::default(),
            spi: Spi::default(),
            card: Card::default(),
            backup: Backup::default(),
            wifi: Wifi::default(),
            audio: Audio::default(),
            dma_ready: Default::default(),
        };
        for io in &mut mem.io {
            io[0x300] = 1;
        }
        mem.io[1][0x304] = 1; // Direct boot leaves the ARM7 sound circuit powered.
        mem
    }
}

impl Memory {
    pub(crate) fn valid(&self) -> bool {
        self.main.len() == MAIN
            && self.shared.len() == WRAM
            && self.arm7.len() == PRIVATE
            && self.itcm.len() == 32768
            && self.dtcm.len() == 16384
            && self.io.iter().all(|b| b.len() == 0x400)
            && self.wramcnt <= 3
            && self.line < 263
            && self.dot < 2130
            && self.display.valid_state()
            && self.wifi.valid()
            && self.audio.valid()
            && self.ipc.valid()
            && self.timers.iter().all(Timers::valid)
            && self.pen.is_none_or(|(_, y)| y < 192)
    }
    fn shared_index(&self, nine: bool, addr: u32) -> Option<usize> {
        shared_index(nine, self.wramcnt, addr)
    }
    fn interrupt_flags(&self, cpu: usize) -> u32 {
        // With immediate geometry consumption the FIFO stays below its watermark.
        // Its level IRQ remains asserted while the selected condition is true.
        self.pending[cpu].get() | (u32::from(cpu == 0 && self.display.gx_fifo_irq()) << 21)
    }
    pub(crate) fn irq(&self, cpu: usize) -> u32 {
        u32::from_le_bytes(
            self.io[cpu][0x210..0x214]
                .try_into()
                .expect("IE is a 32-bit register"),
        ) & self.interrupt_flags(cpu)
    }
    pub(crate) fn master_irq(&self, cpu: usize) -> bool {
        self.io[cpu][0x208] & 1 != 0
    }
    pub(crate) fn irq_vector(&self, nine: bool) -> u32 {
        if nine {
            (self.cp15[1] & 0xffff_f000).wrapping_add(0x3ffc)
        } else {
            0x0380_fffc
        }
    }
    pub(crate) fn bios_flags(&self, nine: bool) -> u32 {
        self.irq_vector(nine) - 4
    }
    fn key_condition(&self, cpu: usize, keys: u16) -> bool {
        let ctl = u16::from_le_bytes([self.io[cpu][0x132], self.io[cpu][0x133]]);
        let mask = ctl & 0x3ff;
        ctl & 0x4000 != 0
            && mask != 0
            && if ctl & 0x8000 != 0 {
                (keys & mask) == mask
            } else {
                (keys & mask) != 0
            }
    }
    pub(crate) fn set_keys(&mut self, keys: u16) {
        for cpu in 0..2 {
            if !self.key_condition(cpu, self.keys) && self.key_condition(cpu, keys) {
                self.signal(cpu, 1 << 12);
            }
        }
        self.keys = keys & 0xfff;
    }
    fn signal(&self, cpu: usize, mask: u32) {
        self.pending[cpu].set(self.pending[cpu].get() | mask);
    }
    fn display_status(&self, cpu: usize) -> u16 {
        let ctl = u16::from_le_bytes([self.io[cpu][4], self.io[cpu][5]]);
        let target = (ctl >> 8) | ((ctl & 128) << 1);
        (ctl & !7)
            | u16::from((192..262).contains(&self.line))
            | (u16::from(self.dot >= 1606) << 1)
            | (u16::from(self.line == target) << 2)
    }
    fn flush_audio(&mut self) {
        let mut ram = SoundRam {
            main: &mut self.main,
            shared: &mut self.shared,
            arm7: &mut self.arm7,
            display: &mut self.display,
            wramcnt: self.wramcnt,
        };
        self.audio.flush(&mut ram, self.io[1][0x304] & 1 != 0);
    }
    /// Advance four ARM7 bus clocks per scheduling quantum. A line has 2,130 clocks.
    pub(crate) fn advance(&mut self, ticks: u16) -> bool {
        if self.audio.accumulate(ticks) {
            self.flush_audio();
        }
        for cpu in 0..2 {
            let irqs = self.timers[cpu].advance(u32::from(ticks));
            self.signal(cpu, irqs);
        }
        let old = self.dot;
        self.dot += ticks;
        if old < 1606 && self.dot >= 1606 {
            for cpu in 0..2 {
                if self.io[cpu][4] & 16 != 0 {
                    self.signal(cpu, 2);
                }
            }
            if self.line < 192 {
                self.trigger_dma(0, 2);
            }
        }
        if self.dot < 2130 {
            return false;
        }
        self.dot -= 2130;
        self.line = (self.line + 1) % 263;
        for cpu in 0..2 {
            if self.line == 192 {
                if self.io[cpu][4] & 8 != 0 {
                    self.signal(cpu, 1);
                }
                self.trigger_dma(cpu, 1);
            }
            if self.display_status(cpu) & 0x24 == 0x24 {
                self.signal(cpu, 4);
            }
        }
        if self.line == 192 {
            self.display.render();
        }
        self.line == 0
    }
    fn dma_control(&self, cpu: usize, channel: usize) -> u32 {
        let i = 0xb8 + channel * 12;
        u32::from_le_bytes(
            self.io[cpu][i..i + 4]
                .try_into()
                .expect("DMA control is a 32-bit register"),
        )
    }
    fn trigger_dma(&self, cpu: usize, timing: u32) {
        for ch in 0..4 {
            let control = self.dma_control(cpu, ch);
            let shift = if cpu == 0 { 27 } else { 28 };
            if control >> 31 != 0 && (control >> shift) & (if cpu == 0 { 7 } else { 3 }) == timing {
                self.dma_ready[cpu].set(self.dma_ready[cpu].get() | (1 << ch));
            }
        }
    }
}

fn shared_index(nine: bool, wramcnt: u8, addr: u32) -> Option<usize> {
    let offset = addr as usize;
    match (nine, wramcnt) {
        (true, 0) | (false, 3) => Some(offset & 0x7fff),
        (true, 1) | (false, 2) => Some(offset & 0x3fff),
        (true, 2) | (false, 1) => Some(0x4000 | offset & 0x3fff),
        _ => None,
    }
}

/// The audio DMA sees the ARM7 mapping of RAM and VRAM, including the WRAM bank switch.
pub(crate) struct SoundRam<'a> {
    main: &'a mut [u8],
    shared: &'a mut [u8],
    arm7: &'a mut [u8],
    display: &'a mut Display,
    wramcnt: u8,
}
impl SoundRam<'_> {
    pub(crate) fn read8(&self, a: u32) -> u8 {
        match a >> 24 {
            2 => self.main[a as usize & (MAIN - 1)],
            3 if a >= 0x0380_0000 => self.arm7[a as usize & (PRIVATE - 1)],
            3 => shared_index(false, self.wramcnt, a).map_or(0, |i| self.shared[i]),
            6 => self.display.arm7_read8(a),
            _ => 0,
        }
    }
    pub(crate) fn read16(&self, a: u32) -> i16 {
        i16::from_le_bytes([self.read8(a), self.read8(a.wrapping_add(1))])
    }
    pub(crate) fn write8(&mut self, a: u32, v: u8) {
        match a >> 24 {
            2 => self.main[a as usize & (MAIN - 1)] = v,
            3 if a >= 0x0380_0000 => self.arm7[a as usize & (PRIVATE - 1)] = v,
            3 => {
                if let Some(i) = shared_index(false, self.wramcnt, a) {
                    self.shared[i] = v;
                }
            }
            6 => self.display.arm7_write8(a, v),
            _ => {}
        }
    }
}
pub(crate) struct Port<'a, const NINE: bool> {
    pub(crate) m: &'a mut Memory,
    pub(crate) rom: &'a [u8],
}
impl<const NINE: bool> Port<'_, NINE> {
    const CPU: usize = if NINE { 0 } else { 1 };
    const CARD_DMA: u8 = if NINE { 5 } else { 2 };
    fn dtcm_offset(&self, a: u32) -> Option<usize> {
        if !NINE || self.m.cp15[0] & (1 << 16) == 0 {
            return None;
        }
        let offset = a.wrapping_sub(self.m.cp15[1] & 0xffff_f000);
        (offset < 16384).then_some(offset as usize)
    }
    fn wifi_access(&self, addr: u32) -> bool {
        (0x0480_0000..0x0481_0000).contains(&addr) && self.m.io[1][0x304] & 2 != 0
    }
    fn io_read(&self, a: usize) -> u8 {
        let m = &self.m;
        let cpu = Self::CPU;
        match a {
            4..=5 => self.m.display_status(cpu).to_le_bytes()[a - 4],
            6..=7 => m.line.to_le_bytes()[a - 6],
            0x100..=0x10f => m.timers[cpu].read(a - 0x100),
            0x130..=0x131 => (!m.keys & 0x03ff).to_le_bytes()[a - 0x130],
            0x136..=0x137 if !NINE => {
                (0x003c | (!(m.keys >> 10) & 3) | (u16::from(m.pen.is_none()) << 6)).to_le_bytes()
                    [a - 0x136]
            }
            0x180..=0x185 => m.ipc.read_control(cpu, a),
            0x1a2 => m.backup.reply(),
            0x1a4..=0x1a7 => {
                let mut byte = m.io[cpu][a];
                if a == 0x1a6 {
                    byte = (byte & !0x80) | (u8::from(m.card.ready()) << 7);
                }
                if a == 0x1a7 {
                    byte = (byte & !0x80) | (u8::from(m.card.ready()) << 7);
                }
                byte
            }
            0x1c2 if !NINE => m.spi.result(),
            0x214..=0x217 => m.interrupt_flags(cpu).to_le_bytes()[a - 0x214],
            0x247 if NINE => m.wramcnt,
            0x241 if !NINE => m.wramcnt,
            0x240 if !NINE => m.display.arm7_read8(0x0400_0240),
            0..=3 | 8..=0x6d | 0x240..=0x249 | 0x304..=0x305 | 0x340..=0x3bf if NINE => {
                m.display.read8(0x0400_0000 + a as u32)
            }
            0x2a0..=0x2af if NINE => self.div_result(a),
            0x2b4..=0x2b7 if NINE => self.sqrt_result().to_le_bytes()[a - 0x2b4],
            _ => m.io[cpu][a],
        }
    }
    fn div_result(&self, a: usize) -> u8 {
        let regs = &self.m.io[0];
        let val = |start: usize| {
            i64::from_le_bytes(
                regs[start..start + 8]
                    .try_into()
                    .expect("64-bit division operand"),
            )
        };
        let mode = regs[0x280] & 3;
        let n = if mode == 0 {
            i64::from(val(0x290) as i32)
        } else {
            val(0x290)
        };
        let d = if mode < 2 {
            i64::from(val(0x298) as i32)
        } else {
            val(0x298)
        };
        let (q, rem) = if d == 0 {
            (if n < 0 { 1 } else { -1 }, n)
        } else {
            (n.wrapping_div(d), n.wrapping_rem(d))
        };
        (if a < 0x2a8 { q } else { rem }).to_le_bytes()[a & 7]
    }
    fn sqrt_result(&self) -> u32 {
        let r = &self.m.io[0];
        let n = u64::from_le_bytes(
            r[0x2b8..0x2c0]
                .try_into()
                .expect("64-bit square root operand"),
        );
        (if r[0x2b0] & 1 == 0 {
            n & u64::from(u32::MAX)
        } else {
            n
        })
        .isqrt() as u32
    }
    fn io_write(&mut self, a: usize, v: u8) {
        let cpu = Self::CPU;
        match a {
            4 => self.m.io[cpu][a] = v & 0xb8,
            5 => self.m.io[cpu][a] = v,
            6..=7 => {}
            0x100..=0x10f => self.m.timers[cpu].write(a - 0x100, v),
            0x132..=0x133 => {
                let was_pressed = self.m.key_condition(cpu, self.m.keys);
                self.m.io[cpu][a] = if a == 0x132 { v } else { v & 0xc3 };
                if !was_pressed && self.m.key_condition(cpu, self.m.keys) {
                    self.m.signal(cpu, 1 << 12);
                }
            }
            0x180..=0x187 => self.m.ipc.write_control(cpu, a, v, &self.m.pending),
            0x214..=0x217 => self.m.pending[cpu]
                .set(self.m.pending[cpu].get() & !(u32::from(v) << ((a - 0x214) * 8))),
            0x247 if NINE => self.m.wramcnt = v & 3,
            0..=3 | 8..=0x6d | 0x240..=0x249 | 0x304..=0x305 | 0x340..=0x3bf if NINE => {
                self.m.display.write8(0x0400_0000 + a as u32, v);
            }
            0x1c2 if !NINE => {
                let ctl = u16::from_le_bytes([self.m.io[cpu][0x1c0], self.m.io[cpu][0x1c1]]);
                self.m.spi.transfer(ctl, v, self.m.pen);
                if ctl & 0xc000 == 0xc000 {
                    self.m.signal(cpu, 1 << 23);
                }
            }
            0x1c1 if !NINE => {
                if self.m.io[cpu][a] & 0x83 != v & 0x83 || v & 0x80 == 0 {
                    self.m.spi.deselect();
                }
                self.m.io[cpu][a] = v;
            }
            0x1a1 => {
                self.m.io[cpu][a] = v;
                if v & 0xa0 != 0xa0 {
                    self.m.backup.deselect();
                }
            }
            0x1a2 => {
                if self.m.io[cpu][0x1a1] & 0xa0 == 0xa0 {
                    self.m.backup.shift(v, self.m.io[cpu][0x1a0] & 0x40 != 0);
                    if self.m.io[cpu][0x1a1] & 0x40 != 0 {
                        self.m.signal(cpu, 1 << 19);
                    }
                }
            }
            0x1a7 if v & 0x80 != 0 => {
                self.m.io[cpu][a] = v;
                let cmd = self.m.io[cpu][0x1a8..0x1b0]
                    .try_into()
                    .expect("eight cartridge command bytes");
                self.m.card.start(cmd, v & 7);
                if self.m.card.ready() {
                    self.m.trigger_dma(cpu, u32::from(Self::CARD_DMA));
                }
            }
            0x304 if !NINE => {
                self.m.flush_audio();
                self.m.io[1][0x304] = v & 3;
            }
            0x301 if !NINE => {
                self.m.halted[cpu] = v >> 6 == 2;
            }
            _ => {
                let old = self.m.io[cpu][a];
                self.m.io[cpu][a] = v;
                if (0xbb..=0xdf).contains(&a)
                    && (a - 0xbb).is_multiple_of(12)
                    && old & 0x80 == 0
                    && v & 0x80 != 0
                {
                    let ch = (a - 0xbb) / 12;
                    let timing = if NINE { v >> 3 & 7 } else { v >> 4 & 3 };
                    // The software geometry engine consumes FIFO writes synchronously,
                    // so it is already below the DMA threshold when a channel starts.
                    if timing == 0
                        || (NINE && timing == 7)
                        || (timing == Self::CARD_DMA && self.m.card.ready())
                    {
                        self.m.dma_ready[cpu].set(self.m.dma_ready[cpu].get() | (1 << ch));
                    }
                }
            }
        }
    }
    pub(crate) fn dma(&mut self) {
        let mask = self.m.dma_ready[Self::CPU].take();
        if mask == 0 {
            return;
        }
        for ch in 0..4 {
            if mask & (1 << ch) == 0 {
                continue;
            }
            let reg = 0x0400_00b0 + 12 * ch;
            let (mut src, mut dst) = (self.read32(reg), self.read32(reg + 4));
            let ctl = self.read32(reg + 8);
            let timing = ctl >> if NINE { 27 } else { 28 } & if NINE { 7 } else { 3 };
            if ctl >> 31 == 0 || (timing == u32::from(Self::CARD_DMA) && !self.m.card.ready()) {
                continue;
            }
            let mask = if NINE {
                0x1f_ffff
            } else if ch == 3 {
                0xffff
            } else {
                0x3fff
            };
            let n = if ctl & mask == 0 {
                mask + 1
            } else {
                ctl & mask
            };
            let width = if ctl & (1 << 26) == 0 { 2 } else { 4 };
            let update = |addr: u32, mode| match mode {
                0 | 3 => addr.wrapping_add(width),
                1 => addr.wrapping_sub(width),
                _ => addr,
            };
            for _ in 0..n {
                if width == 2 {
                    let x = self.read16(src);
                    self.write16(dst, x);
                } else {
                    let x = self.read32(src);
                    self.write32(dst, x);
                }
                src = update(src, ctl >> 23 & 3);
                dst = update(dst, ctl >> 21 & 3);
            }
            if ctl & (1 << 30) != 0 {
                self.m.signal(Self::CPU, 1 << (8 + ch));
            }
            if timing == 0 || ctl & (1 << 25) == 0 {
                self.m.io[Self::CPU][(reg as usize & 0xfff) + 11] &= 0x7f;
            } else {
                if NINE && timing == 7 {
                    self.m.dma_ready[Self::CPU].set(self.m.dma_ready[Self::CPU].get() | (1 << ch));
                }
                self.write32(reg, src);
                if (ctl >> 21) & 3 != 3 {
                    self.write32(reg + 4, dst);
                }
            }
        }
    }
}

impl<const NINE: bool> Bus for Port<'_, NINE> {
    fn armv5(&self) -> bool {
        NINE
    }
    fn cp15_read(&self, n: u8, m: u8, op: u8) -> u32 {
        match (n, m, op) {
            (0, 0, 0) => 0x4105_9461,
            (1, 0, 0) => self.m.cp15[0],
            (9, 1, 0) => self.m.cp15[1],
            (9, 1, 1) => self.m.cp15[2],
            _ => 0,
        }
    }
    fn cp15_write(&mut self, n: u8, m: u8, op: u8, v: u32) {
        match (n, m, op) {
            (1, 0, 0) => self.m.cp15[0] = v,
            (9, 1, 0) => self.m.cp15[1] = v,
            (9, 1, 1) => self.m.cp15[2] = v,
            (7, 0, 4) => self.m.halted[0] = true,
            _ => {}
        }
    }
    fn read8(&self, a: u32) -> u8 {
        if let Some(i) = self.dtcm_offset(a) {
            return self.m.dtcm[i];
        }
        let i = a as usize;
        match a >> 24 {
            0 | 1 if NINE && self.m.cp15[0] & (1 << 18) != 0 => self.m.itcm[i & 0x7fff],
            2 => self.m.main[i & (MAIN - 1)],
            3 if !NINE && a >= 0x0380_0000 => self.m.arm7[i & (PRIVATE - 1)],
            3 => self.m.shared_index(NINE, a).map_or(0, |o| self.m.shared[o]),
            4 if (0x0400_0000..0x0400_0400).contains(&a) => self.io_read(i & 0x3ff),
            4 if !NINE && (0x0400_0400..0x0400_0520).contains(&a) => {
                if self.m.io[1][0x304] & 1 != 0 {
                    self.m.audio.read(i - 0x0400_0400)
                } else {
                    0
                }
            }
            4 if !NINE && self.wifi_access(a) => self.m.wifi.read(a).to_le_bytes()[i & 1],
            4..=7 if NINE => self.m.display.read8(a),
            6 if !NINE => self.m.display.arm7_read8(a),
            _ => 0,
        }
    }
    fn read32(&self, a: u32) -> u32 {
        let a = a & !3;
        if a == 0x0410_0000 {
            return self.m.ipc.receive(Self::CPU, &self.m.pending);
        }
        if a == 0x0410_0010 {
            let was_ready = self.m.card.ready();
            let val = self.m.card.read(self.rom);
            // The game-card interface issues a fresh DMA request for every readable word.
            if self.m.card.ready() {
                self.m.trigger_dma(Self::CPU, u32::from(Self::CARD_DMA));
            } else if was_ready && self.m.io[Self::CPU][0x1a1] & 0x40 != 0 {
                self.m.signal(Self::CPU, 1 << 19);
            }
            return val;
        }
        if a >> 24 == 2 && self.dtcm_offset(a).is_none() {
            let i = a as usize & (MAIN - 1);
            return u32::from_le_bytes(
                self.m.main[i..i + 4]
                    .try_into()
                    .expect("aligned word in main RAM"),
            );
        }
        u32::from_le_bytes(std::array::from_fn(|i| self.read8(a + i as u32)))
    }
    fn write8(&mut self, a: u32, v: u8) {
        if let Some(i) = self.dtcm_offset(a) {
            self.m.dtcm[i] = v;
            return;
        }
        let i = a as usize;
        match a >> 24 {
            0 | 1 if NINE && self.m.cp15[0] & (1 << 18) != 0 => self.m.itcm[i & 0x7fff] = v,
            2 => self.m.main[i & (MAIN - 1)] = v,
            3 if !NINE && a >= 0x0380_0000 => self.m.arm7[i & (PRIVATE - 1)] = v,
            3 => {
                if let Some(o) = self.m.shared_index(NINE, a) {
                    self.m.shared[o] = v;
                }
            }
            4 if (0x0400_0000..0x0400_0400).contains(&a) => self.io_write(i & 0x3ff, v),
            4 if !NINE && (0x0400_0400..0x0400_0520).contains(&a) => {
                if self.m.io[1][0x304] & 1 != 0 {
                    self.m.flush_audio();
                    self.m.audio.write(i - 0x0400_0400, v);
                }
            }
            4..=7 if NINE => self.m.display.write8(a, v),
            6 if !NINE => self.m.display.arm7_write8(a, v),
            _ => {}
        }
    }
    fn write16(&mut self, a: u32, v: u16) {
        let a = a & !1;
        if !NINE && self.wifi_access(a) {
            if self.m.wifi.write(a, v) {
                self.m.signal(Self::CPU, 1 << 24);
            }
        } else {
            let [lo, hi] = v.to_le_bytes();
            self.write8(a, lo);
            self.write8(a + 1, hi);
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        let a = a & !3;
        if a == 0x0400_0188 {
            self.m.ipc.send(Self::CPU, v, &self.m.pending);
            return;
        }
        if a >> 24 == 2 && self.dtcm_offset(a).is_none() {
            let i = a as usize & (MAIN - 1);
            self.m.main[i..i + 4].copy_from_slice(&v.to_le_bytes());
            return;
        }
        self.write16(a, v as u16);
        self.write16(a + 2, (v >> 16) as u16);
    }
}

#[cfg(test)]
mod tests;
