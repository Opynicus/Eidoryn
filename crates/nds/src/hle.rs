//! Software interrupts provided by the built-in DS BIOS implementation.
use crate::bus::Port;
use eidoryn_arm::{Bus, Cpu};

pub(crate) fn invoke<const NINE: bool>(
    op: u8,
    cpu: &mut Cpu,
    bus: &mut Port<'_, NINE>,
    wait: &mut Option<u32>,
) -> Result<(), String> {
    let [r0, r1, r2, ..] = cpu.regs;
    match op {
        0x00 => {
            let entry = bus.read32(if NINE { 0x027f_fe24 } else { 0x027f_fe34 });
            let base = if NINE {
                bus.m.irq_vector(true).wrapping_sub(0x3ffc)
            } else {
                0x0380_0000
            };
            let (sys, irq, svc) = if NINE {
                (0x2f7c, 0x3f80, 0x3fc0)
            } else {
                (0xfd80, 0xff80, 0xffc0)
            };
            *cpu = Cpu::direct_boot(
                entry,
                base.wrapping_add(sys),
                base.wrapping_add(irq),
                base.wrapping_add(svc),
            );
            cpu.regs[14] = entry;
            *wait = None;
        }
        0x03 | 0x0f => cpu.regs[0] = 0, // WaitByLoop and IsDebugger return zero.
        0x04 | 0x05 => {
            let mask = if op == 5 { 1 } else { r1 };
            let flags = bus.m.bios_flags(NINE);
            let prev = bus.read32(flags);
            bus.write32(flags, prev & !mask);
            if op == 5 || r0 != 0 || prev & mask == 0 {
                *wait = Some(mask);
            }
            bus.write32(0x0400_0208, 1);
        }
        0x06 => bus.m.halted[usize::from(!NINE)] = true,
        0x07 if !NINE => bus.m.halted[1] = true,
        0x08 if !NINE => bus.write16(0x0400_0504, if r0 == 0 { 0 } else { 0x200 }),
        0x09 => {
            let (n, d) = (r0 as i32, r1 as i32);
            let (q, r) = if d == 0 {
                (if n < 0 { 1 } else { -1 }, n)
            } else {
                (n.wrapping_div(d), n.wrapping_rem(d))
            };
            cpu.regs[0] = q as u32;
            cpu.regs[1] = r as u32;
            cpu.regs[3] = q.unsigned_abs();
        }
        0x0b | 0x0c => {
            let width = if op == 0x0c || r2 & (1 << 26) != 0 {
                4
            } else {
                2
            };
            let (mut src, mut dst) = (r0 & !(width - 1), r1 & !(width - 1));
            for _ in 0..(r2 & 0x1f_ffff) {
                if width == 4 {
                    let v = bus.read32(src);
                    bus.write32(dst, v);
                } else {
                    let v = bus.read16(src);
                    bus.write16(dst, v);
                }
                if r2 & (1 << 24) == 0 {
                    src = src.wrapping_add(width);
                }
                dst = dst.wrapping_add(width);
            }
        }
        0x0d => cpu.regs[0] = r0.isqrt(),
        0x0e => {
            if r2 > 0x0100_0000 {
                return Err("CRC16 length exceeds the DS memory region".into());
            }
            let mut crc = r0 as u16;
            for i in 0..(r2 & !1) {
                crc = crate::devices::crc16_byte(crc, bus.read8((r1 & !1).wrapping_add(i)));
            }
            cpu.regs[0] = u32::from(crc);
        }
        0x10 => unpack(bus, r0, r1, r2)?,
        0x11 | 0x14 => decompress(bus, r0, r1, op == 0x11)?,
        0x1a..=0x1c if !NINE => cpu.regs[0] = sound_table(op, r0),
        _ => return Err(format!("unsupported DS BIOS service {op:#04x}")),
    }
    Ok(())
}

// The sound driver uses the ARM7 BIOS for its quarter-wave, fractional octave
// and attenuation tables. Compute the entries without requiring a BIOS image.
fn sound_table(service: u8, index: u32) -> u32 {
    let x = f64::from(index);
    match service {
        0x1a if index < 64 => (32_767.0 * (x * std::f64::consts::PI / 128.0).sin()).round() as u32,
        0x1b if index < 768 => (65_536.0 * ((x / 768.0).exp2() - 1.0)).round() as u32,
        0x1c if index <= 723 => {
            // Hardware volume dividers cover successive ranges of 0.1 dB.
            // The last entries before these three boundaries round down in BIOS.
            if matches!(index, 602 | 662 | 722) {
                return 126;
            }
            let divider = match index {
                0..483 => 16.0,
                483..603 => 4.0,
                603..663 => 2.0,
                _ => 1.0,
            };
            (128.0 * divider * 10.0_f64.powf((x - 723.0) / 200.0))
                .round()
                .min(127.0) as u32
        }
        _ => 0,
    }
}

fn unpack(bus: &mut impl Bus, mut src: u32, mut dst: u32, params: u32) -> Result<(), String> {
    let len = bus.read16(params);
    let input = u32::from(bus.read8(params.wrapping_add(2)));
    let output = u32::from(bus.read8(params.wrapping_add(3)));
    if !matches!(input, 1 | 2 | 4 | 8)
        || !matches!(output, 1 | 2 | 4 | 8 | 16 | 32)
        || input > output
    {
        return Err("invalid BitUnPack widths".into());
    }
    let offset = bus.read32(params.wrapping_add(4));
    let mut packed = 0;
    let mut bits = 0;
    for _ in 0..len {
        let byte = u32::from(bus.read8(src));
        src = src.wrapping_add(1);
        for sh in (0..8).step_by(input as usize) {
            let mut v = (byte >> sh) & ((1 << input) - 1);
            if v != 0 || offset >> 31 != 0 {
                v = v.wrapping_add(offset & 0x7fff_ffff);
            }
            packed |= v << bits;
            bits += output;
            if bits == 32 {
                bus.write32(dst, packed);
                dst = dst.wrapping_add(4);
                bits = 0;
                packed = 0;
            }
        }
    }
    Ok(())
}

fn next(bus: &impl Bus, src: &mut u32) -> u8 {
    let v = bus.read8(*src);
    *src = src.wrapping_add(1);
    v
}
fn decompress(bus: &mut impl Bus, mut src: u32, start: u32, lz: bool) -> Result<(), String> {
    let hdr = bus.read32(src);
    if hdr as u8 != if lz { 0x10 } else { 0x30 } {
        return Err("invalid BIOS compression header".into());
    }
    let size = hdr >> 8;
    src = src.wrapping_add(4);
    let mut pos = 0u32;
    while pos < size {
        let flags = next(bus, &mut src);
        if lz {
            for i in 0..8 {
                if pos == size {
                    break;
                }
                if flags & (0x80 >> i) == 0 {
                    let v = next(bus, &mut src);
                    bus.write8(start.wrapping_add(pos), v);
                    pos += 1;
                } else {
                    let (hi, lo) = (next(bus, &mut src), next(bus, &mut src));
                    let back = 1 + ((u32::from(hi & 15) << 8) | u32::from(lo));
                    if back > pos {
                        return Err("invalid BIOS LZ77 distance".into());
                    }
                    let len = (u32::from(hi >> 4) + 3).min(size - pos);
                    for _ in 0..len {
                        let v = bus.read8(start.wrapping_add(pos - back));
                        bus.write8(start.wrapping_add(pos), v);
                        pos += 1;
                    }
                }
            }
        } else {
            let repeated = flags & 128 != 0;
            let len = (u32::from(flags & 127) + if repeated { 3 } else { 1 }).min(size - pos);
            let repeat = if repeated { next(bus, &mut src) } else { 0 };
            for _ in 0..len {
                let v = if repeated {
                    repeat
                } else {
                    next(bus, &mut src)
                };
                bus.write8(start.wrapping_add(pos), v);
                pos += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sound_table;

    #[test]
    fn sound_tables_match_arm7_across_divider_boundaries() {
        assert_eq!(sound_table(0x1a, 0), 0);
        assert_eq!(sound_table(0x1a, 32), 23_170);
        assert_eq!(sound_table(0x1a, 63), 32_757);
        assert_eq!(sound_table(0x1b, 384), 27_146);
        assert_eq!(sound_table(0x1b, 767), 65_418);
        for (index, expected) in [
            (0, 0),
            (1, 1),
            (482, 127),
            (483, 32),
            (602, 126),
            (603, 64),
            (662, 126),
            (663, 64),
            (722, 126),
            (723, 127),
        ] {
            assert_eq!(sound_table(0x1c, index), expected);
        }
    }
}
