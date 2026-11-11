use eidoryn_psx::{
    cdrom::COMMAND_CLOCKS,
    cpu::Cpu,
    demo,
    machine::Machine,
    spu::{CONTROL_CLOCKS, SAMPLE_CLOCKS, SAMPLE_RATE},
    timing::CPU_CLOCK_HZ,
};

const ENTRY: u32 = 0x8001_0000;
const LW_R2_R1: u32 = 0x8c22_0000;
const MULT_R1_R2: u32 = 0x0022_0018;
const MULTU_R1_R2: u32 = 0x0022_0019;
const DIV_R1_R2: u32 = 0x0022_001a;
const MFLO_R3: u32 = 0x0000_1812;

fn machine(words: &[u32]) -> Machine {
    let mut m = Machine::from_exe(&demo::executable(), "CPU clock test").unwrap();
    for (index, word) in words.iter().enumerate() {
        m.bus.write32(ENTRY + index as u32 * 4, *word).unwrap();
    }
    m.cpu = Cpu::new(ENTRY);
    m
}

#[test]
fn alu_memory_aliases_and_exceptions_charge_documented_cycles() {
    assert_eq!(CPU_CLOCK_HZ, SAMPLE_RATE * SAMPLE_CLOCKS);
    for address in [0x0000_0100, 0x8000_0100, 0xa000_0100, 0x0060_0100] {
        let mut m = machine(&[LW_R2_R1, 0x2403_0042]);
        m.cpu.registers[1] = address;
        m.bus.write32(address, 0x1234_5678).unwrap();
        assert_eq!(m.step_cycles(), Ok(7));
        assert_eq!(m.elapsed_cycles(), 7);
        assert_eq!(m.cpu.last_step_cycles(), 7);
        assert_eq!(m.cpu.registers[2], 0); // The existing load delay remains intact.
        assert_eq!(m.step_cycles(), Ok(1));
        assert_eq!(m.elapsed_cycles(), 8);
        assert_eq!(m.cpu.registers[2], 0x1234_5678);
        assert_eq!(m.cpu.registers[3], 0x42);
        assert_eq!(m.cpu.instructions, 2);
    }

    let mut scratch = machine(&[LW_R2_R1]);
    scratch.cpu.registers[1] = 0x1f80_0000;
    scratch.bus.write32(0x1f80_0000, 0xabcd_ef01).unwrap();
    assert_eq!(scratch.step_cycles(), Ok(1));
    assert_eq!(scratch.elapsed_cycles(), 1);

    let mut io = machine(&[LW_R2_R1]);
    io.cpu.registers[1] = 0x1f80_1070;
    assert_eq!(io.step_cycles(), Ok(5));
    assert_eq!(io.elapsed_cycles(), 5);

    let mut unaligned = machine(&[LW_R2_R1]);
    unaligned.cpu.registers[1] = 0x101;
    assert_eq!(unaligned.step_cycles(), Ok(1));
    assert_eq!(unaligned.cpu.cop0[13] >> 2 & 31, 4);
    assert_eq!(unaligned.cpu.cop0[8], 0x101);
}

#[test]
fn multiply_divide_waits_overlap_independent_instructions() {
    let cases: &[(u32, u32, u32)] = &[
        (MULTU_R1_R2, 0x7ff, 6),
        (MULTU_R1_R2, 0x800, 9),
        (MULTU_R1_R2, 0x000f_ffff, 9),
        (MULTU_R1_R2, 0x0010_0000, 13),
        (MULTU_R1_R2, u32::MAX, 13),
        (MULT_R1_R2, 0xffff_f800, 6),
        (MULT_R1_R2, 0xffff_f7ff, 9),
        (MULT_R1_R2, 0xfff0_0000, 9),
        (MULT_R1_R2, 0xffef_ffff, 13),
        (DIV_R1_R2, 0x1234, 36),
    ];
    for &(operation, source, latency) in cases {
        for independent in [0_usize, 1, 6, 9, 13, 36, 40] {
            let mut words = vec![operation];
            words.extend(std::iter::repeat_n(0, independent));
            words.push(MFLO_R3);
            let mut m = machine(&words);
            m.cpu.registers[1] = source;
            m.cpu.registers[2] = 3;
            assert_eq!(m.step_cycles(), Ok(1));
            for _ in 0..independent {
                assert_eq!(m.step_cycles(), Ok(1));
            }
            let wait = latency.saturating_sub(independent as u32);
            assert_eq!(
                m.step_cycles(),
                Ok(1 + wait),
                "{operation:08x}, {source:08x}, {independent}"
            );
            assert_eq!(m.elapsed_cycles(), 2 + independent as u64 + u64::from(wait));
            let expected = if operation == DIV_R1_R2 {
                ((source as i32) / 3) as u32
            } else {
                source.wrapping_mul(3)
            };
            assert_eq!(m.cpu.registers[3], expected);
        }
    }
}

#[test]
fn newer_muldiv_or_hi_write_cancels_the_previous_wait() {
    let mut replaced = machine(&[DIV_R1_R2, MULTU_R1_R2, MFLO_R3]);
    replaced.cpu.registers[1] = 7;
    replaced.cpu.registers[2] = 3;
    assert_eq!(replaced.step_cycles(), Ok(1));
    assert_eq!(replaced.step_cycles(), Ok(1));
    assert_eq!(replaced.step_cycles(), Ok(7)); // Fast MULTU, not the earlier DIV.
    assert_eq!(replaced.cpu.registers[3], 21);

    let mut cancelled = machine(&[DIV_R1_R2, 0x0020_0011, MFLO_R3]); // MTHI r1.
    cancelled.cpu.registers[1] = 7;
    cancelled.cpu.registers[2] = 3;
    assert_eq!(cancelled.step_cycles(), Ok(1));
    assert_eq!(cancelled.step_cycles(), Ok(1));
    assert_eq!(cancelled.step_cycles(), Ok(1));
    assert_eq!(cancelled.cpu.hi, 7);
}

#[test]
fn memory_cycles_overlap_a_running_multiply() {
    let mut m = machine(&[MULTU_R1_R2, 0x8c04_0100, MFLO_R3]); // lw r4, 0x100(zero).
    m.cpu.registers[1] = 7;
    m.cpu.registers[2] = 3;
    m.bus.write32(0x100, 0x1234_5678).unwrap();
    assert_eq!(m.step_cycles(), Ok(1));
    assert_eq!(m.step_cycles(), Ok(7));
    assert_eq!(m.step_cycles(), Ok(1)); // Seven elapsed cycles hide the six-cycle multiply.
    assert_eq!(m.cpu.registers[3], 21);
    assert_eq!(m.cpu.registers[4], 0x1234_5678);
    assert_eq!(m.elapsed_cycles(), 9);
}

#[test]
fn timer_irq_raised_during_wait_is_seen_at_the_next_instruction_boundary() {
    let mut m = machine(&[MULTU_R1_R2, MFLO_R3, 0x2404_0042]);
    m.cpu.registers[1] = 7;
    m.cpu.registers[2] = 3;
    m.cpu.cop0[12] |= 0x401; // Global interrupts and IP2.
    m.bus.write32(0x1f80_1074, 1 << 4).unwrap();
    assert!(m.bus.timers.write(0x1108, 3));
    assert!(m.bus.timers.write(0x1104, 0x18));
    assert_eq!(m.step_cycles(), Ok(1));
    assert!(!m.bus.irq_pending());
    assert_eq!(m.step_cycles(), Ok(7));
    assert!(m.bus.irq_pending());
    assert_eq!(m.cpu.registers[3], 21);
    assert_eq!(m.step_cycles(), Ok(1));
    assert_eq!(m.cpu.exceptions, 1);
    assert_eq!(m.cpu.cop0[14], ENTRY + 8);
    assert_eq!(m.cpu.registers[4], 0);
    assert_eq!(m.elapsed_cycles(), 9);
}

#[test]
fn memory_wait_advances_cd_command_and_spu_sample_clocks() {
    let mut m = machine(&[LW_R2_R1]);
    m.cpu.registers[1] = 0x100;
    // Arrange for CD completion and an SPU sample boundary to coincide.
    m.bus.tick(3 * SAMPLE_CLOCKS - COMMAND_CLOCKS).unwrap();
    m.bus.cdrom.write(0, 0);
    m.bus.cdrom.write(1, 1); // Getstat responds after COMMAND_CLOCKS.
    m.bus.tick(COMMAND_CLOCKS - 3).unwrap();
    assert_eq!(m.bus.cdrom.commands, 0);
    let samples_before = m.bus.spu.generated_samples;
    assert_eq!(m.step_cycles(), Ok(7));
    assert_eq!(m.elapsed_cycles(), u64::from(3 * SAMPLE_CLOCKS + 4));
    assert_eq!(m.bus.cdrom.commands, 1);
    assert_eq!(m.bus.cdrom.last_command, Some(1));
    assert_eq!(
        m.bus.spu.generated_samples,
        m.elapsed_cycles() / u64::from(SAMPLE_CLOCKS)
    );
    assert_eq!(m.bus.spu.generated_samples, samples_before + 1);
    assert_eq!(m.bus.cdrom.read(1), 0x10); // Tray-open Getstat response.
}

#[test]
fn direct_cpu_steps_report_cost_and_machine_clones_keep_independent_clocks() {
    let mut m = machine(&[LW_R2_R1, 0]);
    m.cpu.registers[1] = 0x100;
    m.cpu.step(&mut m.bus).unwrap();
    assert_eq!(m.cpu.last_step_cycles(), 7);
    assert_eq!(m.elapsed_cycles(), 0); // Direct CPU-only steps do not tick devices.
    m.bus.tick(m.cpu.last_step_cycles()).unwrap();
    let mut cloned = m.clone();
    assert_eq!(cloned.elapsed_cycles(), 7);
    cloned.step().unwrap();
    assert_eq!(cloned.elapsed_cycles(), 8);
    assert_eq!(m.elapsed_cycles(), 7);
    assert_eq!(machine(&[0]).elapsed_cycles(), 0);
}

#[test]
fn device_failure_stops_the_remaining_wait_cycles() {
    let mut m = machine(&[LW_R2_R1]);
    m.cpu.registers[1] = 0x100;
    m.bus.spu.write(0x1f80_1da8, 17).unwrap();
    m.bus.spu.write(0x1f80_1daa, 0x8010).unwrap(); // Unsupported RAM transfer mode.
    m.bus.tick(CONTROL_CLOCKS - 1).unwrap();
    let before = m.elapsed_cycles();
    assert!(m.step_cycles().is_err());
    assert_eq!(m.cpu.last_step_cycles(), 7); // Estimated instruction cost.
    assert_eq!(m.elapsed_cycles(), before + 1); // The failing cycle is counted once.
    assert!(m.halted.is_some());
    assert!(m.step_cycles().is_err());
    assert_eq!(m.elapsed_cycles(), before + 1);
}
