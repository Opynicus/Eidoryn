use eidoryn_gba::{Error, HEADER_LEN, HEIGHT, InstructionSet, Machine, WIDTH};

/// Own ARM/Thumb cartridge. ARM enables direct-color mode and enters Thumb.
/// The Thumb loop reads KEYINPUT and stores the selected color in VRAM and SRAM.
fn input_cartridge() -> Vec<u8> {
    let mut rom = vec![0; HEADER_LEN];
    rom[..4].copy_from_slice(&0xea00_002eu32.to_le_bytes()); // B 0x080000c0
    for word in [
        0xe3a0_0301u32, // MOV r0, #0x04000000
        0xe59f_102c,    // LDR r1, =0x0403
        0xe1c0_10b0,    // STRH r1, [r0]
        0xe3a0_2406,    // MOV r2, #0x06000000
        0xe59f_4024,    // LDR r4, =KEYINPUT
        0xe3a0_540e,    // MOV r5, #0x0e000000
        0xe28f_6001,    // ADD r6, pc, #1
        0xe12f_ff16,    // BX r6
    ] {
        rom.extend(word.to_le_bytes());
    }
    for instruction in [
        0x8823u16, 0x2101, 0x420b, 0xd002, // LDRH; MOVS #1; TST; BEQ red
        0x231f, 0x029b, 0xe000, 0x231f, // blue = 31 << 10, red = 31
        0x8013, 0x702b, 0xe7f4, 0x0000, // STRH; STRB; B loop; alignment
    ] {
        rom.extend(instruction.to_le_bytes());
    }
    rom.extend(0x0403u32.to_le_bytes());
    rom.extend(0x0400_0130u32.to_le_bytes());
    rom
}

#[test]
fn cartridge_polls_keypad_and_changes_pixels_and_save_ram() {
    let mut machine = Machine::from_rom(input_cartridge()).unwrap();
    machine.run_frame().unwrap();
    assert_eq!(machine.pixels().len(), WIDTH * HEIGHT * 3);
    assert_eq!(&machine.pixels()[..3], &[0, 0, 255]);
    machine.set_buttons(1);
    machine.run_frame().unwrap();
    assert_eq!(&machine.pixels()[..3], &[255, 0, 0]);
    assert_eq!(machine.sram()[0], 31);
    assert!(machine.take_sram_dirty());
    machine.run_frame().unwrap();
    assert!(!machine.take_sram_dirty());
    let saved = machine.sram().to_vec();
    machine.reset();
    assert_eq!(machine.sram(), saved);
    let mut fresh = Machine::from_rom(input_cartridge()).unwrap();
    assert!(matches!(
        fresh.restore_sram(&[]),
        Err(Error::InvalidSramSize(0))
    ));
    fresh.restore_sram(&saved).unwrap();
    assert_eq!(fresh.sram(), saved);
    assert!(!fresh.take_sram_dirty());
}

#[test]
fn validates_cartridge_size() {
    assert!(matches!(
        Machine::from_rom(vec![0; 16]),
        Err(Error::InvalidRomSize(16))
    ));
}

#[test]
fn unsupported_bios_call_reports_the_guest_instruction_in_both_encodings() {
    let arm = [0xef7f_0000u32, 0, 0];
    let thumb = [0xe28f_0001, 0xe12f_ff10, 0x0000_df7f]; // ADD pc+1, BX r0, SWI 7f
    for (opcodes, pc, encoding) in [
        (arm, 0x0800_0000, InstructionSet::Arm),
        (thumb, 0x0800_0008, InstructionSet::Thumb),
    ] {
        let mut rom = vec![0; HEADER_LEN];
        for (dst, op) in rom.as_chunks_mut::<4>().0.iter_mut().zip(opcodes) {
            *dst = op.to_le_bytes();
        }
        let mut machine = Machine::from_rom(rom).unwrap();
        assert!(matches!(
            machine.run_frame(),
            Err(Error::UnsupportedBiosCall { code: 0x7f, address, instruction_set })
                if (address, instruction_set) == (pc, encoding)
        ));
    }
}

fn vblank_counter_rom() -> Vec<u8> {
    // The cartridge installs a handler, enables the LCD interrupt and waits
    // with SWI 05. Its handler acknowledges IF and sets the BIOS work-RAM flag.
    let mut rom = vec![0; HEADER_LEN];
    rom[..4].copy_from_slice(&0xea00_002eu32.to_le_bytes());
    for word in [
        0xe3a0_0301u32,
        0xe28f_1030,
        0xe59f_2044,
        0xe582_1000, // install IRQ vector
        0xe3a0_3008,
        0xe1c0_30b4,
        0xe280_0c02,
        0xe3a0_3001,
        0xe1c0_30b0, // DISPSTAT and IE
        0xe3a0_540e,
        0xe3a0_4000, // SRAM base and frame count
        0xef05_0000,
        0xe284_4001,
        0xe5c5_4000,
        0xeaff_fffb, // wait, increment, store, loop
        0xe280_0c02,
        0xe3a0_1001,
        0xe1c0_10b2, // acknowledge VBlank IF
        0xe59f_2008,
        0xe1c2_10b0,
        0xe12f_ff1e, // set VBlank BIOS flag, BX LR
        0x0300_7ffc,
        0x0300_7ff8,
    ] {
        rom.extend(word.to_le_bytes());
    }
    rom
}

#[test]
fn vblank_wait_dispatches_the_cartridge_handler_once_per_frame() {
    let mut machine = Machine::from_rom(vblank_counter_rom()).unwrap();
    for frame in 1..=3 {
        machine.run_frame().unwrap();
        assert_eq!(machine.sram()[0], frame);
    }
}

#[test]
fn debugger_snapshot_inside_hle_irq_restores_wait_and_banked_return_state() {
    let mut live = Machine::from_rom(vblank_counter_rom()).unwrap();
    // SWI 05 sleeps until VBlank, then the HLE dispatcher enters the cartridge's IRQ handler.
    for _ in 0..400 {
        live.step().unwrap();
        if live.debug_snapshot().cpsr & 0x1f == 0x12 {
            break;
        }
    }
    assert_eq!(live.debug_snapshot().cpsr & 0x1f, 0x12);
    let mut restored = Machine::from_rom(vblank_counter_rom()).unwrap();
    restored.load_state(&live.save_state().unwrap()).unwrap();
    for machine in [&mut live, &mut restored] {
        machine.run_frame().unwrap();
        assert_eq!(machine.battery_save()[0], 1);
    }
    assert_eq!(live.save_state().unwrap(), restored.save_state().unwrap());
}
