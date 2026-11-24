use std::path::Path;

/// Mode 3 cartridge with two observable pixels. Pixel 0 reflects keypad A, and
/// pixel 1 encodes a boot counter held in battery-backed SRAM.
pub fn write_cartridge(path: &Path) {
    let mut rom = vec![0; eidoryn_gba::HEADER_LEN];
    rom[..4].copy_from_slice(&0xea00_002e_u32.to_le_bytes()); // branch past the header
    rom[0xa0..0xac].copy_from_slice(b"RUNNER PROBE");
    rom[0xac..0xb0].copy_from_slice(b"TSTE");
    let code: [u32; 18] = [
        0xe3a0_0301, // mov r0,#0x04000000 (I/O)
        0xe3a0_1b01, // mov r1,#0x400 (BG2 on)
        0xe381_1003, // orr r1,r1,#3
        0xe1c0_10b0, // strh r1,[r0] (DISPCNT)
        0xe3a0_2406, // mov r2,#0x06000000 (VRAM)
        0xe3a0_340e, // mov r3,#0x0e000000 (SRAM)
        0xe5d3_5000, // ldrb r5,[r3]
        0xe285_5001, // add r5,r5,#1
        0xe5c3_5000, // strb r5,[r3]
        0xe205_501f, // and r5,r5,#31
        0xe1c2_50b2, // strh r5,[r2,#2] (boot count)
        0xe280_4e13, // add r4,r0,#0x130 (KEYINPUT)
        0xe1d4_60b0, // loop: ldrh r6,[r4]
        0xe316_0001, // tst r6,#1
        0x03a0_701f, // moveq r7,#0x001f (red if A pressed)
        0x13a0_7b1f, // movne r7,#0x7c00 (blue if A released)
        0xe1c2_70b0, // strh r7,[r2]
        0xeaff_fff9, // b loop
    ];
    rom.extend(code.into_iter().flat_map(u32::to_le_bytes));
    std::fs::write(path, rom).unwrap();
}
