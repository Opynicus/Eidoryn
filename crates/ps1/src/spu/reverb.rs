//! Stereo reflection/comb/all-pass network in the guest's SPU RAM.
//! The half-rate resampling currently uses a box filter and linear reconstruction.
#[derive(Clone, Default)]
pub(super) struct Reverb {
    pub cursor: usize,
    previous_input: [i32; 2],
    output: [i32; 2],
    odd: bool,
}

struct Buffer<'a> {
    ram: &'a mut [u8],
    base: usize,
    cursor: usize,
    irq_address: Option<usize>,
    irq: bool,
}

impl Buffer<'_> {
    fn address(&self, offset: i32) -> usize {
        let length = 0x80000 - self.base;
        self.base
            + (self.cursor as i32 - self.base as i32 + offset).rem_euclid(length as i32) as usize
    }
    fn read(&mut self, offset: i32) -> i32 {
        let address = self.address(offset);
        self.irq |= self.irq_address.is_some_and(|irq| irq == address & !7);
        i32::from(i16::from_le_bytes([
            self.ram[address],
            self.ram[address + 1],
        ]))
    }
    fn write(&mut self, offset: i32, value: i32) {
        let address = self.address(offset);
        self.irq |= self.irq_address.is_some_and(|irq| irq == address & !7);
        self.ram[address..address + 2]
            .copy_from_slice(&(value.clamp(-32768, 32767) as i16).to_le_bytes());
    }
}

impl Reverb {
    pub fn sample(
        &mut self,
        input: [i32; 2],
        registers: &[u16],
        ram: &mut [u8],
        enabled: bool,
        base: usize,
        irq_address: Option<usize>,
    ) -> ([i32; 2], bool) {
        self.odd = !self.odd;
        if self.odd {
            self.previous_input = input;
            return (self.output, false);
        }
        let mut buffer = Buffer {
            ram,
            base,
            cursor: self.cursor.max(base),
            irq_address,
            irq: false,
        };
        let address = |i: usize| i32::from(registers[i]) * 8;
        let volume = |i: usize| i32::from(registers[i] as i16);
        let multiply = |a: i32, b: i32| ((i64::from(a) * i64::from(b)) >> 15) as i32;
        let mut output = [0; 2];
        for side in 0..2 {
            if enabled {
                let signal = multiply(
                    i32::midpoint(input[side], self.previous_input[side]),
                    volume(30 + side),
                );
                for (destination, source) in [(10 + side, 16 + side), (18 + side, 24 + (side ^ 1))]
                {
                    let old = buffer.read(address(destination) - 2);
                    let reflection = buffer.read(address(source));
                    let value =
                        old + multiply(signal + multiply(reflection, volume(7)) - old, volume(2));
                    buffer.write(address(destination), value);
                }
            }
            let mut signal = 0;
            for (tap, gain) in [
                (12 + side, 3),
                (14 + side, 4),
                (20 + side, 5),
                (22 + side, 6),
            ] {
                signal += multiply(buffer.read(address(tap)), volume(gain));
            }
            for stage in 0..2 {
                let destination = address(26 + stage * 2 + side);
                let delayed = buffer.read(destination - address(stage));
                signal -= multiply(delayed, volume(8 + stage));
                if enabled {
                    buffer.write(destination, signal);
                }
                signal = multiply(signal, volume(8 + stage)) + delayed;
            }
            output[side] = signal.clamp(-32768, 32767);
        }
        self.cursor = ((buffer.cursor + 2) & 0x7fffe).max(base);
        let interpolated =
            std::array::from_fn(|side| i32::midpoint(self.output[side], output[side]));
        self.output = output;
        (interpolated, buffer.irq)
    }
}
