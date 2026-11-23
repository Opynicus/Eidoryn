//! Homebrew cartridges invoking the affine BIOS services through real ARM and Thumb instructions.
use eidoryn_gba::{HEADER_LEN, Machine};

#[derive(Clone, Copy, Debug)]
enum Wrappers {
    Arm,
    Thumb,
}

fn cartridge(region: u8, revision: u8, entry: usize, wrappers: Wrappers) -> Vec<u8> {
    let mut rom = vec![0; entry];
    let branch = 0xea00_0000 | ((entry as u32 - 8) / 4);
    rom[..4].copy_from_slice(&branch.to_le_bytes());
    rom[0xa0..0xab].copy_from_slice(b"AFFINE TEST");
    rom[0xac..0xb0].copy_from_slice(&[b'X', b'B', b'T', region]);
    rom[0xb0..0xb2].copy_from_slice(b"00");
    rom[0xb2] = 0x96;
    rom[0xbc] = revision;
    rom[0xbd] = rom[0xa0..=0xbc]
        .iter()
        .fold(0xe7_u8, |n, b| n.wrapping_sub(*b));

    // This position-independent program calls two eight-byte wrappers. Bit 0 of the
    // first wrapper address chooses the instruction set on BX.
    let thumb = u32::from(matches!(wrappers, Wrappers::Thumb));
    for word in [
        0xe28f_7050 | thumb, // ADR r7, wrappers (+ Thumb bit)
        0xe3a0_440e,         // MOV r4, #SRAM
        0xe3e0_0000,         // MVN r0, #0        x = -1
        0xe3a0_1001,         // MOV r1, #1        y = 1
        0xe1a0_e00f,         // MOV lr, pc
        0xe12f_ff17,         // BX r7             ArcTan2
        0xe1a0_0420,         // MOV r0, r0, LSR #8
        0xe5c4_0010,         // STRB r0, [r4, #16]
        0xe28f_0040,         // ADR r0, affine parameters
        0xe3a0_5403,         // MOV r5, #IWRAM
        0xe1a0_1005,         // MOV r1, r5        matrix destination
        0xe3a0_2001,         // MOV r2, #1        one matrix
        0xe287_7008,         // ADD r7, r7, #8
        0xe1a0_e00f,         // MOV lr, pc
        0xe12f_ff17,         // BX r7             BgAffineSet
        0xe3a0_2010,         // MOV r2, #16       copy the resulting matrix to 8-bit SRAM
        0xe4d5_1001,         // LDRB r1, [r5], #1
        0xe4c4_1001,         // STRB r1, [r4], #1
        0xe252_2001,         // SUBS r2, r2, #1
        0x1aff_fffb,         // BNE copy
        0xef02_0000,         // SWI Halt
        0xeaff_fffe,         // B .
    ] {
        rom.extend(word.to_le_bytes());
    }
    for call in [0x0a_u32, 0x0e] {
        match wrappers {
            Wrappers::Arm => {
                rom.extend((0xef00_0000 | (call << 16)).to_le_bytes());
                rom.extend(0xe12f_ff1e_u32.to_le_bytes()); // BX lr
            }
            Wrappers::Thumb => {
                rom.extend((0xdf00 | call as u16).to_le_bytes());
                rom.extend(0x4770_u16.to_le_bytes()); // BX lr
                rom.extend([0; 4]);
            }
        }
    }
    // Texture pivot (0,0), screen pivot (2,3), Q8 scales (1,2), quarter-turn.
    for word in [0_u32, 0, 0x0003_0002, 0x0200_0100, 0x4000] {
        rom.extend(word.to_le_bytes());
    }
    rom
}

#[test]
fn affine_bios_uses_the_same_abi_across_regional_cartridge_layouts() {
    for (region, revision, entry) in [
        (b'E', 0, HEADER_LEN),
        (b'P', 1, 0x240),
        (b'J', 0, 0x100),
        (b'K', 2, 0x180),
    ] {
        for wrappers in [Wrappers::Arm, Wrappers::Thumb] {
            let mut machine =
                Machine::from_rom(cartridge(region, revision, entry, wrappers)).unwrap();
            let case = format!(
                "region {}, revision {revision}, {wrappers:?}",
                char::from(region)
            );
            machine
                .run_frame()
                .unwrap_or_else(|e| panic!("{case}: {e}"));
            let save = machine.sram();
            let coefficient = |i| i16::from_le_bytes([save[i], save[i + 1]]);
            assert_eq!([0, 2, 4, 6].map(coefficient), [0, -256, 512, 0], "{case}");
            assert_eq!(&save[8..12], &768_i32.to_le_bytes(), "{case}");
            assert_eq!(&save[12..16], &(-1024_i32).to_le_bytes(), "{case}");
            assert_eq!(save[16], 0x60, "135-degree angle: {case}");
        }
    }
}
