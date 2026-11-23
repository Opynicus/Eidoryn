//! High-level emulation of GBA BIOS software interrupts at the cartridge ABI.
use crate::{
    BIOS_LEN, Error, Machine, compression,
    cpu::{Bus, Cpu},
};

/// Value reported by `GetBiosChecksum` on a Game Boy Advance and Game Boy Advance SP.
const GBA_BIOS_CHECKSUM: u32 = 0xbaae_187f;

impl Machine {
    pub(super) fn bios_call(&mut self, code: u8) -> Result<(), Error> {
        match code {
            0x00 => self.soft_reset(),
            0x01 => self.memory.reset_ram(self.cpu.regs[0]),
            0x02 | 0x03 => self.memory.halted = true,
            0x04 | 0x05 => {
                let mask = if code == 5 {
                    1
                } else {
                    self.cpu.regs[1] as u16
                };
                if code == 5 || self.cpu.regs[0] != 0 {
                    let flags = self.memory.read16(0x0300_7ff8);
                    self.memory.write16(0x0300_7ff8, flags & !mask);
                }
                self.memory.write16(0x0400_0208, 1);
                self.bios_wait = Some(mask);
            }
            0x06 | 0x07 => {
                let (mut dividend, mut divisor) =
                    (self.cpu.regs[0] as i32, self.cpu.regs[1] as i32);
                if code == 7 {
                    std::mem::swap(&mut dividend, &mut divisor);
                }
                let (quotient, remainder) = if divisor == 0 {
                    (if dividend < 0 { -1 } else { 1 }, dividend)
                } else {
                    (
                        dividend.wrapping_div(divisor),
                        dividend.wrapping_rem(divisor),
                    )
                };
                self.cpu.regs[0] = quotient as u32;
                self.cpu.regs[1] = remainder as u32;
                self.cpu.regs[3] = quotient.unsigned_abs();
            }
            0x08 => self.cpu.regs[0] = self.cpu.regs[0].isqrt(),
            0x09 => self.arc_tan(),
            0x0a => self.arc_tan2(),
            0x0b | 0x0c => self.bios_copy(code == 0x0c),
            0x0d => {
                self.cpu.regs[0] = GBA_BIOS_CHECKSUM;
                self.cpu.regs[1] = 1;
                self.cpu.regs[3] = BIOS_LEN as u32;
            }
            0x0e => self.bg_affine_set(),
            0x0f => self.obj_affine_set(),
            0x10 => {
                let [src, dst, info, ..] = self.cpu.regs;
                (self.cpu.regs[0], self.cpu.regs[1]) =
                    compression::bit_unpack(&mut self.memory, src, dst, info)?;
            }
            0x13 => {
                let [src, dst, ..] = self.cpu.regs;
                (self.cpu.regs[0], self.cpu.regs[1]) =
                    compression::huffman(&mut self.memory, src, dst)?;
            }
            0x11 | 0x12 | 0x14 | 0x15 => {
                let format = if code <= 0x12 {
                    compression::Format::Lz77
                } else {
                    compression::Format::RunLength
                };
                compression::decode(
                    &mut self.memory,
                    format,
                    matches!(code, 0x12 | 0x15),
                    self.cpu.regs[0],
                    self.cpu.regs[1],
                )?;
            }
            0x16..=0x18 => {
                let format = match code {
                    0x16 => compression::DiffFormat::Bytes,
                    0x17 => compression::DiffFormat::BytePairs,
                    _ => compression::DiffFormat::Halfwords,
                };
                let [src, dst, ..] = self.cpu.regs;
                (self.cpu.regs[0], self.cpu.regs[1]) =
                    compression::unfilter(&mut self.memory, format, src, dst)?;
            }
            _ => {
                let (address, instruction_set) = self.cpu.swi_location();
                return Err(Error::UnsupportedBiosCall {
                    code,
                    address,
                    instruction_set,
                });
            }
        }
        Ok(())
    }

    fn soft_reset(&mut self) {
        // SoftReset clears the BIOS work area. The reset selector lies within it.
        let ram_entry = self.memory.read8(0x0300_7ffa) != 0;
        for addr in (0x0300_7e00..0x0300_8000).step_by(4) {
            self.memory.write32(addr, 0);
        }
        self.cpu = Cpu::default();
        let entry = if ram_entry { 0x0200_0000 } else { 0x0800_0000 };
        self.cpu.regs[14] = entry;
        self.cpu.regs[15] = entry;
        self.bios_wait = None;
    }

    fn bios_copy(&mut self, fast: bool) {
        let control = self.cpu.regs[2];
        let mut count = control & 0x1f_ffff;
        if fast {
            count = count.next_multiple_of(8);
        }
        let width = if fast || control & (1 << 26) != 0 {
            4
        } else {
            2
        };
        let mut src = self.cpu.regs[0] & !(width - 1);
        let mut dst = self.cpu.regs[1] & !(width - 1);
        for _ in 0..count {
            if width == 4 {
                self.memory.write32(dst, self.memory.read32(src));
            } else {
                self.memory.write16(dst, self.memory.read16(src));
            }
            dst = dst.wrapping_add(width);
            if control & (1 << 24) == 0 {
                src = src.wrapping_add(width);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_cartridge_can_query_bios_checksum_without_firmware() {
        let mut rom = vec![0; crate::HEADER_LEN];
        rom[..4].copy_from_slice(&0xef0d_0000u32.to_le_bytes()); // SWI GetBiosChecksum
        rom[4..8].copy_from_slice(&0xeaff_fffeu32.to_le_bytes()); // B .
        let mut machine = Machine::from_rom(rom).unwrap();
        machine.run_frame().unwrap();
        assert_eq!(machine.cpu.regs[0], 0xbaae_187f);
        assert_eq!(machine.cpu.regs[1], 1);
        assert_eq!(machine.cpu.regs[3], 0x4000);
    }
}
