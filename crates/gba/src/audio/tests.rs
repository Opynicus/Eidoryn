use crate::{HEADER_LEN, Machine, cpu::Bus};

const IO: u32 = 0x0400_0000;

fn cart() -> Machine {
    let mut rom = vec![0; HEADER_LEN];
    rom[..4].copy_from_slice(&0xeaff_fffe_u32.to_le_bytes()); // Infinite ARM branch.
    Machine::from_rom(rom).unwrap()
}

fn sample(machine: &mut Machine) -> [i16; 2] {
    machine.memory.tick(512);
    machine
        .take_audio_samples()
        .try_into()
        .map(|[pair]: [_; 1]| pair)
        .unwrap()
}

#[test]
fn square_duty_stereo_length_and_master_power_follow_guest_cycles() {
    let mut machine = cart();
    machine.memory.write8(IO + 0x84, 0x80);
    machine.memory.write16(IO + 0x80, 0x1077); // Channel 1, left only.
    machine.memory.write8(IO + 0x82, 2);
    machine.memory.write16(IO + 0x62, 0xf0bf); // 50% duty, one length tick.
    machine.memory.write16(IO + 0x64, 0xc7e0); // 4096 clocks per waveform.
    assert_eq!(machine.memory.read8(IO + 0x84), 0x81);
    let wave = (0..8).map(|_| sample(&mut machine)).collect::<Vec<_>>();
    assert_eq!(wave.iter().filter(|s| s[0] == 7680).count(), 4);
    assert_eq!(wave.iter().filter(|s| s[0] == -7680).count(), 4);
    assert!(wave.iter().all(|s| s[1] == 0));
    for _ in 8..64 {
        sample(&mut machine);
    }
    assert_eq!(machine.memory.read8(IO + 0x84), 0x80);
    assert_eq!(sample(&mut machine), [0, 0]);
    machine.memory.write8(IO + 0x84, 0);
    machine.memory.write8(IO + 0x63, 0xf0);
    assert_eq!(machine.memory.read16(IO + 0x62), 0);
    assert_eq!(machine.memory.read16(IO + 0x80), 0);
    assert_eq!(machine.memory.read8(IO + 0x82), 2);
}

#[test]
fn second_tone_routes_independently_and_bias_preserves_pwm_resolution() {
    let mut machine = cart();
    machine.memory.write8(IO + 0x84, 0x80);
    machine.memory.write16(IO + 0x80, 0x0277); // Channel 2 to right only.
    machine.memory.write8(IO + 0x82, 2);
    machine.memory.write16(IO + 0x68, 0x7080);
    machine.memory.write16(IO + 0x6c, 0x87e0);
    assert_eq!(machine.memory.read8(IO + 0x84), 0x82);
    assert_eq!(sample(&mut machine), [0, 3584]);
    machine.memory.write16(IO + 0x88, 0xc200);
    machine.cpu.regs[0] = 0;
    machine.bios_call(0x19).unwrap();
    assert_eq!(machine.memory.read16(IO + 0x88), 0xc000);
    machine.cpu.regs[0] = 1;
    machine.bios_call(0x19).unwrap();
    assert_eq!(machine.memory.read16(IO + 0x88), 0xc200);
    machine.memory.tick(511);
    machine.memory.reset_ram(0x40);
    machine.memory.tick(1);
    assert_eq!(machine.take_audio_samples().len(), 1);
    assert_eq!(machine.memory.read8(IO + 0x84), 0);
}

#[test]
fn wave_bank_order_sweep_overflow_and_noise_envelope_are_audible() {
    let mut machine = cart();
    machine.memory.write8(IO + 0x84, 0x80);
    machine.memory.write16(IO + 0x80, 0x4477);
    machine.memory.write8(IO + 0x82, 2);
    machine.memory.write8(IO + 0x70, 0x40);
    for addr in 0x90..0xa0 {
        machine.memory.write8(IO + addr, 0x0f);
    }
    machine.memory.write8(IO + 0x70, 0x80);
    assert_eq!(machine.memory.read8(IO + 0x90), 0); // opposite bank
    machine.memory.write8(IO + 0x73, 0x20);
    machine.memory.write16(IO + 0x74, 0x87c0);
    assert_eq!(sample(&mut machine), [7680; 2]); // Low nibble of the first byte.
    assert_eq!(sample(&mut machine), [-7680; 2]);

    machine.memory.write8(IO + 0x60, 0x11);
    machine.memory.write8(IO + 0x63, 0xf0);
    machine.memory.write16(IO + 0x64, 0x87c0);
    assert_eq!(machine.memory.read8(IO + 0x84) & 1, 0); // Immediate sweep overflow.
    machine.memory.write16(IO + 0x80, 0x8800);
    machine.memory.write8(IO + 0x79, 0x21);
    machine.memory.write16(IO + 0x7c, 0x8031);
    let level = (0..32).map(|_| sample(&mut machine)[0]).collect::<Vec<_>>();
    assert!(level.contains(&128) && level.contains(&-128));
    for _ in 0..1100 {
        sample(&mut machine);
    }
    assert_eq!(sample(&mut machine), [0; 2]);
    assert_ne!(machine.memory.read8(IO + 0x84) & 8, 0);
}

#[test]
fn fifo_dma_ignores_programmed_count_width_and_destination_step_and_replays_after_restore() {
    let mut machine = cart();
    let memory = &mut machine.memory;
    memory.write8(IO + 0x84, 0x80);
    memory.write16(IO + 0x82, 0x520c); // A left/timer 0, B right/cascaded timer 1.
    memory.write8(IO + 0xa0, 32);
    memory.write8(IO + 0xa4, 3);
    for n in 0..64 {
        memory.write8(0x0200_0100 + n, (n as u8).wrapping_sub(4));
        memory.write8(0x0200_0300 + n, n as u8 + 1);
    }
    for (dma, source, dest) in [(0xbc, 0x0200_0100, 0xa0), (0xc8, 0x0200_0300, 0xa4)] {
        memory.write32(IO + dma, source);
        memory.write32(IO + dma + 4, IO + dest);
        memory.write32(IO + dma + 8, 0xf200_0001);
    }
    memory.write32(IO + 0x100, 0x0080_fe00);
    memory.write32(IO + 0x104, 0x0084_fffe);
    assert_eq!(sample(&mut machine), [8192, 0]);
    assert_eq!(sample(&mut machine), [-1024, 768]);
    assert_eq!(machine.memory.read16(IO + 0x202) & 0x600, 0x600);
    machine.memory.tick(173);
    let state = machine.save_state().unwrap();
    let replay = |m: &mut Machine| {
        for _ in 0..50 {
            m.memory.tick(512);
        }
        m.take_audio_samples()
    };
    let expected = replay(&mut machine);
    machine.memory.tick(512);
    machine.load_state(&state).unwrap();
    assert_eq!(machine.take_audio_samples(), [[0; 2]; 0]);
    assert_eq!(replay(&mut machine), expected);
    for (n, pair) in expected.into_iter().enumerate() {
        assert_eq!(pair[0], (n as i16 - 3) * 256);
        assert_eq!(
            pair[1],
            if n == 0 {
                768
            } else {
                (n as i16 + 1) / 2 * 256
            }
        );
    }
}

#[test]
fn undrained_audio_is_bounded_and_guest_reset_discards_it() {
    let mut machine = cart();
    for _ in 0..90 {
        machine.run_frame().unwrap();
    }
    assert_eq!(machine.take_audio_samples().len(), super::MAX_QUEUED);
    machine.run_frame().unwrap();
    machine.reset();
    assert_eq!(machine.take_audio_samples(), [[0; 2]; 0]);
}
