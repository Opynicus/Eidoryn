//! Read-only snapshots of the ARM7TDMI, instruction stream and LCD.
use crate::{InstructionSet, Machine, cpu::Bus};

#[derive(Debug)]
pub struct Instruction {
    pub address: u32,
    pub opcode: u32,
    pub bytes: u32,
    pub assembly: String,
}

#[derive(Debug)]
pub struct DebugSnapshot {
    pub registers: [u32; 16],
    pub cpsr: u32,
    pub instruction_set: InstructionSet,
    pub instructions: [Instruction; 9],
    /// Four consecutive 32-bit words starting at the aligned stack pointer.
    pub stack: [u32; 4],
    pub display_control: u16,
    pub display_status: u16,
    pub scanline: u16,
}

impl Machine {
    /// Captures the next instructions using the current instruction set. Reading a snapshot
    /// does not update instruction prefetch state, advance the LCD, or acknowledge interrupts.
    pub fn debug_snapshot(&self) -> DebugSnapshot {
        let registers = self.cpu.regs;
        let mut address = registers[15];
        let instruction_set = self.cpu.instruction_set();
        let options = unarm::Options {
            version: unarm::Version::V4T,
            ..Default::default()
        };
        let instructions = std::array::from_fn(|_| {
            let (word, decoded, bytes) = match instruction_set {
                InstructionSet::Arm => {
                    let word = self.memory.peek32(address);
                    (word, unarm::parse_arm(word, address, &options), 4)
                }
                InstructionSet::Thumb => {
                    // A long branch can start at address 4n + 2.
                    let low = u32::from(self.memory.peek16(address));
                    let word = low | (u32::from(self.memory.peek16(address.wrapping_add(2))) << 16);
                    let (decoded, bytes) = unarm::parse_thumb(word, address, &options);
                    (if bytes == 2 { low } else { word }, decoded, bytes)
                }
            };
            // ARM7 executes Thumb BL in two steps. At its suffix the LR bank already
            // contains the precomputed base, even if a branch entered the suffix directly.
            let assembly = if address == registers[15]
                && instruction_set == InstructionSet::Thumb
                && word & 0xf800 == 0xf800
            {
                let target = registers[14].wrapping_add((word & 0x07ff) << 1);
                format!("bl 0x{target:08x}")
            } else {
                decoded.display(&options).to_string()
            };
            let instruction = Instruction {
                address,
                opcode: word,
                bytes,
                assembly,
            };
            address = address.wrapping_add(bytes);
            instruction
        });
        DebugSnapshot {
            registers,
            cpsr: self.cpu.status(),
            instruction_set,
            instructions,
            stack: std::array::from_fn(|i| {
                self.memory.peek32(registers[13].wrapping_add(i as u32 * 4))
            }),
            display_control: self.memory.read16(0x0400_0000),
            display_status: self.memory.read16(0x0400_0004),
            scanline: self.memory.line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HEADER_LEN;

    #[test]
    fn capture_and_single_step_follow_arm_to_thumb_with_a_long_branch() {
        let mut rom = vec![0; HEADER_LEN];
        rom[..4].copy_from_slice(&0xe12f_ff10_u32.to_le_bytes()); // bx r0
        rom[4..6].copy_from_slice(&0x2103_u16.to_le_bytes()); // movs r1, #3
        rom[6..10].copy_from_slice(&[0x00, 0xf0, 0x00, 0xf8]); // bl
        let mut machine = Machine::from_rom(rom).unwrap();
        machine.cpu.regs[0] = 0x0800_0005;
        let arm = machine.debug_snapshot();
        assert_eq!(arm.instruction_set, InstructionSet::Arm);
        assert_eq!(arm.instructions[0].assembly, "bx r0");
        assert!(!machine.cartridge_started());
        machine.step().unwrap();
        let thumb = machine.debug_snapshot();
        assert_eq!(thumb.instruction_set, InstructionSet::Thumb);
        assert_eq!(thumb.instructions[0].address, 0x0800_0004);
        assert_eq!(thumb.instructions[0].opcode, 0x2103);
        assert_eq!(thumb.instructions[0].bytes, 2);
        assert!(thumb.instructions[0].assembly.starts_with("mov"));
        assert_eq!(thumb.instructions[1].bytes, 4);
        assert!(thumb.instructions[1].assembly.starts_with("bl "));
        assert_eq!(thumb.instructions[2].address, 0x0800_000a);
        machine.step().unwrap();
        assert_eq!(machine.debug_snapshot().registers[1], 3);
        machine.step().unwrap();
        let suffix = &machine.debug_snapshot().instructions[0];
        assert_eq!(suffix.address, 0x0800_0008);
        assert_eq!(suffix.assembly, "bl 0x0800000a");
        machine.step().unwrap();
        assert_eq!(machine.debug_snapshot().registers[15], 0x0800_000a);
    }

    #[test]
    fn inspection_reads_firmware_without_changing_protection_or_fetch_width() {
        let mut bios = vec![0; crate::BIOS_LEN];
        let opcode = 0xe3a0_1001_u32;
        bios[..4].copy_from_slice(&opcode.to_le_bytes());
        let mut machine = Machine::with_bios(vec![0; HEADER_LEN], bios).unwrap();
        let protected = machine.memory.read32(0);
        assert_ne!(protected, opcode);
        let snapshot = machine.debug_snapshot();
        assert_eq!(snapshot.instructions[0].opcode, opcode);
        assert_eq!(machine.memory.read32(0), protected);

        // The 8-bit SRAM bus broadcasts the first byte across a CPU word fetch.
        machine.cpu.regs[15] = 0x0e00_0000;
        for (offset, value) in [1, 2, 3, 4].into_iter().enumerate() {
            machine.memory.write8(0x0e00_0000 + offset as u32, value);
        }
        assert_eq!(machine.debug_snapshot().instructions[0].opcode, 0x0101_0101);
    }

    #[test]
    fn halted_step_advances_the_lcd_without_executing_an_instruction() {
        let mut machine = Machine::from_rom(vec![0; HEADER_LEN]).unwrap();
        machine.memory.halted = true;
        let pc = machine.cpu.regs[15];
        machine.step().unwrap();
        assert_eq!(machine.cpu.regs[15], pc);
        assert_ne!(machine.memory.cycle, 0);
        machine.step().unwrap();
        assert_eq!(machine.debug_snapshot().scanline, 1);
    }
}
