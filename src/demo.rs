//! Original BIOS-free MIPS program. Every visible pixel comes from guest GP0 writes.
//! RAM[0x100] is a frame-completion counter used only by the demo's host pacing.

pub const ENTRY: u32 = 0x8001_0000;
pub const FRAME_COUNTER: usize = 0x100;

struct Assembler {
    words: Vec<u32>,
}

impl Assembler {
    fn li(&mut self, register: u32, value: u32) {
        self.words
            .push(0x3c00_0000 | (register << 16) | (value >> 16));
        self.words
            .push(0x3400_0000 | (register << 21) | (register << 16) | (value & 0xffff));
    }

    fn gpu(&mut self, value: u32) {
        self.li(9, value); // t1 = command word, t0 = GPU port.
        self.words.push(0xad09_0000); // sw t1, 0(t0)
    }

    fn vertex(&mut self, x: u32, y: u32) {
        self.li(9, x | (y << 16));
        self.words.push((9 << 21) | (17 << 16) | (9 << 11) | 0x21); // addu t1,t1,s1
        self.words.push(0xad09_0000);
    }

    fn rectangle(&mut self, color: u32, x: u32, y: u32, w: u32, h: u32) {
        self.gpu(0x6000_0000 | color);
        self.gpu(x | (y << 16));
        self.gpu(w | (h << 16));
    }
}

#[allow(clippy::decimal_bitwise_operands)] // Pixel positions/sizes are decimal for readability.
pub fn program() -> Vec<u32> {
    let mut a = Assembler { words: Vec::new() };
    a.li(8, 0x1f80_1814);
    a.gpu(0x0000_0000); // GP1 reset.
    a.gpu(0x0800_0001); // 320 x 240, 15-bit.
    a.gpu(0x0300_0000); // Display enabled.
    a.li(8, 0x1f80_1810);
    a.gpu(0xe300_0000);
    a.gpu(0xe400_0000 | 319 | (239 << 10));
    a.li(16, 0);
    let start = a.words.len();
    a.words.push(0x2610_0001); // addiu s0,s0,1
    a.words.push(0x3211_007f); // andi s1,s0,127
    a.words.push(0x2631_0028); // addiu s1,s1,40
    a.gpu(0x0225_1710); // Fill with dark blue (command colors are BGR).
    a.gpu(0);
    a.gpu(320 | (240 << 16));
    a.rectangle(0x0030_251b, 0, 154, 320, 86);
    for y in [154, 162, 175, 196, 227] {
        a.rectangle(0x004a_3c27, 0, y, 320, 1);
    }
    for x in (0..320).step_by(32) {
        a.rectangle(0x003b_3022, x, 154, 1, 86);
    }
    // Decorative status bars, also emitted by guest instructions.
    a.rectangle(0x00bd_c965, 16, 18, 26, 3);
    a.rectangle(0x0076_6243, 48, 18, 50, 3);
    a.rectangle(0x0076_6243, 16, 25, 67, 2);
    a.rectangle(0x0076_6243, 222, 219, 80, 2);
    a.rectangle(0x00bd_c965, 286, 210, 16, 3);
    // Two Gouraud-shaded triangles form a moving faceted crystal.
    a.gpu(0x30dd_f39a);
    a.vertex(56, 49);
    a.gpu(0x009c_ae33);
    a.vertex(4, 119);
    a.gpu(0x006a_471e);
    a.vertex(56, 188);
    a.gpu(0x30d8_efff);
    a.vertex(56, 49);
    a.gpu(0x006a_471e);
    a.vertex(56, 188);
    a.gpu(0x005b_86ef);
    a.vertex(108, 119);
    a.words.push(0xac10_0100); // sw s0,0x100(zero): complete frame.
    a.words
        .push(0x0800_0000 | (((ENTRY + start as u32 * 4) >> 2) & 0x03ff_ffff));
    a.words.push(0); // Branch delay slot.
    a.words
}

pub fn executable() -> Vec<u8> {
    executable_at(program(), ENTRY)
}

/// Original guest that reads ISO sector 16 through CD-ROM commands and DMA3,
/// checks the ISO signature in RAM, then draws the demo. No BIOS calls are made.
pub fn disc_probe_executable() -> Vec<u8> {
    let mut words = program();
    let last_jump = words.len() - 2;
    words[last_jump] = 0x0800_0000 | ((ENTRY + last_jump as u32 * 4) >> 2 & 0x03ff_ffff);
    words.resize(1024, 0);
    let entry = ENTRY + words.len() as u32 * 4;
    let mut a = Assembler { words };
    a.li(8, 0x1f80_1800);
    let byte = |a: &mut Assembler, offset: u32, value: u32| {
        a.li(9, value);
        a.words.push(0xa109_0000 | offset); // sb t1,offset(t0)
    };
    let wait_irq = |a: &mut Assembler, kind| {
        byte(a, 0, 1);
        a.li(9, kind);
        let start = a.words.len();
        a.words.extend([0x910a_0003, 0, 0x314a_0007]); // lbu t2,3(t0); nop; andi t2,7
        let offset = start as i32 - (a.words.len() as i32 + 1);
        a.words.extend([0x1549_0000 | u32::from(offset as u16), 0]); // bne t2,t1,poll
        a.words.extend([0x910a_0001, 0]); // drain status response
        byte(a, 3, 0x1f);
        byte(a, 0, 0);
    };
    byte(&mut a, 0, 1);
    byte(&mut a, 2, 0x1f);
    byte(&mut a, 3, 0x5f);
    byte(&mut a, 0, 0);
    byte(&mut a, 2, 0x80);
    byte(&mut a, 1, 0x0e);
    wait_irq(&mut a, 3); // double speed
    for value in [0, 2, 0x16] {
        byte(&mut a, 2, value);
    }
    byte(&mut a, 1, 2);
    wait_irq(&mut a, 3); // Setloc 00:02:16, LBA 16
    byte(&mut a, 1, 6);
    wait_irq(&mut a, 3); // ReadN acknowledgement
    // Arm DMA before data arrives, exercising the DREQ wait path.
    for (address, value) in [
        (0x1f80_10f0, 0x8000),
        (0x1f80_10f4, 0x0088_0000),
        (0x1f80_10b0, 0x20000),
        (0x1f80_10b4, 0x200),
        (0x1f80_10b8, 0x1100_0000),
    ] {
        a.li(10, address);
        a.li(9, value);
        a.words.push(0xad49_0000); // sw t1,0(t2)
    }
    wait_irq(&mut a, 1);
    byte(&mut a, 3, 0x80); // BFRD: transfer staged sector to data FIFO / DMA
    byte(&mut a, 1, 9);
    wait_irq(&mut a, 3);
    wait_irq(&mut a, 2); // Pause
    a.li(10, 0x8002_0000);
    a.words.extend([0x8d4b_0000, 0]); // lw t3,0(t2); load delay
    a.li(12, 0x3044_4301); // PVD starts 01 'C' 'D' '0'
    let check = a.words.len();
    a.words.extend([0, 0]); // patched bne t3,t4,failure; delay slot
    a.li(9, 0x4344_524d);
    a.words.push(0xac09_0300); // guest success marker: RAM[0x300] = 'CDRM'
    a.words
        .extend([0x0800_0000 | (ENTRY >> 2 & 0x03ff_ffff), 0]);
    let failure = a.words.len();
    a.words[check] = 0x156c_0000 | (failure - check - 1) as u32;
    a.words.extend([
        0x0800_0000 | ((ENTRY + failure as u32 * 4) >> 2 & 0x03ff_ffff),
        0,
    ]);
    executable_at(a.words, entry)
}

fn executable_at(words: Vec<u32>, entry: u32) -> Vec<u8> {
    let mut payload: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    payload.resize((payload.len() + 0x7ff) & !0x7ff, 0);
    let mut exe = vec![0; 0x800];
    exe[..8].copy_from_slice(b"PS-X EXE");
    for (offset, value) in [
        (0x10, entry),
        (0x18, ENTRY),
        (0x1c, payload.len() as u32),
        (0x30, 0x801f_ff00),
    ] {
        exe[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    exe.extend_from_slice(&payload);
    exe
}
