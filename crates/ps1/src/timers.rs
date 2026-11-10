//! Original root counters with system/divided and externally supplied clocks.
//! GPU scanout supplies horizontal/vertical blank and dot-clock inputs.
//! https://psx-spx.consoledev.net/timers/
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
#[allow(clippy::struct_excessive_bools)] // Independent hardware latches, not mutually exclusive states.
struct Counter {
    count: u16,
    mode: u16,
    target: u16,
    divider: u8,
    hold: u8,
    fired: bool,
    pulse: bool,
    reset_next: bool,
    blank: bool,
    released: bool,
}
impl Default for Counter {
    fn default() -> Self {
        Self {
            count: 0,
            mode: 0x400,
            target: 0,
            divider: 0,
            hold: 0,
            fired: false,
            pulse: false,
            reset_next: false,
            blank: false,
            released: false,
        }
    }
}
#[derive(Clone, Default)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub struct Timers {
    counters: [Counter; 3],
}
impl Timers {
    pub fn read(&mut self, address: u32) -> Option<u16> {
        let counter = self.counters.get_mut(((address & 0x3f) >> 4) as usize)?;
        match address & 15 {
            0 => Some(counter.count),
            4 => {
                let value = counter.mode;
                counter.mode &= !0x1800;
                Some(value)
            }
            8 => Some(counter.target),
            _ => None,
        }
    }
    pub fn write(&mut self, address: u32, value: u16) -> bool {
        let Some(counter) = self.counters.get_mut(((address & 0x3f) >> 4) as usize) else {
            return false;
        };
        match address & 15 {
            0 => {
                counter.count = value;
                counter.hold = 2;
                counter.reset_next = false;
            }
            4 => {
                *counter = Counter {
                    mode: (value & 0x3ff) | 0x400,
                    target: counter.target,
                    hold: 2,
                    blank: counter.blank,
                    ..Counter::default()
                };
            }
            8 => counter.target = value,
            _ => return false,
        }
        true
    }
    /// Returns edges to latch into I_STAT bits 4..6.
    #[inline]
    pub fn tick(&mut self, clocks: u32) -> u32 {
        // Fixed indices allow the single-clock CPU path to specialize each clock source.
        let [dot, line, system] = &mut self.counters;
        let dot_irq = dot.mode & 0x100 == 0 && dot.advance(0, clocks);
        let line_irq = line.mode & 0x100 == 0 && line.advance(1, clocks);
        let pulses = if system.mode & 0x200 != 0 {
            let total = u64::from(clocks) + u64::from(system.divider);
            system.divider = (total % 8) as u8;
            (total / 8) as u32
        } else {
            clocks
        };
        (u32::from(dot_irq) << 4)
            | (u32::from(line_irq) << 5)
            | (u32::from(system.advance(2, pulses)) << 6)
    }
    pub fn external_clock(&mut self, index: usize, pulses: u32) -> u32 {
        if index < 2
            && self.counters[index].mode & 0x100 != 0
            && self.counters[index].advance(index, pulses)
        {
            1 << (4 + index)
        } else {
            0
        }
    }
    pub fn set_blank(&mut self, index: usize, active: bool) {
        if index >= 2 {
            return;
        }
        let counter = &mut self.counters[index];
        if active && !counter.blank && counter.mode & 1 != 0 {
            match (counter.mode >> 1) & 3 {
                1 | 2 => {
                    counter.count = 0;
                    counter.hold = 2;
                    counter.reset_next = false;
                }
                3 => counter.released = true,
                _ => {}
            }
        }
        counter.blank = active;
    }
}
impl Counter {
    #[inline]
    fn advance(&mut self, index: usize, clocks: u32) -> bool {
        let sync = (self.mode >> 1) & 3;
        let stopped = self.mode & 1 != 0
            && if index == 2 {
                matches!(sync, 0 | 3)
            } else {
                match sync {
                    0 => self.blank,
                    2 => !self.blank,
                    3 => !self.released,
                    _ => false,
                }
            };
        if stopped {
            return false;
        }
        let mut interrupt = false;
        for _ in 0..clocks {
            if self.pulse {
                self.mode |= 0x400;
                self.pulse = false;
            }
            if self.hold != 0 {
                self.hold -= 1;
                continue;
            }
            if self.reset_next {
                self.count = 0;
                self.hold = 1;
                self.reset_next = false;
                continue;
            }
            self.count = self.count.wrapping_add(1);
            let equal = self.count == self.target;
            let overflow = self.count == 0xffff;
            if equal {
                self.mode |= 0x800;
                self.reset_next = self.mode & 8 != 0;
            }
            if overflow {
                self.mode |= 0x1000;
            }
            if ((equal && self.mode & 0x10 != 0) || (overflow && self.mode & 0x20 != 0))
                && (!self.fired || self.mode & 0x40 != 0)
            {
                self.fired = true;
                if self.mode & 0x80 != 0 {
                    self.mode ^= 0x400;
                } else {
                    self.mode &= !0x400;
                    self.pulse = true;
                }
                interrupt |= self.mode & 0x400 == 0;
            }
        }
        interrupt
    }
}
