#[path = "../../../tests/support/gba_firmware.rs"]
mod gba_firmware;

use eidoryn_gba::{BIOS_LEN, Error, HEADER_LEN, Machine};
use gba_firmware::firmware;

fn cartridge() -> Vec<u8> {
    let mut rom = vec![0; HEADER_LEN];
    rom[..4].copy_from_slice(&0xea00_002e_u32.to_le_bytes()); // B ROM+0xc0
    for word in [
        0xe3a0_440e_u32, // MOV r4, #SRAM
        0xef7f_0000,     // SWI 0x7f (test firmware service)
        0xe5c4_0000,     // STRB r0, [r4]
        0xe284_7080,     // ADD r7, r4, #0x80 (firmware markers)
        0xe3a0_5301,     // MOV r5, #IO
        0xe3a0_6008,     // MOV r6, #8
        0xe1c5_60b4,     // STRH r6, [r5, #4] (enable VBlank at the LCD)
        0xe285_5c02,     // ADD r5, r5, #0x200
        0xe3a0_6001,     // MOV r6, #1
        0xe1c5_60b0,     // STRH r6, [r5] (IE)
        0xe1c5_60b8,     // STRH r6, [r5, #8] (IME)
        0xe28f_3001,     // ADD r3, pc, #1
        0xe12f_ff13,     // BX r3
    ] {
        rom.extend(word.to_le_bytes());
    }
    for op in [
        0xdf7e_u16, // SWI 0x7e
        0x7060,     // STRB r0, [r4, #1]
        0x787d,     // loop: LDRB r5, [r7, #1] (read firmware IRQ counter)
        0x70a5,     // STRB r5, [r4, #2] (returning to Thumb must resume this loop)
        0xe7fc,     // B loop
    ] {
        rom.extend(op.to_le_bytes());
    }
    rom
}

#[test]
fn supplied_firmware_handles_arm_thumb_and_irq_and_runs_again_on_reset() {
    let mut machine = Machine::with_bios(cartridge(), firmware()).unwrap();
    for boot in 0..=1 {
        assert!(!machine.cartridge_started(), "CPU starts in firmware");
        for frame in 0..3 {
            machine.run_frame().unwrap();
            assert!(
                machine.cartridge_started(),
                "firmware handed off to the cartridge"
            );
            let save = machine.sram();
            assert_eq!(save[0x80], boot, "firmware reset counter");
            assert_eq!(
                save[0x83], 0xd3,
                "reset in ARM supervisor mode with IRQ/FIQ masked"
            );
            assert_eq!(
                save[..2],
                [0xa1, 0x72],
                "ARM and Thumb SWIs returned to their callers"
            );
            assert_eq!(
                save[0x84], 0x93,
                "SWI entered supervisor mode with IRQ masked"
            );
            assert_eq!(
                save[0x82], 0x3f,
                "IRQ preserved the Thumb system-mode caller"
            );
            let irq_count = boot * 3 + frame;
            assert_eq!(save[0x81], irq_count, "one firmware IRQ per VBlank");
            assert_eq!(save[2], irq_count, "cartridge resumed in Thumb after IRQ");
        }
        machine.reset();
    }
}

#[test]
fn rejects_incorrect_lengths_and_erased_firmware() {
    for size in [0, BIOS_LEN - 1, BIOS_LEN + 1, 512 * 1024] {
        assert!(matches!(
            Machine::with_bios(cartridge(), vec![0; size]),
            Err(Error::InvalidBiosSize(n)) if n == size
        ));
    }
    for erased in [0, 0xff] {
        assert!(matches!(
            Machine::with_bios(cartridge(), vec![erased; BIOS_LEN]),
            Err(Error::BlankBios)
        ));
    }
}
