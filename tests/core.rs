use eidoryn_psx::{
    EmuError,
    bus::{BIOS_SIZE, Bus, RAM_SIZE},
    cpu::Cpu,
    demo,
    gpu::{Gpu, VRAM_WIDTH},
    machine::Machine,
    runner::{Command, Runner, Snapshot},
};

fn instruction(op: u32, rs: u32, rt: u32, immediate: u16) -> u32 {
    (op << 26) | (rs << 21) | (rt << 16) | u32::from(immediate)
}

fn setup(words: &[u32]) -> (Cpu, Bus) {
    let mut bus = Bus::default();
    for (i, word) in words.iter().enumerate() {
        bus.write32(i as u32 * 4, *word).unwrap();
    }
    (Cpu::new(0), bus)
}

fn step(cpu: &mut Cpu, bus: &mut Bus, count: usize) {
    for _ in 0..count {
        cpu.step(bus).unwrap();
    }
}

#[test]
fn ram_mirrors_and_scratchpad_are_little_endian() {
    let mut bus = Bus::default();
    bus.write32(0x8000_0100, 0x1234_abcd).unwrap();
    for address in [0x100, 0xa000_0100, 0x20_0100, 0x60_0100] {
        assert_eq!(bus.read32(address).unwrap(), 0x1234_abcd);
    }
    assert_eq!(bus.read8(0x101).unwrap(), 0xab);
    bus.write16(0x1f80_0000, 0xbeef).unwrap();
    assert_eq!(bus.read16(0x9f80_0000).unwrap(), 0xbeef);
    assert!(bus.read32(0xc000_0100).is_err());
    assert!(bus.write32(0xbfc0_0000, 0).is_err());
}

#[test]
fn word_fast_paths_preserve_region_edges_and_ram_mirror_crossings() {
    let mut bus = Bus::default();
    for address in [0x001f_fffd, 0x803f_fffe, 0xa05f_ffff] {
        bus.write32(address, 0x8765_4321).unwrap();
        assert_eq!(bus.peek32(address), Some(0x8765_4321));
        assert_eq!(bus.read32(address).unwrap(), 0x8765_4321);
        for (offset, byte) in [0x21, 0x43, 0x65, 0x87].into_iter().enumerate() {
            assert_eq!(bus.read8(address + offset as u32).unwrap(), byte);
        }
    }
    bus.write32(0x9f80_03fc, 0x7654_3210).unwrap();
    assert_eq!(bus.peek32(0xbf80_03fc), Some(0x7654_3210));
    bus.bios[0x7fffc..].copy_from_slice(&0xfedc_ba98u32.to_le_bytes());
    assert_eq!(bus.peek32(0xbfc7_fffc), Some(0xfedc_ba98));
    for address in [0x007f_fffd, 0x1f80_03fd, 0x1fc7_fffd, 0xffff_fffe] {
        assert_eq!(bus.peek32(address), None);
    }
    assert!(bus.write32(0x007f_fffd, 0).is_err());
    assert!(bus.write32(0x1f80_03fd, 0).is_err());
}

#[test]
fn zero_register_ignores_arithmetic_and_loads() {
    let (mut cpu, mut bus) = setup(&[0x2400_1234, 0x8c00_0100, 0]);
    bus.write32(0x100, 99).unwrap();
    step(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.registers[0], 0);
}

#[test]
fn branch_executes_exactly_one_delay_slot() {
    let (mut cpu, mut bus) = setup(&[0x1000_0002, 0x2401_0001, 0x2401_0063, 0x2402_0002]);
    step(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.registers[1], 1);
    assert_eq!(cpu.registers[2], 2);
    assert_eq!(cpu.pc, 16);
}

#[test]
fn load_delay_reads_old_value_then_commits() {
    let (mut cpu, mut bus) = setup(&[0x8c01_0100, 0x2422_0001, 0x2423_0001]);
    cpu.registers[1] = 7;
    bus.write32(0x100, 41).unwrap();
    step(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.registers[2], 8);
    assert_eq!(cpu.registers[3], 42);
}

#[test]
fn direct_write_overrides_pending_load() {
    let (mut cpu, mut bus) = setup(&[0x8c01_0100, 0x2401_0009, 0]);
    bus.write32(0x100, 42).unwrap();
    step(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.registers[1], 9);
}

#[test]
fn consecutive_loads_cancel_first_writeback() {
    let (mut cpu, mut bus) = setup(&[0x8c01_0100, 0x8c01_0104, 0x2422_0000, 0x2423_0000]);
    cpu.registers[1] = 7;
    bus.write32(0x100, 11).unwrap();
    bus.write32(0x104, 22).unwrap();
    step(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.registers[2], 7);
    assert_eq!(cpu.registers[3], 22);
}

#[test]
fn unaligned_load_pairs_merge_pending_value_in_both_orders() {
    for reverse in [false, true] {
        let mut words = [
            instruction(34, 0, 1, 0x104),
            instruction(38, 0, 1, 0x101),
            0,
        ];
        if reverse {
            words.swap(0, 1);
        }
        let (mut cpu, mut bus) = setup(&words);
        bus.write32(0x100, 0x4433_2211).unwrap();
        bus.write32(0x104, 0x8877_6655).unwrap();
        step(&mut cpu, &mut bus, 3);
        assert_eq!(cpu.registers[1], 0x5544_3322);
    }
}

#[test]
fn unaligned_store_pair_preserves_neighbor_bytes() {
    let (mut cpu, mut bus) = setup(&[instruction(42, 0, 1, 0x104), instruction(46, 0, 1, 0x101)]);
    cpu.registers[1] = 0x1234_5678;
    bus.write32(0x100, u32::MAX).unwrap();
    bus.write32(0x104, u32::MAX).unwrap();
    step(&mut cpu, &mut bus, 2);
    assert_eq!(bus.read32(0x100).unwrap(), 0x3456_78ff);
    assert_eq!(bus.read32(0x104).unwrap(), 0xffff_ff12);
}

#[test]
fn signed_loads_extend_and_unsigned_loads_do_not() {
    let words = [
        instruction(32, 0, 1, 0x100),
        0,
        instruction(36, 0, 2, 0x100),
        0,
        instruction(33, 0, 3, 0x100),
        0,
        instruction(37, 0, 4, 0x100),
        0,
    ];
    let (mut cpu, mut bus) = setup(&words);
    bus.write16(0x100, 0x80ff).unwrap();
    step(&mut cpu, &mut bus, words.len());
    assert_eq!(&cpu.registers[1..5], &[u32::MAX, 255, 0xffff_80ff, 0x80ff]);
}

#[test]
fn overflow_traps_without_writing_destination() {
    let (mut cpu, mut bus) = setup(&[instruction(8, 1, 2, 1)]);
    cpu.registers[1] = 0x7fff_ffff;
    cpu.registers[2] = 99;
    step(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.cop0[13] >> 2 & 31, 12);
    assert_eq!(cpu.pc, 0xbfc0_0180);
    assert_eq!(cpu.registers[2], 99);
}

#[test]
fn delay_slot_exception_records_branch_epc_even_when_branch_not_taken() {
    for branch in [0x1000_0001, 0x1400_0001] {
        let (mut cpu, mut bus) = setup(&[branch, instruction(35, 0, 1, 0x101)]);
        step(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.cop0[14], 0);
        assert_ne!(cpu.cop0[13] & (1 << 31), 0);
        assert_eq!(cpu.cop0[13] >> 2 & 31, 4);
        assert_eq!(cpu.cop0[8], 0x101);
    }
}

#[test]
fn misaligned_store_leaves_ram_unchanged() {
    let (mut cpu, mut bus) = setup(&[instruction(43, 0, 1, 0x101)]);
    cpu.registers[1] = u32::MAX;
    step(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.cop0[13] >> 2 & 31, 5);
    assert_eq!(bus.read32(0x100).unwrap(), 0);
}

#[test]
fn jal_and_jalr_link_to_pc_plus_eight() {
    let (mut cpu, mut bus) = setup(&[0x0c00_0004, 0x2401_0007, 0, 0, 0]);
    step(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.registers[31], 8);
    assert_eq!(cpu.pc, 16);
    assert_eq!(cpu.registers[1], 7);
    let (mut cpu, mut bus) = setup(&[(1 << 21) | (1 << 11) | 9, 0]);
    cpu.registers[1] = 0x100;
    step(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.registers[1], 8);
    assert_eq!(cpu.pc, 0x100);
}

#[test]
fn signed_division_handles_zero_and_minimum_without_host_panic() {
    for (s, t, lo, hi) in [
        (9, 0, u32::MAX, 9),
        (0xffff_fffe, 0, 1, 0xffff_fffe),
        (0x8000_0000, u32::MAX, 0x8000_0000, 0),
        (0xffff_fff9, 3, 0xffff_fffe, u32::MAX),
    ] {
        let (mut cpu, mut bus) = setup(&[(1 << 21) | (2 << 16) | 0x1a]);
        cpu.registers[1] = s;
        cpu.registers[2] = t;
        step(&mut cpu, &mut bus, 1);
        assert_eq!((cpu.lo, cpu.hi), (lo, hi));
    }
}

#[test]
fn cop0_load_is_delayed_and_rfe_restores_mode_bits() {
    let (mut cpu, mut bus) = setup(&[0x4001_6000, 0x2422_0000, 0, 0x4200_0010]);
    cpu.cop0[12] = 0x3c;
    step(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.registers[1], 0x3c);
    assert_eq!(cpu.registers[2], 0);
    assert_eq!(cpu.cop0[12] & 15, 15);
}

#[test]
fn cache_isolation_does_not_modify_main_ram() {
    let (mut cpu, mut bus) = setup(&[instruction(43, 0, 1, 0x100)]);
    cpu.cop0[12] |= 1 << 16;
    cpu.registers[1] = 0xffff_ffff;
    step(&mut cpu, &mut bus, 1);
    assert_eq!(bus.read32(0x100).unwrap(), 0);
}

fn gpu_packet(gpu: &mut Gpu, words: &[u32]) {
    for word in words {
        gpu.gp0(*word).unwrap();
    }
}
fn drawing_gpu() -> Gpu {
    let mut gpu = Gpu::default();
    gpu_packet(&mut gpu, &[0xe300_0000, 0xe407_ffff]);
    gpu
}

#[test]
fn fill_rounds_x_and_width_to_sixteen_pixels() {
    let mut gpu = drawing_gpu();
    gpu_packet(&mut gpu, &[0x0200_00ff, 3, (1 << 16) | 1]);
    assert!(gpu.vram()[..16].iter().all(|p| *p == 31));
    assert_eq!(gpu.vram()[16], 0);
}

#[test]
fn rectangle_obeys_drawing_area_and_signed_offset() {
    let mut gpu = Gpu::default();
    gpu_packet(
        &mut gpu,
        &[
            0xe300_0000 | 4 | (4 << 10),
            0xe400_0000 | 6 | (6 << 10),
            0xe500_07ff,
        ],
    );
    gpu_packet(&mut gpu, &[0x6000_ff00, 4 | (4 << 16), 8 | (8 << 16)]);
    assert_eq!(gpu.vram().iter().filter(|p| **p != 0).count(), 9);
    assert_eq!(gpu.vram()[4 * VRAM_WIDTH + 4], 0x3e0);
}

#[test]
fn triangle_draws_interior_but_not_bounding_box_exterior() {
    let mut gpu = drawing_gpu();
    gpu_packet(&mut gpu, &[0x2000_00ff, 0, 10, 10 << 16]);
    assert_eq!(gpu.vram()[VRAM_WIDTH + 1], 31);
    assert_eq!(gpu.vram()[9 * VRAM_WIDTH + 9], 0);
    assert_eq!(gpu.vram()[11], 0);
}

#[test]
fn gouraud_triangle_interpolates_all_channels() {
    let mut gpu = drawing_gpu();
    gpu_packet(
        &mut gpu,
        &[0x3000_00ff, 0, 0x0000_ff00, 12, 0x00ff_0000, 12 << 16],
    );
    let pixel = gpu.vram()[3 * VRAM_WIDTH + 3];
    assert_ne!(pixel & 31, 0);
    assert_ne!(pixel & 0x3e0, 0);
    assert_ne!(pixel & 0x7c00, 0);
}

#[test]
fn odd_upload_and_download_wrap_vram_without_consuming_extra_pixel() {
    let mut gpu = drawing_gpu();
    gpu_packet(
        &mut gpu,
        &[
            0xa000_0000,
            1023 | (511 << 16),
            3 | (1 << 16),
            0x2222_1111,
            0xffff_3333,
        ],
    );
    assert_eq!(gpu.vram()[511 * VRAM_WIDTH + 1023], 0x1111);
    assert_eq!(gpu.vram()[511 * VRAM_WIDTH], 0x2222);
    assert_eq!(gpu.vram()[511 * VRAM_WIDTH + 1], 0x3333);
    assert_eq!(gpu.vram()[511 * VRAM_WIDTH + 2], 0);
    gpu_packet(&mut gpu, &[0xc000_0000, 1023 | (511 << 16), 3 | (1 << 16)]);
    assert_ne!(gpu.status() & (1 << 27), 0);
    assert_eq!(gpu.read(), 0x2222_1111);
    assert_eq!(gpu.read(), 0x3333);
    assert_eq!(gpu.status() & (1 << 27), 0);
}

#[test]
fn mask_bit_protects_draws_but_not_fill() {
    let mut gpu = drawing_gpu();
    gpu_packet(&mut gpu, &[0xe600_0001, 0x6800_00ff, 0]);
    assert_eq!(gpu.vram()[0], 0x801f);
    gpu_packet(&mut gpu, &[0xe600_0002, 0x6800_ff00, 0]);
    assert_eq!(gpu.vram()[0], 0x801f);
    gpu_packet(&mut gpu, &[0x0200_ff00, 0, 1 | (1 << 16)]);
    assert_eq!(gpu.vram()[0], 0x3e0);
}

#[test]
fn gpu_reset_keeps_vram_and_display_disable_blanks_output() {
    let mut gpu = drawing_gpu();
    gpu.vram_mut()[0] = 31;
    gpu.gp1(0x0300_0000).unwrap();
    assert_eq!(&gpu.display_rgb()[..3], &[255, 0, 0]);
    gpu.gp1(0).unwrap();
    assert_eq!(gpu.vram()[0], 31);
    assert!(gpu.display_rgb().iter().all(|v| *v == 0));
}

#[test]
fn gpu_dma_linked_list_reaches_gp0_and_clears_busy() {
    let mut bus = Bus::default();
    for (offset, word) in [0x03ff_ffff, 0x0200_00ff, 0, 0x0001_0010]
        .into_iter()
        .enumerate()
    {
        bus.write32(0x100 + offset as u32 * 4, word).unwrap();
    }
    bus.write32(0x1f80_10f0, 8 << 8).unwrap();
    bus.write32(0x1f80_10a0, 0x100).unwrap();
    bus.write32(0x1f80_10a8, 0x0100_0401).unwrap();
    assert_eq!(bus.gpu.vram()[0], 31);
    assert_eq!(bus.read32(0x1f80_10a8).unwrap() & (1 << 24), 0);
}

#[test]
fn dma_cycle_is_reported_instead_of_hanging() {
    let mut bus = Bus::default();
    bus.write32(0x100, 0x100).unwrap();
    bus.write32(0x1f80_10f0, 8 << 8).unwrap();
    bus.write32(0x1f80_10a0, 0x100).unwrap();
    assert!(matches!(
        bus.write32(0x1f80_10a8, 0x0100_0401),
        Err(EmuError::Dma(_))
    ));
    // A failed list must not leave traversal marks in the next transfer.
    bus.write32(0x100, 0x00ff_ffff).unwrap();
    bus.write32(0x1f80_10a8, 0).unwrap(); // Explicitly abort the failed, still-busy channel.
    for _ in 0..2 {
        bus.write32(0x1f80_10a0, 0x100).unwrap();
        bus.write32(0x1f80_10a8, 0x0100_0401).unwrap();
        assert_eq!(bus.read32(0x1f80_10a8).unwrap() & (1 << 24), 0);
    }
}

#[test]
fn dma_linked_list_limit_and_ram_wrap_are_preserved_on_reuse() {
    let mut bus = Bus::default();
    bus.write32(0x1f80_10f0, 8 << 8).unwrap();
    for nodes in [4096, 4097, 4096] {
        bus.write32(0x1f80_10a8, 0).unwrap();
        for n in 0..nodes {
            let next = if n + 1 == nodes {
                0x00ff_ffff
            } else {
                0x1000 + (n + 1) * 4
            };
            bus.write32(0x1000 + n * 4, next).unwrap();
        }
        bus.write32(0x1f80_10a0, 0x1000).unwrap();
        let result = bus.write32(0x1f80_10a8, 0x0100_0401);
        assert_eq!(result.is_ok(), nodes == 4096);
    }
    // Header at the final RAM word; its three GP0 payload words wrap to RAM zero.
    for (address, word) in [
        (0x1f_fffc, 0x03ff_ffff),
        (0, 0x0200_00ff),
        (4, 0),
        (8, 0x0001_0010),
    ] {
        bus.write32(address, word).unwrap();
    }
    bus.write32(0x1f80_10a0, 0x1f_fffc).unwrap();
    bus.write32(0x1f80_10a8, 0x0100_0401).unwrap();
    assert_eq!(bus.gpu.vram()[0], 31);
}

#[test]
fn otc_dma_builds_reverse_ordering_table_and_signals_irq() {
    let mut bus = Bus::default();
    bus.write32(0x1f80_10f0, 8 << 24).unwrap();
    bus.write32(0x1f80_10f4, (1 << 23) | (1 << 22)).unwrap();
    bus.write32(0x1f80_1074, 8).unwrap();
    bus.write32(0x1f80_10e0, 0x108).unwrap();
    bus.write32(0x1f80_10e4, 3).unwrap();
    bus.write32(0x1f80_10e8, 0x1100_0002).unwrap();
    assert_eq!(bus.read32(0x108).unwrap(), 0x104);
    assert_eq!(bus.read32(0x104).unwrap(), 0x100);
    assert_eq!(bus.read32(0x100).unwrap(), 0x00ff_ffff);
    assert!(bus.irq_pending());
    bus.write32(0x1f80_10f4, (1 << 30) | (1 << 23) | (1 << 22))
        .unwrap();
    bus.write32(0x1f80_1070, !8).unwrap();
    assert!(!bus.irq_pending());
}

#[test]
fn enabled_external_interrupt_vectors_before_next_instruction() {
    let (mut cpu, mut bus) = setup(&[0x2401_0042]);
    bus.write32(0x1f80_1810, 0x1f00_0000).unwrap();
    bus.write32(0x1f80_1074, 2).unwrap();
    cpu.cop0[12] = (1 << 10) | 1;
    step(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.pc, 0x8000_0080);
    assert_eq!(cpu.cop0[14], 0);
    assert_eq!(cpu.registers[1], 0);
    assert_eq!(cpu.cop0[12] & 0x3f, 4);
}

#[test]
fn exe_loader_validates_ranges_before_copying_and_sets_registers() {
    let bytes = demo::executable();
    let machine = Machine::from_exe(&bytes, "demo").unwrap();
    assert_eq!(machine.cpu.pc, demo::ENTRY);
    assert_eq!(machine.cpu.registers[29], 0x801f_ff00);
    assert_eq!(machine.bus.peek32(demo::ENTRY), Some(demo::program()[0]));
    for size in [0, 7, 0x7ff, bytes.len() - 1] {
        assert!(Machine::from_exe(&bytes[..size], "bad").is_err());
    }
    let mut overflow = bytes.clone();
    overflow[0x18..0x1c].copy_from_slice(&(RAM_SIZE as u32 - 4).to_le_bytes());
    assert!(Machine::from_exe(&overflow, "overflow").is_err());
    overflow[0x18..0x1c].copy_from_slice(&0xffff_fff0_u32.to_le_bytes());
    assert!(Machine::from_exe(&overflow, "overflow").is_err());
    let mut unaligned = bytes;
    unaligned[0x10] |= 1;
    assert!(Machine::from_exe(&unaligned, "unaligned").is_err());
}

#[test]
fn bios_size_and_reset_vector_are_validated() {
    assert!(Machine::from_bios(&[0; 10], "bad").is_err());
    let bytes = vec![0xab; BIOS_SIZE];
    let machine = Machine::from_bios(&bytes, "bios").unwrap();
    assert_eq!(machine.cpu.pc, 0xbfc0_0000);
    assert_eq!(machine.bus.peek32(machine.cpu.pc), Some(0xabab_abab));
}

#[test]
fn demo_runs_through_cpu_bus_gpu_and_animates() {
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    let first = machine.bus.gpu.display_rgb();
    assert_eq!(machine.bus.gpu.dimensions(), (320, 240));
    assert_eq!(machine.bus.gpu.commands, 26);
    assert_eq!(machine.cpu.instructions, 265);
    assert_eq!(&first[..3], &[16, 16, 33]);
    for _ in 0..30 {
        machine.run_slice().unwrap();
    }
    assert_ne!(first, machine.bus.gpu.display_rgb());
    assert_eq!(machine.cpu.exceptions, 0);
    assert!(machine.halted.is_none());
}

#[test]
fn unsupported_instruction_halts_with_original_pc_and_can_reset() {
    let mut machine = Machine::demo();
    machine.bus.write32(machine.cpu.pc, 0xffff_ffff).unwrap();
    assert_eq!(
        machine.step(),
        Err(EmuError::UnsupportedInstruction {
            pc: demo::ENTRY,
            instruction: 0xffff_ffff
        })
    );
    let count = machine.cpu.instructions;
    assert!(machine.step().is_err());
    assert_eq!(machine.cpu.instructions, count);
}

fn wait_snapshot(runner: &Runner) -> Snapshot {
    let start = std::time::Instant::now();
    loop {
        if let Some(snapshot) = runner.take_snapshot() {
            return snapshot;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "worker did not respond"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn worker_acknowledges_close_after_publishing_final_snapshot() {
    let runner = Runner::new(Machine::demo(), false).unwrap();
    wait_snapshot(&runner);
    runner.send(Command::RequestClose).unwrap();
    let snapshot = wait_snapshot(&runner);
    assert!(snapshot.close_ready);
    assert!(!snapshot.running);
    assert!(snapshot.message.is_none());
}

#[test]
fn notifications_do_not_determine_execution_state() {
    use eidoryn_psx::runner::ExecutionState;
    let mut machine = Machine::demo();
    for (running, expected) in [
        (true, ExecutionState::Running),
        (false, ExecutionState::Paused),
    ] {
        let snapshot = Snapshot::capture(
            &machine,
            running,
            Some("Renderer selection failed".into()),
            0.,
        );
        assert_eq!(snapshot.execution_state(), expected);
        assert!(snapshot.message.is_some());
    }
    machine.bus.write32(machine.cpu.pc, 0xffff_ffff).unwrap();
    assert!(machine.step().is_err());
    let snapshot = Snapshot::capture(&machine, true, None, 0.);
    assert_eq!(snapshot.execution_state(), ExecutionState::Stopped);
    assert!(!snapshot.running);
}

#[test]
fn renderer_selection_publishes_a_completion_revision() {
    let runner = Runner::new(Machine::demo(), false).unwrap();
    assert_eq!(wait_snapshot(&runner).renderer_revision, 0);
    for expected in 1..=2 {
        runner
            .send(Command::SetRenderer(eidoryn_psx::gpu::RendererMode::Serial))
            .unwrap();
        let snapshot = wait_snapshot(&runner);
        assert_eq!(snapshot.renderer_revision, expected);
        assert!(snapshot.message.is_none());
    }
}

#[test]
fn worker_steps_and_resets_while_paused() {
    let runner = Runner::new(Machine::demo(), false).unwrap();
    assert_eq!(wait_snapshot(&runner).cpu.instructions, 0);
    runner.send(Command::Step).unwrap();
    let stepped = wait_snapshot(&runner);
    assert_eq!(stepped.cpu.instructions, 1);
    assert!(!stepped.running);
    runner.send(Command::Reset).unwrap();
    let reset = wait_snapshot(&runner);
    assert_eq!(reset.cpu.instructions, 0);
    assert!(!reset.running);
}

#[test]
fn debugger_snapshots_can_be_enabled_and_released_without_stepping() {
    use eidoryn_psx::{card_file::CardStore, runner::SnapshotMode};
    let runner = Runner::with_snapshot_mode(
        Machine::demo(),
        false,
        CardStore::default(),
        None,
        SnapshotMode::Display,
    )
    .unwrap();
    let initial = wait_snapshot(&runner);
    assert_eq!(initial.mode, SnapshotMode::Display);
    assert_eq!(initial.disassembly.capacity(), 0);
    for mode in [
        SnapshotMode::Debug,
        SnapshotMode::Display,
        SnapshotMode::Debug,
    ] {
        runner.send(Command::SetSnapshotMode(mode)).unwrap();
        let snapshot = wait_snapshot(&runner);
        assert_eq!(snapshot.mode, mode);
        assert_eq!(
            snapshot.disassembly.len(),
            if mode == SnapshotMode::Debug { 9 } else { 0 }
        );
        if mode == SnapshotMode::Display {
            assert_eq!(snapshot.disassembly.capacity(), 0);
        }
        assert_eq!(snapshot.cpu.pc, initial.cpu.pc);
        assert_eq!(snapshot.cpu.instructions, initial.cpu.instructions);
        assert!(snapshot.has_same_frame(&initial));
        assert!(!snapshot.running);
    }
    runner.send(Command::Step).unwrap();
    assert_eq!(wait_snapshot(&runner).cpu.instructions, 1);
}

#[test]
fn worker_keeps_machine_after_failed_load_and_clears_notice_on_step() {
    let runner = Runner::new(Machine::demo(), false).unwrap();
    let original = wait_snapshot(&runner);
    runner
        .send(Command::Load {
            path: std::env::temp_dir(),
            bios: false,
        })
        .unwrap();
    let failed = wait_snapshot(&runner);
    assert!(failed.message.is_some());
    assert_eq!(failed.cpu.pc, original.cpu.pc);
    assert_eq!(failed.title, original.title);
    runner.send(Command::Step).unwrap();
    let resumed = wait_snapshot(&runner);
    assert!(resumed.message.is_none());
    assert_eq!(resumed.cpu.instructions, 1);
}

#[test]
fn disassembler_preserves_jump_region_at_boundary() {
    assert_eq!(
        eidoryn_psx::cpu::disassemble(0x8001_0424, 0x0800_4015),
        "j 0x80010054"
    );
    assert_eq!(
        eidoryn_psx::cpu::disassemble(0x8fff_fffc, 0x0800_0000),
        "j 0x90000000"
    );
}

#[test]
fn gpu_dma_request_mode_reads_back_vram() {
    let mut bus = Bus::default();
    bus.gpu.vram_mut()[0] = 0x1234;
    bus.gpu.vram_mut()[1] = 0x5678;
    gpu_packet(&mut bus.gpu, &[0xc000_0000, 0, 2 | (1 << 16)]);
    bus.write32(0x1f80_10f0, 8 << 8).unwrap();
    bus.write32(0x1f80_10a0, 0x100).unwrap();
    bus.write32(0x1f80_10a4, 0x0001_0001).unwrap();
    bus.write32(0x1f80_10a8, 0x0100_0200).unwrap();
    assert_eq!(bus.read32(0x100).unwrap(), 0x5678_1234);
    assert_eq!(bus.read32(0x1f80_10a4).unwrap(), 1);
}

#[test]
fn gpu_packet_reset_discards_incomplete_command() {
    let mut gpu = drawing_gpu();
    gpu_packet(&mut gpu, &[0xa000_0000, 0]);
    gpu.gp1(0x0100_0000).unwrap();
    gpu_packet(&mut gpu, &[0x6800_00ff, 0]);
    assert_eq!(gpu.vram()[0], 31);
    assert!(gpu.gp0(0xf000_0000).is_err()); // Unknown commands still report an error.
}
