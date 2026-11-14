use crate::{EmuError, cdrom::CdRom, gpu::Gpu, mdec::Mdec, sio::Sio, spu::Spu, timers::Timers};
use std::collections::HashSet;
mod dma;

pub const RAM_SIZE: usize = 2 * 1024 * 1024;
pub const BIOS_SIZE: usize = 512 * 1024;

#[derive(Clone, Copy, Default)]
struct DmaChannel {
    base: u32,
    block: u32,
    control: u32,
}

#[derive(Clone, Copy)]
struct MdecTransfer {
    address: u32,
    remaining: u32,
    block_size: u32,
}

#[derive(Clone)]
pub struct Bus {
    pub ram: Vec<u8>,
    pub bios: Vec<u8>,
    pub gpu: Gpu,
    pub cdrom: CdRom,
    pub spu: Spu,
    pub post: u8,
    pub timers: Timers,
    pub sio: Sio,
    pub mdec: Mdec,
    mdec_dma: [Option<MdecTransfer>; 2],
    cdrom_dma_resume: Option<u64>,
    scratchpad: [u8; 1024],
    memory_control: [u32; 9],
    ram_size: u32,
    cache_control: u32,
    irq_status: u32,
    irq_mask: u32,
    dma: [DmaChannel; 7],
    dma_control: u32,
    dma_interrupt: u32,
    // Shared CPU-clock timeline for every device, including explicit ticks.
    elapsed_cycles: u64,
    // Scratch storage only: cleared before each list, bounded by the number of
    // aligned RAM addresses. Retain capacity across frames instead of reallocating.
    dma_visited: HashSet<u32>,
}

impl Default for Bus {
    fn default() -> Self {
        Self {
            ram: vec![0; RAM_SIZE],
            bios: vec![0; BIOS_SIZE],
            gpu: Gpu::default(),
            cdrom: CdRom::default(),
            spu: Spu::default(),
            post: 0,
            timers: Timers::default(),
            sio: Sio::default(),
            mdec: Mdec::default(),
            mdec_dma: [None; 2],
            cdrom_dma_resume: None,
            scratchpad: [0; 1024],
            memory_control: [0; 9],
            ram_size: 0,
            cache_control: 0,
            irq_status: 0,
            irq_mask: 0,
            dma: [DmaChannel::default(); 7],
            dma_control: 0x0765_4321,
            dma_interrupt: 0,
            elapsed_cycles: 0,
            dma_visited: HashSet::new(),
        }
    }
}

pub fn physical(address: u32) -> u32 {
    match address >> 29 {
        4 => address & 0x7fff_ffff,
        5 => address & 0x1fff_ffff,
        _ => address,
    }
}

impl Bus {
    /// CPU-clock cycles started on this bus, wrapping after `u64::MAX`.
    /// Cloning a bus preserves the timeline. A failed device tick still consumes
    /// the cycle in which the error occurred; subsequent cycles are not run.
    pub fn elapsed_cycles(&self) -> u64 {
        self.elapsed_cycles
    }

    /// Advance devices in CPU-clock order. Quiet spans end before the next
    /// observable CD/SPU event; that boundary is processed one cycle at a
    /// time. Active DMA and SIO also retain the single-cycle path.
    #[inline]
    pub fn tick(&mut self, clocks: u32) -> Result<(), EmuError> {
        match clocks {
            0 => Ok(()),
            1 => self.tick_one(),
            _ => self.tick_many(clocks),
        }
    }

    fn tick_many(&mut self, mut clocks: u32) -> Result<(), EmuError> {
        while clocks > 1 {
            if self
                .dma
                .iter()
                .any(|channel| channel.control & (1 << 24) != 0)
                || self.sio.needs_single_cycle()
            {
                self.tick_one()?;
                clocks -= 1;
                continue;
            }
            let next = self
                .cdrom
                .clocks_until_event()
                .min(self.spu.clocks_until_event());
            // Process the event itself via tick_one, preserving the exact
            // device order and partial progress if a device reports an error.
            let quiet = clocks.min(next - 1);
            if quiet == 0 {
                self.tick_one()?;
                clocks -= 1;
            } else {
                self.tick_quiet(quiet)?;
                clocks -= quiet;
            }
        }
        if clocks != 0 {
            self.tick_one()?;
        }
        Ok(())
    }

    /// No CD or SPU boundary occurs during this span, and DMA/SIO are idle.
    /// GPU/timers still advance one CPU clock at a time inside `Gpu::tick`.
    #[inline]
    fn tick_quiet(&mut self, clocks: u32) -> Result<(), EmuError> {
        self.elapsed_cycles = self.elapsed_cycles.wrapping_add(u64::from(clocks));
        self.cdrom.tick(clocks);
        self.sio.tick(clocks);
        if self.sio.take_irq_edge() {
            self.irq_status |= 1 << 7;
        }
        self.irq_status |= self.gpu.tick(clocks, &mut self.timers);
        self.spu
            .tick_with_cd(clocks, || self.cdrom.audio_sample())?;
        if self.spu.irq_pending() {
            self.irq_status |= 1 << 9;
        }
        if self.cdrom.irq_pending() {
            self.irq_status |= 1 << 2;
        }
        Ok(())
    }

    #[inline]
    fn tick_one(&mut self) -> Result<(), EmuError> {
        self.elapsed_cycles = self.elapsed_cycles.wrapping_add(1);
        self.cdrom.tick(1);
        self.sio.tick(1);
        if self.sio.take_irq_edge() {
            self.irq_status |= 1 << 7;
        }
        self.irq_status |= self.gpu.tick(1, &mut self.timers);
        self.spu.tick_with_cd(1, || self.cdrom.audio_sample())?;
        for index in 0..2 {
            if self.dma[index].control & (1 << 24) != 0 {
                self.run_dma(index)?;
            }
        }
        if self.dma[4].control & (1 << 24) != 0 {
            self.run_dma(4)?;
        }
        if self.spu.irq_pending() {
            self.irq_status |= 1 << 9;
        }
        if self.cdrom.irq_pending() {
            self.irq_status |= 1 << 2;
        }
        if self.dma[3].control & (1 << 24) != 0 && self.cdrom.data_available() > 0 {
            self.run_dma(3)?;
        }
        Ok(())
    }

    pub fn irq_pending(&self) -> bool {
        let levels = (u32::from(self.gpu.irq) << 1)
            | (u32::from(self.cdrom.irq_pending()) << 2)
            | (u32::from(self.spu.irq_pending()) << 9);
        (self.irq_status | levels) & self.irq_mask != 0
    }

    /// Debug reads have no device side effects; unmapped locations return `None`.
    #[inline]
    pub fn peek32(&self, address: u32) -> Option<u32> {
        // Instruction fetches and ordinary RAM words need only one address decode.
        // The byte path below retains mirror/boundary behavior for crossing accesses.
        let physical = physical(address);
        let bytes = match physical {
            a @ 0..=0x007f_fffc if a as usize & (RAM_SIZE - 1) <= RAM_SIZE - 4 => {
                let offset = a as usize & (RAM_SIZE - 1);
                Some(&self.ram[offset..offset + 4])
            }
            a @ 0x1f80_0000..=0x1f80_03fc => {
                let offset = a as usize & 1023;
                Some(&self.scratchpad[offset..offset + 4])
            }
            a @ 0x1fc0_0000..=0x1fc7_fffc => {
                let offset = (a - 0x1fc0_0000) as usize;
                Some(&self.bios[offset..offset + 4])
            }
            _ => None,
        };
        if let Some(bytes) = bytes {
            return Some(u32::from_le_bytes(
                bytes.try_into().expect("word-sized region"),
            ));
        }
        Some(u32::from_le_bytes([
            self.peek8(address)?,
            self.peek8(address.wrapping_add(1))?,
            self.peek8(address.wrapping_add(2))?,
            self.peek8(address.wrapping_add(3))?,
        ]))
    }

    /// Keep normal instruction fetches out of the large device-register decoder.
    #[inline]
    pub(crate) fn fetch_instruction(&mut self, address: u32) -> Result<u32, EmuError> {
        if let Some(word) = self.peek32(address) {
            Ok(word)
        } else {
            self.read32(address)
        }
    }

    fn peek8(&self, address: u32) -> Option<u8> {
        match physical(address) {
            a @ 0..=0x007f_ffff => Some(self.ram[a as usize & (RAM_SIZE - 1)]),
            a @ 0x1f80_0000..=0x1f80_03ff => Some(self.scratchpad[a as usize & 1023]),
            a @ 0x1fc0_0000..=0x1fc7_ffff => Some(self.bios[(a - 0x1fc0_0000) as usize]),
            0x1f00_0000..=0x1f07_ffff => Some(0xff), // No expansion ROM is attached.
            _ => None,
        }
    }

    pub fn read8(&mut self, address: u32) -> Result<u8, EmuError> {
        let dma_address = physical(address);
        if (0x1f80_1080..=0x1f80_10f7).contains(&dma_address) {
            return self
                .read32(dma_address & !3)
                .map(|value| (value >> ((dma_address & 3) * 8)) as u8);
        }
        if (0x1f80_1040..=0x1f80_104f).contains(&physical(address)) {
            return Ok(self.sio.read(address, 1) as u8);
        }
        if physical(address) == 0x1f80_2041 {
            return Ok(0xff);
        } // POST is an external write-only latch.
        if (0x1f80_1800..=0x1f80_1803).contains(&physical(address)) {
            return Ok(self.cdrom.read(address & 3));
        }
        if (0x1f80_1c00..=0x1f80_1fff).contains(&physical(address)) {
            return Ok((self.spu.read(address) >> ((address & 1) * 8)) as u8);
        }
        if let Some(value) = self.peek8(address) {
            return Ok(value);
        }
        Err(EmuError::Bus {
            address,
            operation: "byte read",
        })
    }

    pub fn read16(&mut self, address: u32) -> Result<u16, EmuError> {
        let dma_address = physical(address);
        if (0x1f80_1080..=0x1f80_10f6).contains(&dma_address) {
            return self
                .read32(dma_address & !3)
                .map(|value| (value >> ((dma_address & 3) * 8)) as u16);
        }
        if (0x1f80_1040..=0x1f80_104e).contains(&physical(address)) {
            return Ok(self.sio.read(address, 2) as u16);
        }
        if (0x1f80_1100..=0x1f80_1128).contains(&physical(address)) {
            return self.timers.read(address).ok_or(EmuError::Bus {
                address,
                operation: "timer register read",
            });
        }
        if (0x1f80_1c00..=0x1f80_1ffe).contains(&physical(address)) {
            return Ok(self.spu.read(address));
        }
        if physical(address) == 0x1f80_1802 {
            return Ok(u16::from_le_bytes([
                self.cdrom.read_data(),
                self.cdrom.read_data(),
            ]));
        }
        if matches!(physical(address), 0x1f80_1070 | 0x1f80_1074) {
            return Ok(self.read32(address)? as u16);
        }
        Ok(u16::from_le_bytes([
            self.read8(address)?,
            self.read8(address.wrapping_add(1))?,
        ]))
    }

    pub fn read32(&mut self, address: u32) -> Result<u32, EmuError> {
        if let Some(value) = self.peek32(address) {
            return Ok(value);
        }
        match physical(address) {
            a @ 0x1f80_1040..=0x1f80_104c => Ok(self.sio.read(a, 4)),
            a @ 0x1f80_1000..=0x1f80_1020 if a & 3 == 0 => {
                Ok(self.memory_control[((a - 0x1f80_1000) / 4) as usize])
            }
            0x1f80_1060 => Ok(self.ram_size),
            0x1f80_1070 => Ok(self.irq_status
                | (u32::from(self.gpu.irq) << 1)
                | (u32::from(self.cdrom.irq_pending()) << 2)
                | (u32::from(self.spu.irq_pending()) << 9)),
            0x1f80_1074 => Ok(self.irq_mask),
            a @ 0x1f80_1080..=0x1f80_10ef => {
                let channel = self.dma[((a - 0x1f80_1080) >> 4) as usize];
                match a & 15 {
                    0 => Ok(channel.base),
                    4 => Ok(channel.block),
                    8 => Ok(channel.control),
                    _ => Err(EmuError::Bus {
                        address,
                        operation: "DMA register read",
                    }),
                }
            }
            a @ 0x1f80_1100..=0x1f80_1128 => self.read16(a).map(u32::from),
            0x1f80_10f0 => Ok(self.dma_control),
            0x1f80_10f4 => Ok(self.dma_interrupt),
            0x1f80_1800 => Ok(u32::from_le_bytes([
                self.cdrom.read(0),
                self.cdrom.read(1),
                self.cdrom.read(2),
                self.cdrom.read(3),
            ])),
            0x1f80_1810 => Ok(self.gpu.read()),
            0x1f80_1814 => Ok(self.gpu.status()),
            0x1f80_1820 => Ok(self.mdec.read_data()),
            0x1f80_1824 => Ok(self.mdec.status()),
            0xfffe_0130 => Ok(self.cache_control),
            a @ 0x1f80_1c00..=0x1f80_1ffc => {
                Ok(u32::from(self.spu.read(a)) | (u32::from(self.spu.read(a + 2)) << 16))
            }
            _ => Err(EmuError::Bus {
                address,
                operation: "word read",
            }),
        }
    }

    /// SB keeps the CPU data-bus value: the SPU samples 16 bits on even byte stores.
    pub fn store_byte(&mut self, address: u32, value: u32) -> Result<(), EmuError> {
        if (0x1f80_1c00..=0x1f80_1fff).contains(&physical(address)) {
            if address & 1 == 0 {
                return self.spu.write(address, value as u16);
            }
            return Ok(());
        }
        self.write8(address, value as u8)
    }

    pub fn write8(&mut self, address: u32, value: u8) -> Result<(), EmuError> {
        if (0x1f80_1080..=0x1f80_10f7).contains(&physical(address)) {
            return self.write_dma_lanes(address, u32::from(value), 0xff);
        }
        match physical(address) {
            a @ 0x1f80_1040..=0x1f80_104f => self.sio.write(a, 1, u32::from(value)),
            0x1f80_2041 => self.post = value & 15,
            a @ 0x1f80_1c00..=0x1f80_1fff => return self.store_byte(a, u32::from(value)),
            a @ 0..=0x007f_ffff => self.ram[a as usize & (RAM_SIZE - 1)] = value,
            a @ 0x1f80_0000..=0x1f80_03ff => self.scratchpad[a as usize & 1023] = value,
            a @ 0x1f80_1800..=0x1f80_1803 => {
                self.cdrom.write(a & 3, value);
                if self.cdrom.irq_pending() {
                    self.irq_status |= 4;
                }
                if self.cdrom.data_available() > 0 {
                    self.run_dma(3)?;
                }
            }
            _ => {
                return Err(EmuError::Bus {
                    address,
                    operation: "byte write",
                });
            }
        }
        Ok(())
    }

    pub fn write16(&mut self, address: u32, value: u16) -> Result<(), EmuError> {
        if (0x1f80_1080..=0x1f80_10f6).contains(&physical(address)) {
            return self.write_dma_lanes(address, u32::from(value), 0xffff);
        }
        if (0x1f80_1040..=0x1f80_104e).contains(&physical(address)) {
            self.sio.write(address, 2, u32::from(value));
            return Ok(());
        }
        if (0x1f80_1100..=0x1f80_1128).contains(&physical(address))
            && self.timers.write(address, value)
        {
            return Ok(());
        }
        if (0x1f80_1c00..=0x1f80_1ffe).contains(&physical(address)) {
            return self.spu.write(address, value);
        }
        if matches!(physical(address), 0x1f80_1070 | 0x1f80_1074) {
            return self.write32(address, u32::from(value));
        }
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    pub fn write32(&mut self, address: u32, value: u32) -> Result<(), EmuError> {
        match physical(address) {
            a @ 0x1f80_1040..=0x1f80_104c => self.sio.write(a, 4, value),
            a @ 0..=0x007f_fffc => {
                let offset = a as usize & (RAM_SIZE - 1);
                if offset <= RAM_SIZE - 4 {
                    self.ram[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                } else {
                    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
                        self.write8(address.wrapping_add(offset as u32), byte)?;
                    }
                }
            }
            a @ 0x1f80_0000..=0x1f80_03fc => {
                let offset = a as usize & 1023;
                self.scratchpad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            a @ 0x1f80_1000..=0x1f80_1020 if a & 3 == 0 => {
                self.memory_control[((a - 0x1f80_1000) / 4) as usize] = value;
            }
            0x1f80_1060 => self.ram_size = value,
            0x1f80_1070 => self.irq_status &= value & 0x7ff,
            0x1f80_1074 => self.irq_mask = value & 0x7ff,
            a @ 0x1f80_1080..=0x1f80_10ef => {
                let index = ((a - 0x1f80_1080) >> 4) as usize;
                match a & 15 {
                    0 => self.dma[index].base = value & 0x00ff_ffff,
                    4 => self.dma[index].block = value,
                    8 => {
                        self.dma[index].control = value;
                        if index < 2 {
                            self.mdec_dma[index] = None;
                        }
                        if index == 3 {
                            self.cdrom_dma_resume = None;
                        }
                    }
                    _ => {
                        return Err(EmuError::Bus {
                            address,
                            operation: "DMA register write",
                        });
                    }
                }
                self.run_dma(index)?;
            }
            a @ 0x1f80_1100..=0x1f80_1128 => return self.write16(a, value as u16),
            0x1f80_10f0 => {
                self.dma_control = value;
                for index in 0..7 {
                    self.run_dma(index)?;
                }
            }
            0x1f80_10f4 => {
                self.write_dicr(value, u32::MAX);
            }
            0x1f80_1810 => {
                self.gpu.gp0(value)?;
                if self.gpu.irq {
                    self.irq_status |= 2;
                }
            }
            0x1f80_1814 => self.gpu.gp1(value)?,
            0x1f80_1820 => self.mdec.write_data(value)?,
            0x1f80_1824 => self.mdec.write_control(value),
            0xfffe_0130 => self.cache_control = value,
            a @ 0x1f80_1c00..=0x1f80_1ffc => {
                self.spu.write(a, value as u16)?;
                self.spu.write(a + 2, (value >> 16) as u16)?;
            }
            _ => {
                return Err(EmuError::Bus {
                    address,
                    operation: "word write",
                });
            }
        }
        Ok(())
    }
}
