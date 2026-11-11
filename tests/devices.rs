use eidoryn_psx::{
    bus::Bus,
    cpu::Cpu,
    gpu::Gpu,
    spu::{CONTROL_CLOCKS, Spu},
    timers::Timers,
};

#[test]
fn spu_manual_transfer_waits_for_control_and_preserves_fifo_order() {
    let mut spu = Spu::default();
    spu.write(0x1f80_1dac, 4).unwrap();
    spu.write(0x1f80_1da6, 0x20).unwrap();
    spu.write(0x1f80_1da8, 0x1234).unwrap();
    spu.write(0x1f80_1da8, 0xabcd).unwrap();
    spu.write(0x1f80_1daa, 0x8010).unwrap();
    spu.tick(CONTROL_CLOCKS - 1).unwrap();
    assert_eq!(spu.status() & 0x3f, 0);
    spu.tick(1).unwrap();
    assert_eq!(spu.status() & 0x410, 0x410);
    assert_eq!(spu.transferred_halfwords, 0);
    spu.tick(31).unwrap();
    assert_eq!(spu.transferred_halfwords, 1);
    spu.tick(1).unwrap();
    assert_eq!(spu.transferred_halfwords, 2);
    assert_eq!(spu.status() & 0x400, 0);
    spu.write(0x1f80_1da6, 0x20).unwrap();
    spu.write(0x1f80_1daa, 0x8030).unwrap();
    spu.tick(CONTROL_CLOCKS).unwrap();
    assert!(spu.dma_ready(false));
    assert_eq!(spu.dma_read().unwrap(), 0xabcd_1234);
    assert_eq!(spu.read(0x1f80_1da6), 0x20); // Visible transfer address stays latched.
}

#[test]
fn spu_dma4_round_trip_wraps_sound_ram_and_latches_irq9() {
    let mut bus = Bus::default();
    let words = [0x1122_3344_u32, 0x5566_7788, 0x99aa_bbcc];
    for (n, word) in words.iter().enumerate() {
        bus.write32(0x1000 + n as u32 * 4, *word).unwrap();
    }
    bus.write16(0x1f80_1dac, 4).unwrap();
    bus.write16(0x1f80_1da6, 0xffff).unwrap();
    bus.write16(0x1f80_1da4, 0).unwrap();
    bus.write16(0x1f80_1daa, 0x8060).unwrap();
    bus.write32(0x1f80_1074, 1 << 9).unwrap();
    bus.write32(0x1f80_10f0, 8 << 16).unwrap();
    bus.write32(0x1f80_10f4, 0x0090_0000).unwrap();
    bus.write32(0x1f80_10c0, 0x1000).unwrap();
    bus.write32(0x1f80_10c4, 0x0001_0003).unwrap();
    bus.write32(0x1f80_10c8, 0x0100_0201).unwrap();
    let mut batched = bus.clone();
    let mut single = bus.clone();
    batched.tick(CONTROL_CLOCKS + 1).unwrap();
    for _ in 0..=CONTROL_CLOCKS {
        single.tick(1).unwrap();
    }
    assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
    assert_eq!(
        batched.spu.transferred_halfwords,
        single.spu.transferred_halfwords
    );
    assert_eq!(batched.spu.status(), single.spu.status());
    assert_eq!(batched.read32(0x1f80_10c8), single.read32(0x1f80_10c8));
    assert_eq!(batched.read32(0x1f80_10f4), single.read32(0x1f80_10f4));
    assert_eq!(batched.read32(0x1f80_1070), single.read32(0x1f80_1070));
    assert_eq!(
        batched.spu.drain_audio().collect::<Vec<_>>(),
        single.spu.drain_audio().collect::<Vec<_>>()
    );
    bus.tick(CONTROL_CLOCKS - 1).unwrap();
    assert_eq!(bus.spu.transferred_halfwords, 0);
    bus.tick(1).unwrap();
    assert_eq!(bus.spu.transferred_halfwords, 6);
    assert_eq!(bus.read32(0x1f80_10c8).unwrap() & (1 << 24), 0);
    assert_eq!(bus.read32(0x1f80_10f4).unwrap() & 0x9000_0000, 0x9000_0000);
    assert!(bus.irq_pending());
    bus.write32(0x1f80_1070, !(1 << 9)).unwrap();
    assert!(bus.irq_pending()); // SPU level must be acknowledged at the device.
    bus.write16(0x1f80_1daa, 0x8030).unwrap();
    bus.write32(0x1f80_1070, !(1 << 9)).unwrap();
    assert!(!bus.irq_pending());
    // DMA wrapped intact; the subsequent sample tick may overwrite the CD capture area.
    bus.spu.write(0x1f80_1da6, 0).unwrap();
    assert_eq!(bus.spu.dma_read().unwrap(), words[2]);
    bus.write16(0x1f80_1da6, 0xffff).unwrap();
    bus.write32(0x1f80_10c0, 0x2000).unwrap();
    bus.write32(0x1f80_10c4, 0x0001_0003).unwrap();
    bus.write32(0x1f80_10c8, 0x0100_0200).unwrap();
    bus.tick(CONTROL_CLOCKS).unwrap();
    for (n, word) in words.iter().enumerate() {
        let expected = if n == 2 { word & 0xffff } else { *word };
        assert_eq!(bus.read32(0x2000 + n as u32 * 4).unwrap(), expected);
    }
}

#[test]
fn batched_manual_spu_transfers_match_single_cycles() {
    let mut batched = Bus::default();
    batched.write16(0x1f80_1dac, 4).unwrap();
    batched.write16(0x1f80_1da6, 0x20).unwrap();
    batched.write16(0x1f80_1da8, 0x1234).unwrap();
    batched.write16(0x1f80_1da8, 0xabcd).unwrap();
    batched.write16(0x1f80_1daa, 0x8010).unwrap();
    let mut single = batched.clone();
    for clocks in [CONTROL_CLOCKS - 1, 1, 15, 1, 16, 64] {
        batched.tick(clocks).unwrap();
        for _ in 0..clocks {
            single.tick(1).unwrap();
        }
        assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
        assert_eq!(batched.spu.status(), single.spu.status());
        assert_eq!(
            batched.spu.transferred_halfwords,
            single.spu.transferred_halfwords
        );
        assert_eq!(batched.spu.generated_samples, single.spu.generated_samples);
    }
    assert_eq!(batched.spu.transferred_halfwords, 2);
    for bus in [&mut batched, &mut single] {
        bus.write16(0x1f80_1da6, 0x20).unwrap();
        assert_eq!(bus.spu.dma_read().unwrap(), 0xabcd_1234);
    }
}

#[test]
fn cpu_byte_store_supplies_spu_halfword_bus_and_ignores_odd_address() {
    let mut bus = Bus::default();
    let mut cpu = Cpu::new(0);
    bus.write32(0, (0x28 << 26) | (8 << 21) | (9 << 16))
        .unwrap();
    bus.write32(4, (0x28 << 26) | (8 << 21) | (9 << 16) | 1)
        .unwrap();
    cpu.registers[8] = 0x1f80_1d80;
    cpu.registers[9] = 0x1234_abcd;
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.read16(0x1f80_1d80).unwrap(), 0xabcd);
    cpu.registers[9] = 0;
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.read16(0x1f80_1d80).unwrap(), 0xabcd);
    assert_eq!(bus.read8(0x1f80_1d81).unwrap(), 0xab);
}

#[test]
fn spu_fifo_and_unsupported_ram_mode_fail_boundedly() {
    let mut spu = Spu::default();
    for n in 0..32 {
        spu.write(0x1f80_1da8, n).unwrap();
    }
    assert!(spu.write(0x1f80_1da8, 32).is_err());
    spu.write(0x1f80_1daa, 0x8010).unwrap();
    assert!(spu.tick(CONTROL_CLOCKS + 16).is_err());
}

#[test]
fn batched_spu_error_preserves_partial_cycles_and_preexisting_cd_irq() {
    let mut single = Bus::default();
    // Set a CD level directly so only subsequent bus ticks latch it in I_STAT.
    single.cdrom.write(0, 1);
    single.cdrom.write(2, 0x1f);
    single.cdrom.write(0, 0);
    single.cdrom.write(1, 1);
    single.cdrom.tick(2_000);
    assert!(single.cdrom.irq_pending());
    single.write16(0x1f80_1da8, 0x1234).unwrap();
    single.write16(0x1f80_1daa, 0x8010).unwrap();
    let mut batched = single.clone();

    let batch_error = batched.tick(CONTROL_CLOCKS + 100).unwrap_err();
    let single_error = loop {
        if let Err(error) = single.tick(1) {
            break error;
        }
    };
    assert_eq!(batch_error, single_error);
    assert_eq!(single.elapsed_cycles(), u64::from(CONTROL_CLOCKS));
    assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
    assert_eq!(batched.spu.status(), single.spu.status());
    assert_eq!(batched.spu.generated_samples, single.spu.generated_samples);
    assert_eq!(batched.gpu.status(), single.gpu.status());
    for bus in [&mut batched, &mut single] {
        bus.cdrom.write(0, 1);
        bus.cdrom.write(3, 0x1f);
        assert_eq!(bus.read32(0x1f80_1070).unwrap() & 4, 4);
    }
}

#[test]
fn batched_waiting_dma_reports_error_on_the_same_cycle() {
    let mut batched = Bus::default();
    batched.write32(0x1000, 0x1234_5678).unwrap();
    // Leave sound RAM in an unsupported configuration; the DMA only becomes
    // ready once the SPU control latch changes at CONTROL_CLOCKS.
    batched.write16(0x1f80_1daa, 0x8060).unwrap();
    batched.write32(0x1f80_10f0, 8 << 16).unwrap();
    batched.write32(0x1f80_10c0, 0x1000).unwrap();
    batched.write32(0x1f80_10c4, 0x0001_0001).unwrap();
    batched.write32(0x1f80_10c8, 0x0100_0201).unwrap();
    let mut single = batched.clone();

    let batch_error = batched.tick(CONTROL_CLOCKS + 100).unwrap_err();
    let single_error = loop {
        if let Err(error) = single.tick(1) {
            break error;
        }
    };
    assert_eq!(batch_error, single_error);
    assert_eq!(single.elapsed_cycles(), u64::from(CONTROL_CLOCKS));
    assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
    assert_eq!(batched.spu.generated_samples, 1);
    assert_eq!(batched.spu.generated_samples, single.spu.generated_samples);
    assert_eq!(batched.gpu.status(), single.gpu.status());
    assert_eq!(batched.read32(0x1f80_10c8), single.read32(0x1f80_10c8));
    assert_eq!(batched.read32(0x1f80_1070), single.read32(0x1f80_1070));
}

#[test]
fn batched_quiet_spans_match_gpu_timers_and_audio_across_frames() {
    for mode in [0, 8] {
        // NTSC and PAL scanout.
        let mut batched = Bus::default();
        batched.write32(0x1f80_1814, 0x0800_0000 | mode).unwrap();
        batched.write16(0x1f80_1108, 17).unwrap();
        batched.write16(0x1f80_1104, 0x58).unwrap();
        batched.write16(0x1f80_1118, 4).unwrap();
        batched.write16(0x1f80_1114, 0x158).unwrap();
        batched.write16(0x1f80_1124, 0x220).unwrap();
        let mut single = batched.clone();
        for clocks in [0, 1, 2, 7, 37, 767, 768, 769, 750_000] {
            batched.tick(clocks).unwrap();
            for _ in 0..clocks {
                single.tick(1).unwrap();
            }
            assert_eq!(batched.elapsed_cycles(), single.elapsed_cycles());
            assert_eq!(batched.gpu.status(), single.gpu.status());
            assert_eq!(batched.gpu.frames, single.gpu.frames);
            assert_eq!(batched.read32(0x1f80_1070), single.read32(0x1f80_1070));
            for timer in 0..3 {
                for offset in [0, 4, 8] {
                    let address = 0x1f80_1100 + timer * 16 + offset;
                    assert_eq!(batched.read16(address), single.read16(address));
                }
            }
            assert_eq!(batched.spu.generated_samples, single.spu.generated_samples);
            assert_eq!(
                batched.spu.drain_audio().collect::<Vec<_>>(),
                single.spu.drain_audio().collect::<Vec<_>>()
            );
        }
        assert!(batched.gpu.frames > 0);
    }
}

#[test]
fn root_counter_target_irq_flags_and_two_zero_clock_hold() {
    let mut timers = Timers::default();
    timers.write(0x1108, 3);
    timers.write(0x1104, 0x58); // target reset, target IRQ, repeat, pulse.
    assert_eq!(timers.tick(4), 0);
    assert_eq!(timers.read(0x1100), Some(2));
    assert_eq!(timers.tick(1), 1 << 4);
    assert_eq!(timers.read(0x1104).unwrap() & 0xc00, 0x800);
    assert_eq!(timers.read(0x1104).unwrap() & 0x800, 0);
    timers.tick(1);
    assert_eq!(timers.read(0x1100), Some(0));
    timers.tick(1);
    assert_eq!(timers.read(0x1100), Some(0));
    assert_eq!(timers.tick(3), 1 << 4);
}

#[test]
fn root_counter_one_shot_toggle_overflow_and_divider() {
    let mut timers = Timers::default();
    timers.write(0x1108, 1);
    timers.write(0x1104, 0x18);
    assert_eq!(timers.tick(3), 1 << 4);
    assert_eq!(timers.tick(3), 0); // one-shot until mode rewritten.
    timers.write(0x1104, 0xd8);
    assert_eq!(timers.tick(3), 1 << 4);
    assert_eq!(timers.tick(3), 0); // toggle back to inactive.
    assert_eq!(timers.tick(3), 1 << 4);
    timers.write(0x1124, 0x220); // CPU/8, FFFF interrupt.
    timers.write(0x1120, 0xfffe);
    assert_eq!(timers.tick(23) & (1 << 6), 0);
    assert_eq!(timers.read(0x1120), Some(0xfffe));
    assert_eq!(timers.tick(1) & (1 << 6), 1 << 6);
    assert_eq!(timers.read(0x1120), Some(0xffff));
    assert_ne!(timers.read(0x1124).unwrap() & 0x1000, 0);
    assert_eq!(timers.read(0x1124).unwrap() & 0x1000, 0);
    timers.tick(8);
    assert_eq!(timers.read(0x1120), Some(0));
}

#[test]
fn root_counter_external_clock_and_blank_synchronization() {
    let mut timers = Timers::default();
    timers.write(0x1104, 0x101); // dot clock, pause during HBlank.
    timers.tick(100);
    assert_eq!(timers.read(0x1100), Some(0));
    timers.external_clock(0, 7);
    assert_eq!(timers.read(0x1100), Some(5));
    timers.set_blank(0, true);
    timers.external_clock(0, 10);
    assert_eq!(timers.read(0x1100), Some(5));
    timers.set_blank(0, false);
    timers.external_clock(0, 1);
    assert_eq!(timers.read(0x1100), Some(6));
    timers.write(0x1114, 7); // wait for first VBlank, then free-run.
    timers.tick(10);
    assert_eq!(timers.read(0x1110), Some(0));
    timers.set_blank(1, true);
    timers.tick(7);
    assert_eq!(timers.read(0x1110), Some(5));
    timers.set_blank(1, false);
    timers.tick(1);
    assert_eq!(timers.read(0x1110), Some(6));
    timers.write(0x1124, 1); // Counter 2 sync mode 0 stops.
    timers.tick(100);
    assert_eq!(timers.read(0x1120), Some(0));
}

#[test]
fn gpu_scanout_changes_odd_line_bit_and_generates_vblank_and_timer_clocks() {
    let mut gpu = Gpu::default();
    let mut timers = Timers::default();
    assert_eq!(gpu.status(), 0x1480_2000);
    gpu.gp1(0x0700_0000 | (3 << 10) | 1).unwrap(); // visible lines 1, 2.
    timers.write(0x1114, 0x100); // count HBlank edges.
    timers.write(0x1104, 0x100); // count pixel clocks.
    gpu.tick(2200, &mut timers);
    assert_ne!(gpu.status() & (1 << 31), 0); // odd visible line 1.
    assert_eq!(timers.read(0x1100), Some(346)); // 340 + 8 dots, minus two initial holds.
    let irq = gpu.tick(4500, &mut timers);
    assert_eq!(irq & 1, 1); // Entered VBlank on line 3.
    assert_eq!(gpu.status() & (1 << 31), 0);
    assert_eq!(timers.read(0x1110), Some(1)); // Three HBlanks, two initial holds.
}

#[test]
fn bus_latches_vblank_and_absent_expansion_rom_reads_open_bus() {
    let mut bus = Bus::default();
    bus.gpu.gp1(0x0700_0000 | (3 << 10) | 1).unwrap();
    bus.write32(0x1f80_1074, 1).unwrap();
    bus.tick(6500).unwrap();
    assert!(bus.irq_pending());
    bus.write32(0x1f80_1070, !1).unwrap();
    assert!(!bus.irq_pending());
    assert_eq!(bus.read32(0x1f00_0084).unwrap(), u32::MAX);
    bus.write8(0x1f80_2041, 0xf7).unwrap();
    assert_eq!(bus.post, 7);
    assert_eq!(bus.read8(0x1f80_2041).unwrap(), 0xff);
}

#[test]
fn dma_irq_is_edge_triggered_and_channel_flags_require_master_enable() {
    let mut bus = Bus::default();
    bus.write32(0x1f80_1074, 8).unwrap();
    bus.write32(0x1f80_10f0, 8 << 24).unwrap(); // OTC
    let transfer = |bus: &mut Bus| {
        bus.write32(0x1f80_10e0, 0x1000).unwrap();
        bus.write32(0x1f80_10e4, 1).unwrap();
        bus.write32(0x1f80_10e8, 0x1100_0002).unwrap();
    };
    bus.write32(0x1f80_10f4, 1 << 22).unwrap();
    transfer(&mut bus);
    assert_eq!(bus.read32(0x1f80_10f4).unwrap() & 0xff00_0000, 0);
    bus.write32(0x1f80_10f4, 0x00c0_0000).unwrap();
    assert!(!bus.irq_pending());
    transfer(&mut bus);
    assert!(bus.irq_pending());
    assert_eq!(bus.read32(0x1f80_10f4).unwrap(), 0xc0c0_0000);
    assert_eq!(bus.read8(0x1f80_10f6).unwrap(), 0xc0);
    assert_eq!(bus.read16(0x1f80_10f6).unwrap(), 0xc0c0);
    bus.write8(0x1f80_10f4, 0x41).unwrap();
    assert_eq!(bus.read32(0x1f80_10f4).unwrap(), 0xc0c0_0041); // Low-byte update must not ACK pending flags.
    bus.write32(0x1f80_1070, !8).unwrap();
    bus.tick(1).unwrap();
    assert!(!bus.irq_pending()); // Clearing I_STAT first is permitted.
    assert_eq!(bus.read32(0x1f80_1070).unwrap() & 8, 0);
    bus.write8(0x1f80_10f6, 0x80).unwrap(); // Mask the channel after its flag is set.
    assert_eq!(bus.read32(0x1f80_10f4).unwrap(), 0xc080_0041);
    assert!(!bus.irq_pending()); // A high -> high rewrite is not a new edge.
    bus.write32(0x1f80_10f4, 0x40c0_0000).unwrap();
    transfer(&mut bus);
    assert!(bus.irq_pending());
    bus.write32(0x1f80_10f4, 0x40c0_0000).unwrap();
    assert!(bus.irq_pending()); // Clearing DICR first still requires I_STAT ACK.
    bus.write32(0x1f80_1070, !8).unwrap();
    assert!(!bus.irq_pending());
}
