use super::*;

fn cartridge(nine: &[u32], seven: &[u32]) -> Vec<u8> {
    let mut data = vec![0; HEADER_LEN + (nine.len() + seven.len()) * 4];
    data[..9].copy_from_slice(b"HOME TEST");
    data[12..16].copy_from_slice(b"NDSJ");
    let mut pos = HEADER_LEN;
    for (hdr, base, ops) in [(0x20, 0x0200_0000u32, nine), (0x30, 0x0380_0000, seven)] {
        for (i, n) in [pos as u32, base, base, (4 * ops.len()) as u32]
            .into_iter()
            .enumerate()
        {
            let p = hdr + i * 4;
            data[p..p + 4].copy_from_slice(&n.to_le_bytes());
        }
        for op in ops {
            data[pos..pos + 4].copy_from_slice(&op.to_le_bytes());
            pos += 4;
        }
    }
    data
}
const LOOP: u32 = 0xeaff_fffe;

#[test]
fn header_only_probe_checks_both_descriptors() {
    let mut rom = cartridge(&[LOOP], &[LOOP]);
    let header = Header::parse(&rom[..HEADER_LEN], rom.len() as u64).unwrap();
    assert_eq!(header.title(), "HOME TEST");
    assert_eq!(header.game_code(), Some("NDSJ"));
    rom[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Header::parse(&rom, rom.len() as u64).is_err());
    assert!(Machine::from_rom(rom).is_err());
}

#[test]
fn arm9_divides_via_bios_and_arm7_executes_thumb_from_its_private_ram() {
    let mut rom = cartridge(
        &[0xe3a0_002b, 0xe3a0_1007, 0xef09_0000, LOOP],
        &[0xe7fe_2206],
    );
    // MOV r0,#43; MOV r1,#7; SWI Div; B .
    rom[0x34..0x38].copy_from_slice(&0x0380_0001u32.to_le_bytes());
    let mut m = Machine::from_rom(rom).unwrap();
    m.step().unwrap();
    m.step().unwrap();
    assert_eq!(&m.system.cpus[0].regs[..2], &[6, 1]);
    assert_eq!(m.system.cpus[1].regs[2], 6);
    assert_eq!(m.pcs(), (0x0200_000c, 0x0380_0002));
}

#[test]
fn hardware_input_dma_ipc_and_touch_use_the_correct_cpu_views() {
    let mut m = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    m.set_touch(Some((93, 179)));
    m.set_buttons(0x401);
    let mut nine = Port::<true> {
        m: &mut m.system.memory,
        rom: &m.rom,
    };
    assert_eq!(nine.read16(0x0400_0130), 0x3fe);
    nine.write32(0x0200_1000, 0xabc0_0123);
    nine.write32(0x0400_00b0, 0x0200_1000);
    nine.write32(0x0400_00b4, 0x0201_2340);
    nine.write32(0x0400_00b8, 0xc400_0001);
    nine.dma();
    assert_eq!(nine.read32(0x0201_2340), 0xabc0_0123);
    assert_eq!(nine.read32(0x0400_0214), 1 << 8);
    nine.write16(0x0400_0184, 0x8000);
    nine.write32(0x0400_0188, 42);
    let mut seven = Port::<false> {
        m: &mut m.system.memory,
        rom: &m.rom,
    };
    seven.write16(0x0400_0184, 0x8400);
    assert_eq!(seven.read32(0x0410_0000), 42);
    assert_eq!(seven.read32(0x0400_0214), 1 << 18);
    assert_eq!(seven.read8(0x0400_0136) & 0x43, 2);
    seven.write16(0x0400_01c0, 0x8a00);
    seven.write8(0x0400_01c2, 0xd0);
    seven.write8(0x0400_01c2, 0);
    let hi = seven.read8(0x0400_01c2);
    seven.write8(0x0400_01c2, 0);
    let lo = seven.read8(0x0400_01c2);
    assert_eq!((u16::from(hi) << 5) | (u16::from(lo) >> 3), 0x100 + 93 * 14);
}

#[test]
fn snapshot_roundtrip_preserves_partial_frame_and_rejects_other_games() {
    let rom = cartridge(&[0xe280_0001, 0xeaff_fffd], &[LOOP]);
    let mut m = Machine::from_rom(rom).unwrap();
    m.step().unwrap();
    let state = m.save_state().unwrap();
    let pcs = m.pcs();
    for _ in 0..20 {
        m.step().unwrap();
    }
    let r = m.system.cpus[0].regs[0];
    m.load_state(&state).unwrap();
    assert_eq!(m.pcs(), pcs);
    for _ in 0..20 {
        m.step().unwrap();
    }
    assert_eq!(m.system.cpus[0].regs[0], r);
    let mut other = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    assert!(other.load_state(&state).is_err());
    let mut damaged = state;
    *damaged.last_mut().unwrap() ^= 0xff;
    assert!(m.load_state(&damaged).is_err());
    assert_eq!(m.system.cpus[0].regs[0], r);
}

#[test]
fn vblank_wait_wakes_when_display_raises_irq_and_timers_cascade() {
    let rom = cartridge(
        &[
            0xef05_0000,
            0xe3a0_0007,
            LOOP,
            0xe59f_0014,
            0xe3a0_1001,
            0xe580_1000,
            0xe59f_000c,
            0xe580_1000,
            0xe12f_ff1e,
            LOOP,
            0x0300_3ff8,
            0x0400_0214,
        ],
        &[LOOP],
    );
    let mut m = Machine::from_rom(rom).unwrap();
    let mut nine = Port::<true> {
        m: &mut m.system.memory,
        rom: &m.rom,
    };
    nine.write32(0x0300_3ffc, 0x0200_000c);
    nine.write8(0x0400_0004, 8);
    nine.write32(0x0400_0210, 1);
    nine.write32(0x0400_0100, 0x0080_fffc);
    nine.write32(0x0400_0104, 0x00c4_ffff);
    m.step().unwrap();
    assert_eq!(m.system.cpus[0].regs[0], 0);
    assert_eq!(m.system.memory.pending[0].get() & (1 << 4), 1 << 4);
    m.system.memory.line = 191;
    m.system.memory.dot = 2128;
    for _ in 0..8 {
        m.step().unwrap();
    }
    assert_eq!(m.system.cpus[0].regs[0], 7);
}

#[test]
fn arm9_can_count_leading_zeros_and_call_thumb_with_blx() {
    // MOV r0,#1; CLZ r1,r0; BLX PC-relative to Thumb subroutine; B .
    let rom = cartridge(
        &[0xe3a0_0001, 0xe16f_1f10, 0xfa00_0000, LOOP, 0x4770_2217],
        &[LOOP],
    );
    let mut m = Machine::from_rom(rom).unwrap();
    for _ in 0..4 {
        m.step().unwrap();
    }
    assert_eq!(m.system.cpus[0].regs[1], 31);
    assert_eq!(m.system.cpus[0].regs[2], 23);
    assert_eq!(m.pcs().0, 0x0200_000c);
}

#[test]
fn internal_titles_ignore_padding_and_non_printable_garbage() {
    let mut rom = cartridge(&[LOOP], &[LOOP]);
    rom[..12].copy_from_slice(b"NAME\0GARBAGE");
    assert_eq!(
        Header::parse(&rom, rom.len() as u64).unwrap().title(),
        "NAME"
    );
    rom[1] = 0xff;
    assert_eq!(Header::parse(&rom, rom.len() as u64).unwrap().title(), "");
}

#[test]
fn keypad_irq_follows_the_selected_cpus_and_or_conditions() {
    let mut mem = Memory::default();
    Port::<true> {
        m: &mut mem,
        rom: &[],
    }
    .write16(0x0400_0132, 0xc003); // A and B
    Port::<false> {
        m: &mut mem,
        rom: &[],
    }
    .write16(0x0400_0132, 0x4003); // A or B
    mem.set_keys(1);
    assert_eq!(mem.pending[0].get(), 0);
    assert_eq!(mem.pending[1].get(), 1 << 12);
    mem.set_keys(3);
    assert_eq!(mem.pending[0].get(), 1 << 12);
    mem.pending[0].set(0);
    mem.set_keys(3);
    assert_eq!(mem.pending[0].get(), 0);
    mem.set_keys(2);
    mem.set_keys(3);
    assert_eq!(mem.pending[0].get(), 1 << 12);
}

#[test]
fn spi_firmware_exposes_verified_settings_and_calibrated_touch_at_both_edges() {
    let mut m = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    let mut port = Port::<false> {
        m: &mut m.system.memory,
        rom: &m.rom,
    };
    port.write16(0x0400_01c0, 0x8900);
    for v in [3, 3, 0xff, 0] {
        port.write8(0x0400_01c2, v);
    }
    let settings: [u8; 0x74] = std::array::from_fn(|_| {
        port.write8(0x0400_01c2, 0);
        port.read8(0x0400_01c2)
    });
    assert_eq!(settings[0], 5);
    let crc = settings[..0x70]
        .iter()
        .fold(0xffff, |crc, &byte| devices::crc16_byte(crc, byte));
    assert_eq!(&settings[0x72..], &crc.to_le_bytes());
    for (i, &byte) in settings[..0x70].iter().enumerate() {
        assert_eq!(port.read8(0x027f_fc80 + i as u32), byte);
    }
    for (x, y) in [(0, 0), (100, 150), (255, 191)] {
        port.m.pen = Some((x, y));
        port.write16(0x0400_01c0, 0x8a00);
        for (axis, command, want) in [(0, 0xd0, x), (1, 0x90, y)] {
            port.write8(0x0400_01c2, command);
            port.write8(0x0400_01c2, 0);
            let high = port.read8(0x0400_01c2);
            port.write8(0x0400_01c2, 0);
            let adc = i32::from((u16::from(high) << 5) | (u16::from(port.read8(0x0400_01c2)) >> 3));
            let word = |i| i32::from(u16::from_le_bytes([settings[i], settings[i + 1]]));
            let (adc1, adc2) = (word(0x58 + 2 * axis), word(0x5e + 2 * axis));
            let (px1, px2) = (
                i32::from(settings[0x5c + axis]),
                i32::from(settings[0x62 + axis]),
            );
            assert_eq!(
                (adc - adc1) * (px2 - px1) / (adc2 - adc1) + px1 - 1,
                i32::from(want)
            );
        }
    }
}

#[test]
fn arm9_saturating_add_sets_q_until_guest_clears_it() {
    let rom = cartridge(
        &[
            0xe3a0_0102,
            0xe1e0_0000,
            0xe3a0_1001, // r0=i32::MAX, r1=1.
            0xe101_2050, // QADD r2,r0,r1.
            0xe10f_3000,
            0xe328_f000,
            0xe10f_4000,
            LOOP, // MRS, MSR CPSR_f,#0, MRS.
        ],
        &[LOOP],
    );
    let mut m = Machine::from_rom(rom).unwrap();
    for _ in 0..4 {
        m.step().unwrap();
    }
    let regs = &m.system.cpus[0].regs;
    assert_eq!(regs[2], i32::MAX as u32);
    assert_ne!(regs[3] & (1 << 27), 0);
    assert_eq!(regs[4] & (1 << 27), 0);
}

#[test]
fn eeprom_survives_reset_and_restores_across_state_versions() {
    let mut machine = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    let blank = machine.save_state().unwrap();
    let mut legacy = LEGACY_STATE_MAGIC.to_vec();
    legacy.extend(machine.rom_digest());
    let payload = codec().serialize(&machine.system).unwrap();
    legacy.extend(crc32fast::hash(&payload).to_le_bytes());
    legacy.extend(payload);

    let mut image = vec![0xff; crate::devices::backup::SMALL];
    image[0x1c4..0x1c8].copy_from_slice(b"SAVE");
    machine.load_backup(&image).unwrap();
    let revision = machine.backup_revision();
    machine.reset();
    assert_eq!(machine.backup(), Some(image.as_slice()));
    assert_eq!(machine.backup_revision(), revision);

    machine.load_state(&legacy).unwrap();
    assert_eq!(machine.backup(), Some(image.as_slice()));
    assert_eq!(machine.backup_revision(), revision);
    let saved = machine.save_state().unwrap();
    machine.load_state(&blank).unwrap();
    assert!(machine.backup().unwrap().iter().all(|&b| b == 0xff));
    assert_ne!(machine.backup_revision(), revision);
    machine.load_state(&saved).unwrap();
    assert_eq!(machine.backup(), Some(image.as_slice()));
    assert!(machine.load_backup(&[0; 32]).is_err());
    assert_eq!(machine.backup(), Some(image.as_slice()));
}

#[test]
fn in_flight_eeprom_program_is_committed_only_on_chip_deselect() {
    let mut m = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    m.system.memory.backup.shift(6, false); // Write enable
    for byte in [2, 0, 0x1f, 0x55] {
        m.system.memory.backup.shift(byte, true);
    }
    let partial = m.save_state().unwrap();
    assert!(m.backup().is_none());
    let rev = m.backup_revision();
    m.system.memory.backup.shift(0xaa, false);
    assert_eq!(m.backup_revision(), rev + 1);
    assert_eq!(m.backup().unwrap()[0], 0xaa);
    m.load_state(&partial).unwrap();
    assert_eq!(m.backup().unwrap()[0x1f], 0xff);
    m.system.memory.backup.deselect();
    assert_eq!(m.backup().unwrap()[0x1f], 0x55);
    assert_eq!(m.backup().unwrap()[0], 0xff);
}

#[test]
fn sound_phase_survives_snapshot_and_reset_discards_pending_host_audio() {
    let mut machine = Machine::from_rom(cartridge(&[LOOP], &[LOOP])).unwrap();
    let old_body = codec()
        .serialize(&(
            &machine.system,
            &machine.system.memory.backup,
            &machine.system.memory.display.geometry,
            &machine.system.memory.wifi,
        ))
        .unwrap();
    let mut old = EXTENDED_STATE_MAGIC.to_vec();
    old.extend(machine.rom_digest());
    old.extend(crc32fast::hash(&old_body).to_le_bytes());
    old.extend(old_body);
    let mut seven = Port::<false> {
        m: &mut machine.system.memory,
        rom: &machine.rom,
    };
    seven.write16(0x0400_0500, 0x807f);
    seven.write16(0x0400_0488, 0xfe81);
    seven.write32(0x0400_0480, 0xe340_007f);
    for _ in 0..9 {
        machine.system.memory.advance(1024);
    }
    machine.system.memory.advance(12); // Save in the middle of a timer.
    let state = machine.save_state().unwrap();
    assert_eq!(machine.take_audio_samples().len(), 9);
    let generate = |m: &mut Machine| {
        for _ in 0..47 {
            m.system.memory.advance(1024);
        }
        m.take_audio_samples()
    };
    let old_run = generate(&mut machine);
    assert!(old_run.windows(2).any(|w| w[0] != w[1]));
    machine.system.memory.advance(1024);
    machine.load_state(&state).unwrap();
    assert_eq!(machine.take_audio_samples().len(), 0);
    assert_eq!(generate(&mut machine), old_run);
    machine.load_state(&old).unwrap();
    assert_eq!(generate(&mut machine), [[0; 2]; 47]);
    machine.load_state(&state).unwrap();
    machine.system.memory.advance(1024);
    machine.reset();
    assert_eq!(machine.take_audio_samples().len(), 0);
    assert_eq!(generate(&mut machine), [[0; 2]; 47]);
}
