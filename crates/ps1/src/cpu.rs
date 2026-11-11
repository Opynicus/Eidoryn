//! MIPS I interpreter with approximate CPU-clock instruction and device timing.
#[cfg(feature = "jit")]
mod jit;
mod timing;

use crate::{EmuError, bus::Bus, gte::Gte};

pub const REGISTER_NAMES: [&str; 32] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra",
];

#[derive(Clone, Debug)]
pub struct Cpu {
    pub registers: [u32; 32],
    pub pc: u32,
    pub hi: u32,
    pub lo: u32,
    pub cop0: [u32; 32],
    pub gte: Gte,
    pub instructions: u64,
    pub exceptions: u64,
    next_pc: u32,
    instruction_pc: u32,
    branch_pending: bool,
    in_delay_slot: bool,
    load: Option<(usize, u32)>,
    last_step_cycles: u32,
    muldiv_ready: Option<u64>,
}

impl Cpu {
    pub fn new(pc: u32) -> Self {
        let mut cop0 = [0; 32];
        cop0[12] = 1 << 22; // BEV: ROM exception vectors.
        cop0[15] = 2; // R3000A processor revision.
        Self {
            registers: [0; 32],
            pc,
            next_pc: pc.wrapping_add(4),
            hi: 0,
            lo: 0,
            cop0,
            gte: Gte::default(),
            instructions: 0,
            exceptions: 0,
            instruction_pc: pc,
            branch_pending: false,
            in_delay_slot: false,
            load: None,
            last_step_cycles: 0,
            muldiv_ready: None,
        }
    }

    pub fn instruction_address(&self) -> u32 {
        self.instruction_pc
    }

    /// Estimated cost of the last successful CPU step, in CPU cycles. A device
    /// error may stop the bus before the full cost has elapsed. Direct
    /// `Cpu::step` does not advance the bus; `Machine::step` does.
    pub fn last_step_cycles(&self) -> u32 {
        self.last_step_cycles
    }

    #[inline]
    pub(crate) fn step_timed(&mut self, bus: &mut Bus) -> Result<u32, EmuError> {
        self.step(bus)?;
        let cycles = self.last_step_cycles;
        bus.tick(cycles)?;
        Ok(cycles)
    }

    fn exception(&mut self, code: u32, bad_address: Option<u32>) {
        self.exceptions += 1;
        self.cop0[13] = (self.cop0[13] & 0x0000_ff00) | (code << 2);
        self.cop0[14] = self.instruction_pc;
        if self.in_delay_slot {
            self.cop0[13] |= 1 << 31;
            self.cop0[14] = self.instruction_pc.wrapping_sub(4);
        }
        if let Some(address) = bad_address {
            self.cop0[8] = address;
        }
        let mode = self.cop0[12] & 0x3f;
        self.cop0[12] = (self.cop0[12] & !0x3f) | ((mode << 2) & 0x3f);
        self.pc = if self.cop0[12] & (1 << 22) != 0 {
            0xbfc0_0180
        } else {
            0x8000_0080
        };
        self.next_pc = self.pc.wrapping_add(4);
        self.branch_pending = false;
    }

    fn branch(&mut self, taken: bool, immediate: u32) {
        self.branch_pending = true;
        if taken {
            self.next_pc = self.pc.wrapping_add(immediate << 2);
        }
    }

    pub fn step(&mut self, bus: &mut Bus) -> Result<(), EmuError> {
        self.last_step_cycles = 0;
        self.instruction_pc = self.pc;
        self.in_delay_slot = self.branch_pending;
        self.branch_pending = false;
        // IP2 is the external interrupt line; software pending bits are preserved.
        self.cop0[13] = (self.cop0[13] & !(1 << 10)) | (u32::from(bus.irq_pending()) << 10);
        if self.pc & 3 != 0
            || (self.cop0[12] & 1 != 0 && self.cop0[12] & self.cop0[13] & 0xff00 != 0)
        {
            if let Some((r, value)) = self.load.take() {
                self.registers[r] = value;
            }
            self.registers[0] = 0;
            if self.pc & 3 != 0 {
                self.exception(4, Some(self.pc));
            } else {
                self.exception(0, None);
            }
            self.last_step_cycles = 1;
            return Ok(());
        }
        let instruction = bus.fetch_instruction(self.pc)?;
        self.pc = self.next_pc;
        self.next_pc = self.next_pc.wrapping_add(4);
        self.instructions += 1;
        let previous_load = self.load.take();
        // Operands are read before the previous instruction's delayed load commits.
        let rs = ((instruction >> 21) & 31) as usize;
        let rt = ((instruction >> 16) & 31) as usize;
        let s = self.registers[rs];
        let t = self.registers[rt];
        let merge = previous_load
            .filter(|(r, _)| *r == rt)
            .map_or(t, |(_, v)| v);
        if let Some((r, value)) = previous_load {
            self.registers[r] = value;
        }
        self.registers[0] = 0;
        let exceptions = self.exceptions;
        let result = self.execute(instruction, s, t, merge, bus);
        // Back-to-back loads to the same register cancel the first writeback.
        if self
            .load
            .is_some_and(|(r, _)| previous_load.is_some_and(|(old, _)| old == r))
        {
            self.registers[rt] = t;
        }
        self.registers[0] = 0;
        if result.is_ok() {
            self.last_step_cycles = if self.exceptions == exceptions {
                self.instruction_cycles(instruction, s, bus.elapsed_cycles())
            } else {
                1
            };
        }
        result
    }

    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    fn execute(
        &mut self,
        i: u32,
        s: u32,
        t: u32,
        merge: u32,
        bus: &mut Bus,
    ) -> Result<(), EmuError> {
        let op = i >> 26;
        let rt = ((i >> 16) & 31) as usize;
        let rd = ((i >> 11) & 31) as usize;
        let shift = (i >> 6) & 31;
        let immediate = i32::from(i as i16) as u32;
        let address = s.wrapping_add(immediate);
        if matches!(op, 0x12 | 0x32 | 0x3a) && self.cop0[12] & (1 << 30) == 0 {
            self.exception(11, None);
            self.cop0[13] |= 2 << 28; // Coprocessor unusable: COP2.
            return Ok(());
        }
        match op {
            0 => match i & 63 {
                0 => self.registers[rd] = t << shift,
                2 => self.registers[rd] = t >> shift,
                3 => self.registers[rd] = ((t as i32) >> shift) as u32,
                4 => self.registers[rd] = t << (s & 31),
                6 => self.registers[rd] = t >> (s & 31),
                7 => self.registers[rd] = ((t as i32) >> (s & 31)) as u32,
                8 | 9 => {
                    if i & 63 == 9 {
                        self.registers[rd] = self.next_pc;
                    }
                    self.next_pc = s;
                    self.branch_pending = true;
                }
                12 => self.exception(8, None),
                13 => self.exception(9, None),
                16 => self.registers[rd] = self.hi,
                17 => self.hi = s,
                18 => self.registers[rd] = self.lo,
                19 => self.lo = s,
                24 | 25 => {
                    let product = if i & 63 == 24 {
                        (i64::from(s as i32) * i64::from(t as i32)) as u64
                    } else {
                        u64::from(s) * u64::from(t)
                    };
                    self.lo = product as u32;
                    self.hi = (product >> 32) as u32;
                }
                26 => {
                    let (a, b) = (s as i32, t as i32);
                    (self.lo, self.hi) = if b == 0 {
                        (if a < 0 { 1 } else { u32::MAX }, s)
                    } else if a == i32::MIN && b == -1 {
                        (s, 0)
                    } else {
                        ((a / b) as u32, (a % b) as u32)
                    };
                }
                27 => {
                    (self.lo, self.hi) = s
                        .checked_div(t)
                        .map_or((u32::MAX, s), |quotient| (quotient, s % t));
                }
                32 | 34 => {
                    let result = if i & 63 == 32 {
                        (s as i32).checked_add(t as i32)
                    } else {
                        (s as i32).checked_sub(t as i32)
                    };
                    if let Some(value) = result {
                        self.registers[rd] = value as u32;
                    } else {
                        self.exception(12, None);
                    }
                }
                33 => self.registers[rd] = s.wrapping_add(t),
                35 => self.registers[rd] = s.wrapping_sub(t),
                36 => self.registers[rd] = s & t,
                37 => self.registers[rd] = s | t,
                38 => self.registers[rd] = s ^ t,
                39 => self.registers[rd] = !(s | t),
                42 => self.registers[rd] = u32::from((s as i32) < (t as i32)),
                43 => self.registers[rd] = u32::from(s < t),
                _ => return self.unsupported(i),
            },
            1 => {
                if !matches!(rt, 0 | 1 | 16 | 17) {
                    return self.unsupported(i);
                }
                let taken = ((s as i32) < 0) ^ (rt & 1 != 0);
                if rt & 16 != 0 {
                    self.registers[31] = self.next_pc;
                }
                self.branch(taken, immediate);
            }
            2 | 3 => {
                if op == 3 {
                    self.registers[31] = self.next_pc;
                }
                self.next_pc = (self.pc & 0xf000_0000) | ((i & 0x03ff_ffff) << 2);
                self.branch_pending = true;
            }
            4 => self.branch(s == t, immediate),
            5 => self.branch(s != t, immediate),
            6 => self.branch((s as i32) <= 0, immediate),
            7 => self.branch((s as i32) > 0, immediate),
            8 => {
                if let Some(value) = (s as i32).checked_add(immediate as i32) {
                    self.registers[rt] = value as u32;
                } else {
                    self.exception(12, None);
                }
            }
            9 => self.registers[rt] = address,
            10 => self.registers[rt] = u32::from((s as i32) < (immediate as i32)),
            11 => self.registers[rt] = u32::from(s < immediate),
            12 => self.registers[rt] = s & (i & 0xffff),
            13 => self.registers[rt] = s | (i & 0xffff),
            14 => self.registers[rt] = s ^ (i & 0xffff),
            15 => self.registers[rt] = i << 16,
            16 => match (i >> 21) & 31 {
                0 => self.load = Some((rt, self.cop0[rd])),
                4 => match rd {
                    13 => self.cop0[13] = (self.cop0[13] & !0x300) | (t & 0x300),
                    15 => {}
                    _ => self.cop0[rd] = t,
                },
                16 if i & 63 == 16 => {
                    self.cop0[12] = (self.cop0[12] & !15) | ((self.cop0[12] >> 2) & 15);
                }
                _ => return self.unsupported(i),
            },
            0x12 => match (i >> 21) & 31 {
                0 => self.load = Some((rt, self.gte.read_data(rd))),
                2 => self.load = Some((rt, self.gte.read_control(rd))),
                4 => self.gte.write_data(rd, t),
                6 => self.gte.write_control(rd, t),
                8 if rt <= 1 => self.branch(rt == 0, immediate),
                16..=31 if self.gte.execute(i) => {}
                _ => return self.unsupported(i),
            },
            0x32 | 0x3a => {
                if address & 3 != 0 {
                    self.exception(if op == 0x32 { 4 } else { 5 }, Some(address));
                } else if op == 0x32 {
                    self.gte.write_data(rt, bus.read32(address)?);
                } else if self.cop0[12] & (1 << 16) == 0 {
                    bus.write32(address, self.gte.read_data(rt))?;
                }
            }
            32..=38 => {
                if (matches!(op, 33 | 37) && address & 1 != 0) || (op == 35 && address & 3 != 0) {
                    self.exception(4, Some(address));
                    return Ok(());
                }
                let value = match op {
                    32 => i32::from(bus.read8(address)? as i8) as u32,
                    33 => i32::from(bus.read16(address)? as i16) as u32,
                    34 => {
                        let value = bus.read32(address & !3)?;
                        match address & 3 {
                            0 => (merge & 0x00ff_ffff) | (value << 24),
                            1 => (merge & 0x0000_ffff) | (value << 16),
                            2 => (merge & 0x0000_00ff) | (value << 8),
                            _ => value,
                        }
                    }
                    35 => bus.read32(address)?,
                    36 => u32::from(bus.read8(address)?),
                    37 => u32::from(bus.read16(address)?),
                    38 => {
                        let value = bus.read32(address & !3)?;
                        match address & 3 {
                            0 => value,
                            1 => (merge & 0xff00_0000) | (value >> 8),
                            2 => (merge & 0xffff_0000) | (value >> 16),
                            _ => (merge & 0xffff_ff00) | (value >> 24),
                        }
                    }
                    _ => unreachable!(),
                };
                self.load = Some((rt, value));
            }
            40..=43 | 46 => {
                if (op == 41 && address & 1 != 0) || (op == 43 && address & 3 != 0) {
                    self.exception(5, Some(address));
                    return Ok(());
                }
                // Isolated cache stores do not modify RAM. Cache contents are not modeled.
                if self.cop0[12] & (1 << 16) != 0 {
                    return Ok(());
                }
                match op {
                    40 => bus.store_byte(address, t)?,
                    41 => bus.write16(address, t as u16)?,
                    43 => bus.write32(address, t)?,
                    42 | 46 => {
                        let aligned = address & !3;
                        let lane = address & 3;
                        // Byte lanes avoid a read-modify-write against side-effecting MMIO.
                        if op == 42 {
                            for n in 0..=lane {
                                bus.write8(aligned + n, (t >> ((3 - lane + n) * 8)) as u8)?;
                            }
                        } else {
                            for n in lane..4 {
                                bus.write8(aligned + n, (t >> ((n - lane) * 8)) as u8)?;
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
            _ => return self.unsupported(i),
        }
        Ok(())
    }

    fn unsupported(&self, instruction: u32) -> Result<(), EmuError> {
        Err(EmuError::UnsupportedInstruction {
            pc: self.instruction_pc,
            instruction,
        })
    }
}

/// A compact debugger disassembly, with raw words retained for unsupported encodings.
pub fn disassemble(pc: u32, i: u32) -> String {
    let rs = REGISTER_NAMES[((i >> 21) & 31) as usize];
    let rt = REGISTER_NAMES[((i >> 16) & 31) as usize];
    let rd = REGISTER_NAMES[((i >> 11) & 31) as usize];
    if i == 0 {
        return "nop".into();
    }
    match i >> 26 {
        0 => match i & 63 {
            0 | 2 | 3 => format!(
                "{} {rd}, {rt}, {}",
                match i & 63 {
                    0 => "sll",
                    2 => "srl",
                    _ => "sra",
                },
                (i >> 6) & 31
            ),
            8 => format!("jr {rs}"),
            9 => format!("jalr {rd}, {rs}"),
            12 => "syscall".into(),
            13 => "break".into(),
            32..=39 | 42 | 43 => format!(
                "{} {rd}, {rs}, {rt}",
                match i & 63 {
                    32 => "add",
                    33 => "addu",
                    34 => "sub",
                    35 => "subu",
                    36 => "and",
                    37 => "or",
                    38 => "xor",
                    39 => "nor",
                    42 => "slt",
                    _ => "sltu",
                }
            ),
            _ => format!("special 0x{i:08X}"),
        },
        2 | 3 => format!(
            "{} 0x{:08X}",
            if i >> 26 == 2 { "j" } else { "jal" },
            (pc.wrapping_add(4) & 0xf000_0000) | ((i & 0x03ff_ffff) << 2)
        ),
        4 | 5 => format!(
            "{} {rs}, {rt}, {:+}",
            if i >> 26 == 4 { "beq" } else { "bne" },
            i as i16
        ),
        8..=14 => format!(
            "{} {rt}, {rs}, 0x{:04X}",
            ["addi", "addiu", "slti", "sltiu", "andi", "ori", "xori"][(i >> 26) as usize - 8],
            i & 0xffff
        ),
        15 => format!("lui {rt}, 0x{:04X}", i & 0xffff),
        0x12 => match (i >> 21) & 31 {
            operation @ (0 | 2 | 4 | 6) => format!(
                "{} {rt}, {}{}",
                match operation {
                    0 => "mfc2",
                    2 => "cfc2",
                    4 => "mtc2",
                    _ => "ctc2",
                },
                if operation & 2 == 0 { "d" } else { "c" },
                (i >> 11) & 31
            ),
            8 => format!(
                "bc2{} {:+}",
                if i & (1 << 16) == 0 { "f" } else { "t" },
                i as i16
            ),
            _ => format!("gte 0x{:07X}", i & 0x1ff_ffff),
        },
        0x32 | 0x3a => format!(
            "{} d{}, {}({rs})",
            if i >> 26 == 0x32 { "lwc2" } else { "swc2" },
            (i >> 16) & 31,
            i as i16
        ),
        32..=38 | 40..=43 | 46 => format!(
            "{} {rt}, {}({rs})",
            match i >> 26 {
                32 => "lb",
                33 => "lh",
                34 => "lwl",
                35 => "lw",
                36 => "lbu",
                37 => "lhu",
                38 => "lwr",
                40 => "sb",
                41 => "sh",
                42 => "swl",
                43 => "sw",
                _ => "swr",
            },
            i as i16
        ),
        _ => format!(".word 0x{i:08X}"),
    }
}
