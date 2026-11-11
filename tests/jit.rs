use eidoryn_psx::{cpu_engine::CpuMode, machine::Machine};

#[cfg(not(all(feature = "jit", target_arch = "x86_64", target_os = "linux")))]
#[test]
fn unavailable_jit_preserves_machine_and_selected_mode() {
    let mut machine = Machine::demo();
    machine.run_slice().unwrap();
    let before = format!("{:?}", machine.cpu);
    assert!(machine.set_cpu_mode(CpuMode::Jit).is_err());
    assert_eq!(machine.cpu_mode(), CpuMode::Interpreter);
    assert_eq!(format!("{:?}", machine.cpu), before);
    machine.run_slice().unwrap();
}

#[cfg(all(feature = "jit", target_arch = "x86_64", target_os = "linux"))]
mod native {
    use super::{CpuMode, Machine};
    use eidoryn_psx::{
        cpu::Cpu,
        demo,
        runner::{Command, Runner, Snapshot},
        spu::CONTROL_CLOCKS,
    };
    use std::time::{Duration, Instant};

    const ENTRY: u32 = 0x8001_0000;

    fn program(words: &[u32]) -> Machine {
        let mut machine = Machine::from_exe(&demo::executable(), "JIT regression").unwrap();
        machine.bus.ram.fill(0);
        for (i, word) in words.iter().enumerate() {
            machine.bus.write32(ENTRY + i as u32 * 4, *word).unwrap();
        }
        machine.cpu = Cpu::new(ENTRY);
        machine
    }

    fn warm(words: &[u32]) -> Machine {
        let mut words = words.to_vec();
        words.extend([0x0800_4000, 0]); // Jump back, with a real delay slot.
        let mut machine = program(&words);
        machine.set_cpu_mode(CpuMode::Jit).unwrap();
        machine.run_steps(words.len() * 12).unwrap();
        assert!(machine.cpu_stats().native_instructions > 0);
        machine.cpu = Cpu::new(ENTRY);
        machine
    }

    fn pair(machine: Machine) -> [Box<Machine>; 2] {
        let mut reference = machine.clone();
        reference.set_cpu_mode(CpuMode::Interpreter).unwrap();
        [Box::new(reference), Box::new(machine)]
    }

    fn equal(left: &mut Machine, right: &mut Machine) {
        // Debug includes private branch/load delay state and all GTE state.
        assert_eq!(format!("{:?}", left.cpu), format!("{:?}", right.cpu));
        assert_eq!(left.halted, right.halted);
        assert_eq!(left.bus.elapsed_cycles(), right.bus.elapsed_cycles());
        assert_eq!(left.bus.ram, right.bus.ram);
        assert_eq!(left.bus.gpu.status(), right.bus.gpu.status());
        assert_eq!(left.bus.gpu.frames, right.bus.gpu.frames);
        assert_eq!(left.bus.gpu.commands, right.bus.gpu.commands);
        assert_eq!(left.bus.gpu.vram(), right.bus.gpu.vram());
        assert_eq!(left.bus.spu.status(), right.bus.spu.status());
        assert_eq!(
            left.bus.spu.transferred_halfwords,
            right.bus.spu.transferred_halfwords
        );
        assert_eq!(
            left.bus.spu.drain_audio().collect::<Vec<_>>(),
            right.bus.spu.drain_audio().collect::<Vec<_>>()
        );
        for address in [
            0x1f80_1070,
            0x1f80_1074,
            0x1f80_10c0,
            0x1f80_10c4,
            0x1f80_10c8,
            0x1f80_10f4,
        ] {
            assert_eq!(left.bus.read32(address), right.bus.read32(address));
        }
        // Read timer latches on clones so observation cannot alter later execution.
        let (mut a, mut b) = (left.bus.timers.clone(), right.bus.timers.clone());
        for timer in 0..3 {
            for register in [0, 4, 8] {
                assert_eq!(
                    a.read(0x1100 + timer * 16 + register),
                    b.read(0x1100 + timer * 16 + register)
                );
            }
        }
    }

    #[test]
    fn timed_memory_and_muldiv_waits_match_interpreter() {
        let mut words = vec![
            0x8c05_1000, // lw r5, 0x1000(zero): seven CPU cycles.
            0x0022_001a, // div r1, r2: runs while following instructions execute.
        ];
        words.extend(std::iter::repeat_n(0x24c6_0001, 20)); // JIT-able ALU block.
        words.push(0x0000_1812); // mflo r3 waits for remaining cycles.
        let mut checkpoint = warm(&words);
        checkpoint.cpu.registers[1] = 0x800;
        checkpoint.cpu.registers[2] = 3;
        checkpoint.bus.write32(0x1000, 0x1234_5678).unwrap();
        for budget in 1..=64 {
            let [mut reference, mut native] = pair(checkpoint.clone());
            reference.run_steps(budget).unwrap();
            native.run_steps(budget).unwrap();
            equal(&mut reference, &mut native);
        }
    }

    #[test]
    fn all_integer_encodings_and_budget_boundaries_match_interpreter() {
        let mut seed = 0x7abc_4521_u32;
        let mut random = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for _ in 0..16 {
            let words: Vec<_> = (0..64)
                .map(|index| {
                    let fields = random() & 0x03ff_ffc0;
                    match index % 21 {
                        n @ 0..=13 => {
                            fields | [0, 2, 3, 4, 6, 7, 33, 35, 36, 37, 38, 39, 42, 43][n]
                        }
                        n => (((n - 14 + 9) as u32) << 26) | (random() & 0x03ff_ffff),
                    }
                })
                .collect();
            let mut checkpoint = warm(&words);
            for r in &mut checkpoint.cpu.registers[1..] {
                *r = random();
            }
            for budget in 0..=66 {
                let [mut reference, mut native] = pair(checkpoint.clone());
                reference.run_steps(budget).unwrap();
                native.run_steps(budget).unwrap();
                equal(&mut reference, &mut native);
            }
        }
    }

    #[test]
    fn load_branch_delay_and_trapping_instructions_keep_pipeline_state() {
        let cases: &[&[u32]] = &[
            &[0x8c01_0100, 0x2422_0001, 0x2423_0001, 0x2464_0001],
            &[0x8c01_0100, 0x2401_0009, 0x2423_0001, 0x2464_0001],
            &[0x8c01_0100, 0x8c01_0104, 0x2422_0000, 0x2423_0000],
            &[0x1000_0002, 0x2401_0001, 0x2401_0063, 0x2402_0002],
            &[0x1400_0002, 0x2401_0001, 0x2401_0063, 0x2402_0002],
            &[0x1000_0001, 0x8c01_0101, 0, 0],
            &[0x1400_0001, 0x8c01_0101, 0, 0],
            &[0x3c01_7fff, 0x3421_ffff, 0x2022_0001, 0],
            &[0x4001_6000, 0x2422_0000, 0, 0x4200_0010],
        ];
        for words in cases {
            let mut checkpoint = program(words);
            checkpoint.set_cpu_mode(CpuMode::Jit).unwrap();
            checkpoint.bus.write32(0x100, 41).unwrap();
            checkpoint.bus.write32(0x104, 91).unwrap();
            for _ in 0..12 {
                checkpoint.cpu = Cpu::new(ENTRY);
                checkpoint.cpu.registers[1] = 7;
                let [mut reference, mut native] = pair(checkpoint);
                for _ in 0..4 {
                    reference.step().unwrap();
                    native.run_steps(1).unwrap();
                    equal(&mut reference, &mut native);
                }
                checkpoint = *native;
            }
        }
    }

    #[test]
    fn interpreted_spans_refetch_modified_code_and_respect_budget() {
        let words = [0x8c01_0100, 0x2422_0001, 0x2443_0001, 0x0800_4000, 0];
        let mut checkpoint = program(&words);
        checkpoint.set_cpu_mode(CpuMode::Jit).unwrap();
        checkpoint.bus.write32(0x100, 41).unwrap();
        checkpoint.run_steps(100).unwrap();
        // Change the middle of a cached interpreter span to a branch with a
        // misaligned load in its delay slot. The cached length is only a hint.
        checkpoint.bus.write32(ENTRY + 4, 0x1000_0001).unwrap();
        checkpoint.bus.write32(ENTRY + 8, 0x8c02_0101).unwrap();
        for budget in 0..=32 {
            let mut sample = checkpoint.clone();
            sample.cpu = Cpu::new(ENTRY);
            let [mut reference, mut native] = pair(sample);
            reference.run_steps(budget).unwrap();
            native.run_steps(budget).unwrap();
            equal(&mut reference, &mut native);
        }
    }

    #[test]
    fn interrupts_enter_at_exact_instruction_boundary_in_hot_block() {
        let checkpoint = warm(&[0x2421_0001; 64]);
        for interrupt_after in 0..=63 {
            let mut sample = checkpoint.clone();
            sample.cpu.cop0[12] = 0x401;
            sample.bus.write32(0x1f80_1074, 1 << 4).unwrap();
            sample
                .bus
                .timers
                .write(0x1108, (interrupt_after + 1) as u16);
            sample.bus.timers.write(0x1104, 0x18);
            sample.bus.tick(3).unwrap(); // Release the counter's two startup holds.
            let [mut reference, mut native] = pair(sample);
            reference.run_steps(64).unwrap();
            native.run_steps(64).unwrap();
            equal(&mut reference, &mut native);
            assert_eq!(native.cpu.exceptions, 1);
            assert_eq!(native.cpu.cop0[14], ENTRY + interrupt_after * 4);
            assert_eq!(native.cpu.registers[1], interrupt_after);
        }
    }

    #[test]
    fn ram_alias_and_debug_writes_invalidate_cached_opcodes() {
        let checkpoint = warm(&[0x2421_0001; 64]);
        for address in [ENTRY + 32, 0xa021_0020, 0x0041_0020] {
            let mut sample = checkpoint.clone();
            sample.bus.write32(address, 0x2402_0063).unwrap();
            let [mut reference, mut native] = pair(sample);
            reference.run_steps(64).unwrap();
            native.run_steps(64).unwrap();
            equal(&mut reference, &mut native);
            assert_eq!(native.cpu.registers[2], 99);
            assert!(native.cpu_stats().code_invalidations > 0);
        }
        let mut sample = checkpoint.clone();
        sample.bus.ram[0x10000..0x10004].copy_from_slice(&0x2402_0042_u32.to_le_bytes());
        let [mut reference, mut native] = pair(sample);
        reference.run_steps(64).unwrap();
        native.run_steps(64).unwrap();
        equal(&mut reference, &mut native);
        assert_eq!(native.cpu.registers[2], 66);
        assert!(native.cpu_stats().code_invalidations > 0);
        assert_eq!(checkpoint.bus.peek32(ENTRY), Some(0x2421_0001)); // Clone is isolated.
    }

    #[test]
    fn dma_write_during_native_block_is_seen_before_next_fetch() {
        let mut sample = warm(&[0x2421_0001; 64]);
        let word = 0x2402_0063_u32;
        sample.bus.write16(0x1f80_1dac, 4).unwrap();
        sample.bus.write16(0x1f80_1da6, 0x1000).unwrap();
        sample.bus.write16(0x1f80_1da8, word as u16).unwrap();
        sample
            .bus
            .write16(0x1f80_1da8, (word >> 16) as u16)
            .unwrap();
        sample.bus.write16(0x1f80_1daa, 0x8010).unwrap();
        sample.bus.tick(CONTROL_CLOCKS + 32).unwrap();
        sample.bus.write16(0x1f80_1da6, 0x1000).unwrap();
        sample.bus.write16(0x1f80_1daa, 0x8030).unwrap();
        sample.bus.write32(0x1f80_10f0, 8 << 16).unwrap();
        sample.bus.write32(0x1f80_10c0, 0x10020).unwrap();
        sample.bus.write32(0x1f80_10c4, 0x0001_0001).unwrap();
        sample.bus.write32(0x1f80_10c8, 0x0100_0200).unwrap();
        sample.bus.tick(CONTROL_CLOCKS - 8).unwrap();
        assert_eq!(sample.bus.peek32(ENTRY + 32), Some(0x2421_0001));
        let [mut reference, mut native] = pair(sample);
        reference.run_steps(64).unwrap();
        native.run_steps(64).unwrap();
        equal(&mut reference, &mut native);
        assert_eq!(native.cpu.registers[2], 99);
        assert_eq!(native.cpu.registers[1], 63);
        assert!(native.cpu_stats().code_invalidations > 0);
    }

    #[test]
    fn tick_failure_halts_at_same_completed_instruction() {
        let mut sample = warm(&[0x2421_0001; 64]);
        sample.bus.spu.write(0x1f80_1da8, 17).unwrap();
        sample.bus.spu.write(0x1f80_1daa, 0x8010).unwrap(); // Unsupported RAM mode.
        sample.bus.tick(CONTROL_CLOCKS - 1).unwrap();
        let [mut reference, mut native] = pair(sample);
        assert_eq!(reference.run_steps(64), native.run_steps(64));
        assert!(native.halted.is_some());
        equal(&mut reference, &mut native);
        let count = native.cpu.instructions;
        assert!(native.run_steps(1).is_err());
        assert_eq!(native.cpu.instructions, count);
    }

    #[test]
    fn mmio_fetches_are_never_speculative_and_unsupported_code_still_halts() {
        let mut sample = warm(&[0x2421_0001; 64]);
        for _ in 0..12 {
            for word in [
                0xa000_0000,
                0,
                4 | (1 << 16),
                0x2401_0011,
                0x2402_0022,
                0xc000_0000,
                0,
                4 | (1 << 16),
            ] {
                sample.bus.gpu.gp0(word).unwrap();
            }
            sample.cpu = Cpu::new(0x1f80_1810);
            let [mut reference, mut native] = pair(sample);
            reference.run_steps(1).unwrap();
            native.run_steps(1).unwrap();
            equal(&mut reference, &mut native);
            assert_eq!(native.cpu.registers[1], 17);
            assert_eq!(reference.bus.gpu.read(), 0x2402_0022);
            assert_eq!(native.bus.gpu.read(), 0x2402_0022);
            sample = *native;
        }
        sample.cpu = Cpu::new(ENTRY);
        sample.bus.write32(ENTRY + 8, u32::MAX).unwrap();
        let [mut reference, mut native] = pair(sample);
        assert_eq!(reference.run_steps(64), native.run_steps(64));
        equal(&mut reference, &mut native);
        assert!(native.halted.is_some());
        assert_eq!(native.cpu.instruction_address(), ENTRY + 8);
    }

    fn snapshot(runner: &Runner) -> Snapshot {
        let start = Instant::now();
        loop {
            if let Some(snapshot) = runner.take_snapshot() {
                return snapshot;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn demo_frames_and_runtime_switches_preserve_state_and_reset_choice() {
        let mut native = Machine::demo();
        native.set_cpu_mode(CpuMode::Jit).unwrap();
        let mut reference = Machine::demo();
        for _ in 0..100 {
            assert_eq!(reference.run_slice(), native.run_slice());
            equal(&mut reference, &mut native);
            assert_eq!(
                reference.bus.gpu.completed_frame().unwrap().rgb,
                native.bus.gpu.completed_frame().unwrap().rgb
            );
        }
        assert!(native.cpu_stats().native_instructions > 0);
        let runner = Runner::new(native, false).unwrap();
        let original = snapshot(&runner);
        for mode in [CpuMode::Interpreter, CpuMode::Jit] {
            runner.send(Command::SetCpuMode(mode)).unwrap();
            let switched = snapshot(&runner);
            assert_eq!(switched.cpu_mode, mode);
            assert_eq!(switched.cpu.instructions, original.cpu.instructions);
            assert_eq!(switched.rgb, original.rgb);
            assert!(!switched.running);
            assert_eq!(switched.message, None);
        }
        for command in [Command::Reset, Command::Demo, Command::RequestClose] {
            runner.send(command).unwrap();
            let state = snapshot(&runner);
            assert_eq!(state.cpu_mode, CpuMode::Jit);
            assert_eq!(state.message, None);
        }
    }
}
