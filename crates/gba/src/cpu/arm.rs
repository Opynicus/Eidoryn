use super::{
    Bus, C, Cpu, LR, MODE, N, PC, Step, Z, load_half, load_signed_byte, load_signed_half,
    load_word, sign_extend,
};

fn reg_index(op: u32, shift: u32) -> usize {
    ((op >> shift) & 15) as usize
}

impl Cpu {
    pub(super) fn arm(&mut self, bus: &mut impl Bus, pc: u32, op: u32) -> Result<Step, String> {
        if !self.condition(op >> 28) {
            return Ok(Step::Run);
        }
        let visible_pc = pc.wrapping_add(8);
        match op {
            _ if op & 0x0fff_fff0 == 0x012f_ff10 => {
                self.exchange(self.reg(reg_index(op, 0), visible_pc));
            }
            _ if op & 0x0fbf_0fff == 0x010f_0000 => {
                let status = if op & (1 << 22) == 0 {
                    self.cpsr
                } else {
                    self.spsr[Self::bank(self.cpsr)]
                };
                self.assign(reg_index(op, 12), status);
            }
            _ if op & 0x0fb0_fff0 == 0x0120_f000 || op & 0x0fb0_f000 == 0x0320_f000 => {
                self.arm_status_write(op, visible_pc);
            }
            _ if op & 0x0fc0_00f0 == 0x0000_0090 => self.arm_multiply(op),
            _ if op & 0x0f80_00f0 == 0x0080_0090 => self.arm_long_multiply(op),
            _ if op & 0x0fb0_0ff0 == 0x0100_0090 => {
                let addr = self.reg(reg_index(op, 16), visible_pc);
                let value = self.reg(reg_index(op, 0), visible_pc);
                let old = if op & (1 << 22) == 0 {
                    let old = load_word(bus, addr);
                    bus.write32(addr & !3, value);
                    old
                } else {
                    let old = bus.read8(addr);
                    bus.write8(addr, value as u8);
                    u32::from(old)
                };
                self.assign(reg_index(op, 12), old);
            }
            _ if op & 0x0e00_0090 == 0x0000_0090 => self.arm_extra_transfer(bus, visible_pc, op)?,
            _ => match (op >> 25) & 7 {
                0 | 1 => self.arm_alu(op, visible_pc)?,
                2 | 3 => self.arm_transfer(bus, op, visible_pc)?,
                4 => self.arm_multiple(bus, visible_pc, op)?,
                5 => {
                    if op & (1 << 24) != 0 {
                        self.regs[LR] = pc.wrapping_add(4);
                    }
                    self.branch(visible_pc.wrapping_add(sign_extend(op & 0x00ff_ffff, 24) << 2));
                }
                7 if op & (1 << 24) != 0 => return Ok(Step::Swi((op >> 16) as u8)),
                _ => return Err(format!("unsupported ARM instruction {op:08x} at {pc:08x}")),
            },
        }
        Ok(Step::Run)
    }

    fn arm_operand(&self, op: u32, pc: u32) -> (u32, bool) {
        if op & (1 << 25) != 0 {
            let rotate = ((op >> 8) & 15) * 2;
            let val = (op & 255).rotate_right(rotate);
            let carry = if rotate == 0 {
                self.cpsr & C != 0
            } else {
                val & N != 0
            };
            return (val, carry);
        }
        let immediate = op & 16 == 0;
        let rm_pc = if immediate { pc } else { pc.wrapping_add(4) };
        let rm = self.reg(reg_index(op, 0), rm_pc);
        let amount = if immediate {
            (op >> 7) & 31
        } else {
            self.reg(reg_index(op, 8), pc) & 255
        };
        self.shift(rm, (op >> 5) & 3, amount, immediate)
    }

    fn arm_alu(&mut self, op: u32, pc: u32) -> Result<(), String> {
        let rd = reg_index(op, 12);
        let code = (op >> 21) & 15;
        let test = (8..=11).contains(&code);
        if test && op & (1 << 20) == 0 {
            return Err(format!(
                "unsupported ARM instruction {op:08x} at {:08x}",
                pc.wrapping_sub(8)
            ));
        }
        let set = op & (1 << 20) != 0;
        let flags = set && (rd != PC || test);
        let (y, shifter_carry) = self.arm_operand(op, pc);
        // Register-specified shifts add a pipeline cycle before Rn and Rm are read.
        let rn_pc = if op & ((1 << 25) | 16) == 16 {
            pc.wrapping_add(4)
        } else {
            pc
        };
        let x = self.reg(reg_index(op, 16), rn_pc);
        let c = self.cpsr & C != 0;
        let result = match code {
            0 | 8 => x & y,
            1 | 9 => x ^ y,
            2 | 10 => self.add(x, !y, true, flags),
            3 => self.add(y, !x, true, flags),
            4 | 11 => self.add(x, y, false, flags),
            5 => self.add(x, y, c, flags),
            6 => self.add(x, !y, c, flags),
            7 => self.add(y, !x, c, flags),
            12 => x | y,
            13 => y,
            14 => x & !y,
            _ => !y,
        };
        if flags && matches!(code, 0 | 1 | 8 | 9 | 12..=15) {
            self.logical(result, shifter_carry);
        }
        if !test {
            if set && rd == PC {
                self.restore_status()?;
            }
            self.assign(rd, result);
        }
        Ok(())
    }

    fn arm_status_write(&mut self, op: u32, pc: u32) {
        let fields = (op >> 16) & 15;
        // ARMv4T implements the flag byte and the interrupt/mode bits of the control byte.
        let value = if op & (1 << 25) == 0 {
            self.reg(reg_index(op, 0), pc)
        } else {
            self.arm_operand(op, pc).0
        };
        let saved = op & (1 << 22) != 0;
        let mut mask = if fields & 8 != 0 { 0xf000_0000 } else { 0 };
        if fields & 1 != 0 && self.cpsr & MODE != 0x10 {
            mask |= if saved { 0xff } else { 0xdf };
        }
        if saved {
            let bank = Self::bank(self.cpsr);
            if bank != 0 {
                self.spsr[bank] = (self.spsr[bank] & !mask) | (value & mask);
            }
        } else {
            self.set_status((self.cpsr & !mask) | (value & mask));
        }
    }

    fn arm_multiply(&mut self, op: u32) {
        let mut val = self.regs[reg_index(op, 0)].wrapping_mul(self.regs[reg_index(op, 8)]);
        if op & (1 << 21) != 0 {
            val = val.wrapping_add(self.regs[reg_index(op, 12)]);
        }
        if op & (1 << 20) != 0 {
            self.set_nz(val);
        }
        self.regs[reg_index(op, 16)] = val;
    }

    fn arm_long_multiply(&mut self, op: u32) {
        let x = self.regs[reg_index(op, 0)];
        let y = self.regs[reg_index(op, 8)];
        let hi = reg_index(op, 16);
        let lo = reg_index(op, 12);
        let mut val = if op & (1 << 22) != 0 {
            (i64::from(x as i32) * i64::from(y as i32)) as u64
        } else {
            u64::from(x) * u64::from(y)
        };
        if op & (1 << 21) != 0 {
            val = val.wrapping_add((u64::from(self.regs[hi]) << 32) | u64::from(self.regs[lo]));
        }
        self.regs[lo] = val as u32;
        self.regs[hi] = (val >> 32) as u32;
        if op & (1 << 20) != 0 {
            self.cpsr = (self.cpsr & !(N | Z)) | (self.regs[hi] & N) | if val == 0 { Z } else { 0 };
        }
    }

    fn address(&self, op: u32, pc: u32, offset: u32) -> (u32, u32) {
        let base = self.reg(reg_index(op, 16), pc);
        let indexed = if op & (1 << 23) != 0 {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        (if op & (1 << 24) != 0 { indexed } else { base }, indexed)
    }

    fn writeback(&mut self, op: u32, addr: u32) {
        if op & (1 << 24) == 0 || op & (1 << 21) != 0 {
            self.assign(reg_index(op, 16), addr);
        }
    }

    fn arm_transfer(&mut self, bus: &mut impl Bus, op: u32, pc: u32) -> Result<(), String> {
        let load = op & (1 << 20) != 0;
        let byte = op & (1 << 22) != 0;
        let rd = reg_index(op, 12);
        let offset = if op & (1 << 25) == 0 {
            op & 0xfff
        } else if op & 16 == 0 {
            let rm = self.reg(reg_index(op, 0), pc);
            self.shift(rm, (op >> 5) & 3, (op >> 7) & 31, true).0
        } else {
            return Err(format!("undefined ARM transfer instruction {op:08x}"));
        };
        let (addr, indexed) = self.address(op, pc, offset);
        if load {
            let value = if byte {
                u32::from(bus.read8(addr))
            } else {
                load_word(bus, addr)
            };
            self.writeback(op, indexed);
            self.assign(rd, value);
        } else {
            let value = self.reg(rd, pc.wrapping_add(4));
            if byte {
                bus.write8(addr, value as u8);
            } else {
                bus.write32(addr & !3, value);
            }
            self.writeback(op, indexed);
        }
        Ok(())
    }

    fn arm_extra_transfer(&mut self, bus: &mut impl Bus, pc: u32, op: u32) -> Result<(), String> {
        let rd = reg_index(op, 12);
        let offset = if op & (1 << 22) == 0 {
            self.reg(reg_index(op, 0), pc)
        } else {
            ((op >> 4) & 0xf0) | (op & 15)
        };
        let (addr, indexed) = self.address(op, pc, offset);
        let kind = (op >> 5) & 3;
        if op & (1 << 20) != 0 {
            let value = match kind {
                1 => load_half(bus, addr),
                2 => load_signed_byte(bus, addr),
                3 => load_signed_half(bus, addr),
                _ => return Err(format!("undefined ARM load instruction {op:08x}")),
            };
            self.writeback(op, indexed);
            self.assign(rd, value);
        } else if kind == 1 {
            bus.write16(addr & !1, self.reg(rd, pc.wrapping_add(4)) as u16);
            self.writeback(op, indexed);
        } else {
            return Err(format!("undefined ARM store instruction {op:08x}"));
        }
        Ok(())
    }

    fn arm_multiple(&mut self, bus: &mut impl Bus, pc: u32, op: u32) -> Result<(), String> {
        let rn = reg_index(op, 16);
        let load = op & (1 << 20) != 0;
        let pre = op & (1 << 24) != 0;
        let up = op & (1 << 23) != 0;
        let writeback = op & (1 << 21) != 0;
        let mut list = op & 0xffff;
        let size = if list == 0 {
            list = 1 << PC;
            64
        } else {
            list.count_ones() * 4
        };
        let base = self.reg(rn, pc);
        let end = if up {
            base.wrapping_add(size)
        } else {
            base.wrapping_sub(size)
        };
        let mut addr = if up { base } else { end };
        if pre == up {
            addr = addr.wrapping_add(4);
        }
        let special = op & (1 << 22) != 0;
        let restore = special && load && list & (1 << PC) != 0;
        let saved_mode = self.cpsr;
        if special && !restore {
            self.set_status((saved_mode & !MODE) | 0x10);
        }
        let mut target = None;
        for r in 0..16 {
            if list & (1 << r) == 0 {
                continue;
            }
            if load {
                let value = bus.read32(addr & !3);
                if r == PC {
                    target = Some(value);
                } else {
                    self.regs[r] = value;
                }
            } else {
                let value = if r == rn && writeback && r != list.trailing_zeros() as usize {
                    end
                } else {
                    self.reg(r, pc.wrapping_add(4))
                };
                bus.write32(addr & !3, value);
            }
            addr = addr.wrapping_add(4);
        }
        if special && !restore {
            self.set_status(saved_mode);
        }
        if writeback && (!load || list & (1 << rn) == 0) {
            self.assign(rn, end);
        }
        if restore {
            self.restore_status()?;
        }
        if let Some(target) = target {
            self.branch(target);
        }
        Ok(())
    }
}
