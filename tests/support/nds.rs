//! An original DS cartridge. ARM9 draws both LCDs and polls A, while ARM7
//! supplies the lower screen's color through shared RAM. No firmware or SDK.
use std::path::Path;

#[derive(Default)]
struct Program {
    words: Vec<u32>,
    literals: Vec<(usize, u32)>,
}

impl Program {
    fn load(&mut self, reg: u32, value: u32) {
        self.literals.push((self.words.len(), value));
        self.words.push(0xe59f_0000 | reg << 12); // LDR Rd, [PC, #literal]
    }

    fn branch_back(&mut self, condition: u32, target: usize) {
        let distance = target as i32 - self.words.len() as i32 - 2;
        self.words
            .push(condition << 28 | 0x0a00_0000 | (distance as u32 & 0x00ff_ffff));
    }

    // Read the upper seven bits of a 12-bit sample over the ARM7 touch SPI bus.
    fn sample_axis(&mut self, channel: u32, destination: u32) {
        self.words.extend([
            0xe3a0_5000 | channel, // MOV r5, #command
            0xe5c3_5002,           // STRB r5, [r3, #2]
            0xe3a0_5000,           // MOV r5, #0
            0xe5c3_5002,           // Clock out the high byte
            0xe5d3_0002 | destination << 12,
            0xe5c3_5002, // Clock out the low byte
        ]);
    }

    fn finish(mut self) -> Vec<u8> {
        for (instruction, value) in self.literals {
            let offset = 4 * (self.words.len() - (instruction + 2));
            self.words[instruction] |= u32::try_from(offset).unwrap();
            self.words.push(value);
        }
        self.words.into_iter().flat_map(u32::to_le_bytes).collect()
    }
}

pub fn cartridge() -> Vec<u8> {
    let mut nine = Program::default();
    nine.load(0, 0x0400_0000);
    nine.load(1, 0x0400_1000);
    nine.load(2, 1 << 16); // Enable normal display on each engine.
    nine.words.extend([0xe580_2000, 0xe581_2000]);
    nine.load(0, 0x0500_0000); // Main backdrop palette
    nine.load(1, 0x0500_0400); // Sub backdrop palette
    nine.load(2, 0x0400_0130); // Active-low keypad
    nine.load(3, 0x0201_0000); // Shared mailbox
    nine.load(4, 0x001f); // Red
    nine.load(5, 0x03e0); // Green
    nine.words.extend([
        0xe1d2_60b0, // LDRH r6, [r2]
        0xe316_0001, // TST r6, #1
        0x01c0_50b0, // STRHEQ r5, [r0]
        0x11c0_40b0, // STRHNE r4, [r0]
        0xe593_6000, // LDR r6, [r3]
        0xe1c1_60b0, // STRH r6, [r1]
        0xeaff_fff8, // B poll
    ]);
    let mut seven = Program::default();
    seven.load(0, 0x0201_0000);
    seven.load(1, 0x7c00); // Blue
    seven.load(2, 0x0400_0136); // ARM7 external keys and pen-down flag
    seven.load(3, 0x0400_01c0); // SPI control, data at +2
    seven.load(4, 0x8a00); // Touch ADC, enable, hold chip select
    let poll = seven.words.len();
    seven.words.extend([
        0xe1d2_50b0, // LDRH r5, [r2]
        0xe315_0040, // TST r5, #PENIRQ
        0x1580_1000, // STRNE r1, [r0] (blue while the stylus is up)
    ]);
    seven.branch_back(1, poll); // BNE
    seven.words.push(0xe1c3_40b0); // STRH r4, [r3]
    seven.sample_axis(0xd0, 6); // X
    seven.sample_axis(0x90, 7); // Y
    seven.words.extend([
        0xe1a0_6126, // MOV r6, r6, LSR #2 (5-bit red)
        0xe1a0_7127, // MOV r7, r7, LSR #2 (5-bit green)
        0xe186_6287, // ORR r6, r6, r7, LSL #5
        0xe580_6000, // STR r6, [r0]
    ]);
    seven.branch_back(14, poll); // B

    let mut rom = vec![0; 0xa000];
    rom[..11].copy_from_slice(b"DUAL SCREEN");
    rom[12..18].copy_from_slice(b"EIDE00");
    for (header, offset, base, program) in [
        (0x20, 0x4000, 0x0200_0000u32, nine.finish()),
        (0x30, 0x9000, 0x0380_0000u32, seven.finish()),
    ] {
        let descriptor = [offset as u32, base, base, program.len() as u32];
        for (i, word) in descriptor.into_iter().enumerate() {
            let at = header + 4 * i;
            rom[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        rom[offset..offset + program.len()].copy_from_slice(&program);
    }
    rom[0x80..0x84].copy_from_slice(&0xa000u32.to_le_bytes());
    rom[0x84..0x88].copy_from_slice(&0x200u32.to_le_bytes());
    rom
}

pub fn write_cartridge(path: &Path) {
    std::fs::write(path, cartridge()).unwrap();
}
