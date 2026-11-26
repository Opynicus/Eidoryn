use super::{Bus, C, Cpu, I, IRQ, LR, MODE, N, PC, SP, Step, T, V, Z};
use std::collections::BTreeMap;

#[derive(Default)]
struct Ram(BTreeMap<u32, u8>);

impl Bus for Ram {
    fn read8(&self, addr: u32) -> u8 {
        self.0.get(&addr).copied().unwrap_or_default()
    }

    fn write8(&mut self, addr: u32, value: u8) {
        self.0.insert(addr, value);
    }
}

const ROM: u32 = 0x0800_0000;

fn test_cpu() -> Cpu {
    Cpu::direct_boot(ROM, 0x0300_7f00, 0x0300_7fa0, 0x0300_7fe0)
}

fn arm_rom(words: &[u32]) -> Ram {
    let mut bus = Ram::default();
    for (i, &op) in words.iter().enumerate() {
        bus.write32(ROM + (i as u32) * 4, op);
    }
    bus
}

fn run(cpu: &mut Cpu, bus: &mut Ram, steps: usize) {
    for _ in 0..steps {
        assert_eq!(cpu.step(bus).unwrap(), Step::Run);
    }
}

#[test]
fn cartridge_switches_to_thumb_and_invokes_bios() {
    let mut cpu = test_cpu();
    let mut bus = arm_rom(&[
        0xe28f_0001, // add r0,pc,#1 -> ROM+9
        0xe12f_ff10, // bx r0
    ]);
    for (i, op) in [
        0x2105, // movs r1,#5
        0x3103, // adds r1,#3
        0xf000, 0xf801, // bl sub
        0xdf05, // swi 5
        0x0049, // sub: lsls r1,r1,#1
        0x4770, // bx lr
    ]
    .into_iter()
    .enumerate()
    {
        bus.write16(ROM + 8 + 2 * i as u32, op);
    }
    run(&mut cpu, &mut bus, 8);
    assert_eq!(cpu.regs[1], 16);
    assert_eq!(cpu.regs[LR], ROM + 17);
    assert_eq!(cpu.step(&mut bus), Ok(Step::Swi(5)));
    assert_eq!(cpu.regs[PC], ROM + 18);
    assert_ne!(cpu.cpsr & T, 0);
}

#[test]
fn arm_conditions_carry_borrow_and_overflow() {
    let mut cpu = test_cpu();
    cpu.regs[2] = u32::MAX;
    let mut bus = arm_rom(&[
        0xe3a0_0102, // mov r0,#0x80000000
        0xe250_0001, // subs r0,r0,#1
        0x6281_1001, // addvs r1,r1,#1
        0xe351_0001, // cmp r1,#1
        0x13a0_102a, // movne r1,#42
        0xe292_2001, // adds r2,r2,#1
        0xe2b3_3000, // adcs r3,r3,#0
        0xe2d3_3000, // sbcs r3,r3,#0
        0xef06_0000, // swi 0x60000
    ]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.cpsr & (N | Z | C | V), C | V);
    run(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.regs[1], 1);
    assert_eq!(cpu.cpsr & (N | Z | C | V), Z | C);
    run(&mut cpu, &mut bus, 2);
    assert_eq!(cpu.regs[3], 0);
    assert_eq!(cpu.cpsr & (N | Z | C | V), Z | C);
    assert_eq!(cpu.step(&mut bus), Ok(Step::Swi(6)));
}

#[test]
fn arm_pc_sees_the_extra_register_shift_cycle() {
    let mut cpu = test_cpu();
    let mut bus = arm_rom(&[
        0xe08f_0211, // add r0,pc,r1,lsl r2
        0xe1a0_321f, // mov r3,pc,lsl r2
    ]);
    run(&mut cpu, &mut bus, 2);
    assert_eq!((cpu.regs[0], cpu.regs[3]), (ROM + 12, ROM + 16));
}

#[test]
fn register_shift_edges_and_immediate_rrx() {
    let mut cpu = test_cpu();
    cpu.cpsr |= C;
    for (kind, amount, immediate, expected) in [
        (0, 0, false, (0x8000_0001, true)),
        (0, 32, false, (0, true)),
        (0, 33, false, (0, false)),
        (1, 32, false, (0, true)),
        (1, 0, true, (0, true)),
        (1, 255, false, (0, false)),
        (2, 40, false, (u32::MAX, true)),
        (2, 0, true, (u32::MAX, true)),
        (3, 32, false, (0x8000_0001, true)),
        (3, 0, true, (0xc000_0000, true)),
    ] {
        assert_eq!(cpu.shift(0x8000_0001, kind, amount, immediate), expected);
    }
}

#[test]
fn arm_halfword_signed_and_rotated_unaligned_loads() {
    let mut cpu = test_cpu();
    let mut bus = arm_rom(&[
        0xe1c1_00b0, // strh r0,[r1]
        0xe1d1_20b1, // ldrh r2,[r1,#1]
        0xe1d1_30f1, // ldrsh r3,[r1,#1]
        0xe591_4001, // ldr r4,[r1,#1]
    ]);
    cpu.regs[0] = 0xff82;
    cpu.regs[1] = 0x0200_0000;
    run(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.regs[2], 0x8200_00ff);
    assert_eq!(cpu.regs[3], u32::MAX);
    assert_eq!(cpu.regs[4], 0x8200_00ff);
}

#[test]
fn arm_long_multiply_and_banked_stack() {
    let mut cpu = test_cpu();
    let mut bus = arm_rom(&[
        0xe081_0392, // umull r0,r1,r2,r3
        0xe0e1_0392, // smlal r0,r1,r2,r3
        0xe321_f012, // msr cpsr_c,#0x12
        0xe3a0_d040, // mov sp,#64
        0xe321_f01f, // msr cpsr_c,#0x1f
        0xe321_f012, // msr cpsr_c,#0x12
    ]);
    cpu.regs[2] = u32::MAX;
    cpu.regs[3] = 2;
    run(&mut cpu, &mut bus, 1);
    assert_eq!((cpu.regs[1], cpu.regs[0]), (1, 0xffff_fffe));
    run(&mut cpu, &mut bus, 1);
    assert_eq!((cpu.regs[1], cpu.regs[0]), (1, 0xffff_fffc));
    let original_sp = cpu.regs[SP];
    run(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.regs[SP], original_sp);
    run(&mut cpu, &mut bus, 1);
    assert_eq!(cpu.regs[SP], 64);
}

#[test]
fn irq_handler_preserves_interrupted_thumb_context() {
    let mut cpu = test_cpu();
    let mut bus = Ram::default();
    let handler = 0x0300_0100;
    bus.write32(0x0300_7ffc, handler);
    bus.write32(handler, 0xe3a0_00ff); // mov r0,#255
    bus.write32(handler + 4, 0xe12f_ff1e); // bx lr
    cpu.exchange(ROM | 1);
    cpu.regs[0] = 123;
    let status = cpu.cpsr;
    let sp = cpu.regs[SP];
    cpu.service_irq(&mut bus, 0x0300_7ffc);
    assert_eq!(cpu.cpsr & (MODE | T | I), IRQ | I);
    assert_eq!(cpu.regs[PC], handler);
    run(&mut cpu, &mut bus, 3);
    assert_eq!(cpu.regs[0], 123);
    assert_eq!(cpu.regs[PC], ROM);
    assert_eq!(cpu.regs[SP], sp);
    assert_eq!(cpu.cpsr, status);
}

#[test]
fn thumb_memory_operations_and_stack_return() {
    let mut cpu = test_cpu();
    let mut bus = Ram::default();
    let words = [
        0x605a, // str r2,[r3,#4]
        0x685c, // ldr r4,[r3,#4]
        0x70dc, // strb r4,[r3,#3]
        0x78d9, // ldrb r1,[r3,#3]
        0x805c, // strh r4,[r3,#2]
        0x8859, // ldrh r1,[r3,#2]
        0xb503, // push {r0,r1,lr}
        0xbd06, // pop {r1,r2,pc}
    ];
    for (i, op) in words.into_iter().enumerate() {
        bus.write16(ROM + 2 * i as u32, op);
    }
    let sp = cpu.regs[SP];
    cpu.exchange(ROM | 1);
    cpu.regs[0] = 123;
    cpu.regs[2] = 0xabcd_8765;
    cpu.regs[3] = 0x0200_1000;
    cpu.regs[LR] = ROM | 1;
    run(&mut cpu, &mut bus, 4);
    assert_eq!(cpu.regs[1], 0x65);
    assert_eq!(cpu.regs[4], 0xabcd_8765);
    run(&mut cpu, &mut bus, 4);
    assert_eq!((cpu.regs[1], cpu.regs[2]), (123, 0x8765));
    assert_eq!(cpu.regs[PC], ROM);
    assert_eq!(cpu.regs[SP], sp);
}

#[test]
fn unsupported_coprocessor_and_thumb_instructions_are_reported() {
    let mut cpu = test_cpu();
    let mut bus = arm_rom(&[0xee00_0010, 0x0000_be00]);
    assert!(
        cpu.step(&mut bus)
            .unwrap_err()
            .contains("ee000010 at 08000000")
    );
    cpu.exchange((ROM + 4) | 1);
    assert!(cpu.step(&mut bus).unwrap_err().contains("be00 at 08000004"));
}
