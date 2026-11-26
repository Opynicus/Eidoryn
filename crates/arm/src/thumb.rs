use super::{
    Bus, C, Cpu, LR, PC, SP, Step, load_half, load_signed_byte, load_signed_half, load_word,
    sign_extend,
};

fn low_reg(op: u32, shift: u32) -> usize {
    ((op >> shift) & 7) as usize
}

impl Cpu {
    pub(super) fn thumb(&mut self, bus: &mut impl Bus, pc: u32, op: u32) -> Result<Step, String> {
        let visible_pc = pc.wrapping_add(4);
        let rd = low_reg(op, 0);
        let rs = low_reg(op, 3);
        match op >> 11 {
            0..=2 => {
                let (value, carry) = self.shift(self.regs[rs], op >> 11, (op >> 6) & 31, true);
                self.regs[rd] = self.logical(value, carry);
            }
            3 => {
                let rhs = low_reg(op, 6);
                let y = if op & (1 << 10) == 0 {
                    self.regs[rhs]
                } else {
                    rhs as u32
                };
                self.regs[rd] = if op & (1 << 9) == 0 {
                    self.add(self.regs[rs], y, false, true)
                } else {
                    self.add(self.regs[rs], !y, true, true)
                };
            }
            4..=7 => self.thumb_immediate(op),
            8 if op & (1 << 10) == 0 => self.thumb_alu(op, rd, rs),
            8 => self.thumb_high(op, visible_pc, bus.armv5())?,
            9 => {
                let addr = (visible_pc & !3).wrapping_add((op & 255) << 2);
                self.regs[low_reg(op, 8)] = bus.read32(addr);
            }
            10 | 11 => {
                let addr = self.regs[rs].wrapping_add(self.regs[low_reg(op, 6)]);
                self.thumb_transfer(bus, rd, addr, (op >> 9) & 7);
            }
            12..=15 => {
                let byte = op & (1 << 12) != 0;
                let offset = ((op >> 6) & 31) << if byte { 0 } else { 2 };
                let addr = self.regs[rs].wrapping_add(offset);
                let kind = ((op >> 9) & 4) | (u32::from(byte) << 1);
                self.thumb_transfer(bus, rd, addr, kind);
            }
            16 | 17 => {
                let addr = self.regs[rs].wrapping_add(((op >> 6) & 31) << 1);
                self.thumb_transfer(bus, rd, addr, 1 | ((op >> 9) & 4));
            }
            18 | 19 => {
                let addr = self.regs[SP].wrapping_add((op & 255) << 2);
                self.thumb_transfer(bus, low_reg(op, 8), addr, (op >> 9) & 4);
            }
            20 | 21 => {
                let base = if op & (1 << 11) == 0 {
                    visible_pc & !3
                } else {
                    self.regs[SP]
                };
                self.regs[low_reg(op, 8)] = base.wrapping_add((op & 255) << 2);
            }
            22 if op & 0xff00 == 0xb000 => {
                let offset = (op & 127) << 2;
                self.regs[SP] = if op & 128 == 0 {
                    self.regs[SP].wrapping_add(offset)
                } else {
                    self.regs[SP].wrapping_sub(offset)
                };
            }
            22 | 23 if op & 0xf600 == 0xb400 => self.thumb_stack(bus, op, visible_pc),
            24 | 25 => self.thumb_multiple(bus, op, visible_pc),
            26 | 27 => {
                let cond = (op >> 8) & 15;
                if cond == 15 {
                    return Ok(Step::Swi(op as u8));
                }
                if cond == 14 {
                    return Err(format!("undefined Thumb instruction {op:04x} at {pc:08x}"));
                }
                if self.condition(cond) {
                    self.branch(visible_pc.wrapping_add(sign_extend(op & 255, 8) << 1));
                }
            }
            28 => self.branch(visible_pc.wrapping_add(sign_extend(op & 0x7ff, 11) << 1)),
            29 if bus.armv5() => {
                let target = self.regs[LR].wrapping_add((op & 0x7ff) << 1);
                self.regs[LR] = self.regs[PC] | 1;
                self.exchange(target & !3);
            }
            30 => self.regs[LR] = visible_pc.wrapping_add(sign_extend(op & 0x7ff, 11) << 12),
            31 => {
                let target = self.regs[LR].wrapping_add((op & 0x7ff) << 1);
                self.regs[LR] = self.regs[PC] | 1;
                self.branch(target);
            }
            _ => {
                return Err(format!(
                    "unsupported Thumb instruction {op:04x} at {pc:08x}"
                ));
            }
        }
        Ok(Step::Run)
    }

    fn thumb_immediate(&mut self, op: u32) {
        let rd = low_reg(op, 8);
        let x = self.regs[rd];
        let y = op & 255;
        match (op >> 11) & 3 {
            0 => {
                self.set_nz(y);
                self.regs[rd] = y;
            }
            1 => {
                self.add(x, !y, true, true);
            }
            2 => self.regs[rd] = self.add(x, y, false, true),
            _ => self.regs[rd] = self.add(x, !y, true, true),
        }
    }

    fn thumb_alu(&mut self, op: u32, rd: usize, rs: usize) {
        let x = self.regs[rd];
        let y = self.regs[rs];
        let c = self.cpsr & C != 0;
        let code = (op >> 6) & 15;
        let result = match code {
            0 | 8 => x & y,
            1 => x ^ y,
            2..=4 | 7 => {
                let kind = if code == 7 { 3 } else { code - 2 };
                let (val, carry) = self.shift(x, kind, y & 255, false);
                self.logical(val, carry)
            }
            5 => self.add(x, y, c, true),
            6 => self.add(x, !y, c, true),
            9 => self.add(0, !y, true, true),
            10 => self.add(x, !y, true, true),
            11 => self.add(x, y, false, true),
            12 => x | y,
            13 => x.wrapping_mul(y),
            14 => x & !y,
            _ => !y,
        };
        if matches!(code, 0 | 1 | 8 | 12..=15) {
            self.set_nz(result);
        }
        if !matches!(code, 8 | 10 | 11) {
            self.regs[rd] = result;
        }
    }

    fn thumb_high(&mut self, op: u32, pc: u32, armv5: bool) -> Result<(), String> {
        let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
        let rs = ((op >> 3) & 15) as usize;
        let x = self.reg(rd, pc);
        let y = self.reg(rs, pc);
        match (op >> 8) & 3 {
            0 => self.assign(rd, x.wrapping_add(y)),
            1 => {
                self.add(x, !y, true, true);
            }
            2 => self.assign(rd, y),
            _ if op & (1 << 7) == 0 => self.exchange(y),
            _ if armv5 => {
                self.regs[LR] = self.regs[PC] | 1;
                self.exchange(y);
            }
            _ => return Err(format!("Thumb BLX {op:04x} requires ARMv5")),
        }
        Ok(())
    }

    fn thumb_transfer(&mut self, bus: &mut impl Bus, rd: usize, addr: u32, kind: u32) {
        match kind {
            0 => bus.write32(addr & !3, self.regs[rd]),
            1 => bus.write16(addr & !1, self.regs[rd] as u16),
            2 => bus.write8(addr, self.regs[rd] as u8),
            3 => self.regs[rd] = load_signed_byte(bus, addr),
            4 => self.regs[rd] = load_word(bus, addr),
            5 => self.regs[rd] = load_half(bus, addr),
            6 => self.regs[rd] = u32::from(bus.read8(addr)),
            _ => self.regs[rd] = load_signed_half(bus, addr),
        }
    }

    fn thumb_stack(&mut self, bus: &mut impl Bus, op: u32, pc: u32) {
        let pop = op & (1 << 11) != 0;
        let mut list = op & 255;
        if op & 256 != 0 {
            list |= 1 << if pop { PC } else { LR };
        }
        let size = if list == 0 {
            list = 1 << PC;
            64
        } else {
            list.count_ones() * 4
        };
        let sp = self.regs[SP];
        let mut addr = if pop { sp } else { sp.wrapping_sub(size) };
        self.regs[SP] = if pop { sp.wrapping_add(size) } else { addr };
        for reg in 0..16 {
            if list & (1 << reg) == 0 {
                continue;
            }
            if pop {
                let value = bus.read32(addr & !3);
                if reg == PC && bus.armv5() {
                    self.exchange(value);
                } else {
                    self.assign(reg, value);
                }
            } else {
                bus.write32(addr & !3, self.reg(reg, pc));
            }
            addr = addr.wrapping_add(4);
        }
    }

    fn thumb_multiple(&mut self, bus: &mut impl Bus, op: u32, pc: u32) {
        let rb = low_reg(op, 8);
        let base = self.regs[rb];
        let list = op & 255;
        let load = op & (1 << 11) != 0;
        if list == 0 {
            if load {
                self.branch(bus.read32(base & !3));
            } else {
                bus.write32(base & !3, pc);
            }
            self.regs[rb] = base.wrapping_add(64);
            return;
        }
        let end = base.wrapping_add(list.count_ones() * 4);
        let mut addr = base;
        for (r, reg) in self.regs[..8].iter_mut().enumerate() {
            if list & (1 << r) == 0 {
                continue;
            }
            if load {
                *reg = bus.read32(addr & !3);
            } else {
                let value = if r == rb && r != list.trailing_zeros() as usize {
                    end
                } else {
                    *reg
                };
                bus.write32(addr & !3, value);
            }
            addr = addr.wrapping_add(4);
        }
        if !load || list & (1 << rb) == 0 {
            self.regs[rb] = end;
        }
    }
}
