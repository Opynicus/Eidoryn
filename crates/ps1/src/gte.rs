//! Independent integer GTE, derived from the hardware register/operation descriptions:
//! https://psx-spx.consoledev.net/geometrytransformationenginegte/
//! Results are synchronous. Command busy times and register pipeline hazards are not modeled.

#[derive(Clone, Debug, Default)]
pub struct Gte {
    data: [u32; 32],
    control: [u32; 32],
    pub commands: u64,
}

#[derive(Clone, Copy)]
struct Mode {
    shift: u32,
    positive: bool,
}

fn signed(value: u32) -> i64 {
    i64::from(value as i16)
}

impl Gte {
    pub fn read_data(&self, register: usize) -> u32 {
        match register & 31 {
            15 => self.data[14],
            28 | 29 => (0..3).fold(0, |rgb, n| {
                rgb | (((self.data[9 + n] as i32 >> 7).clamp(0, 31) as u32) << (n * 5))
            }),
            n => self.data[n],
        }
    }

    pub fn write_data(&mut self, register: usize, value: u32) {
        match register & 31 {
            n @ (1 | 3 | 5 | 8..=11) => self.data[n] = i32::from(value as i16) as u32,
            n @ (7 | 16..=19) => self.data[n] = value & 0xffff,
            15 => {
                self.data[12] = self.data[13];
                self.data[13] = self.data[14];
                self.data[14] = value;
            }
            28 => {
                for n in 0..3 {
                    self.data[9 + n] = ((value >> (n * 5)) & 31) << 7;
                }
            }
            29 | 31 => {}
            30 => {
                self.data[30] = value;
                self.data[31] = if value & 0x8000_0000 != 0 {
                    value.leading_ones()
                } else {
                    value.leading_zeros()
                };
            }
            n => self.data[n] = value,
        }
    }

    pub fn read_control(&self, register: usize) -> u32 {
        self.control[register & 31]
    }

    pub fn write_control(&mut self, register: usize, value: u32) {
        match register & 31 {
            n @ (4 | 12 | 20 | 26 | 27 | 29 | 30) => {
                self.control[n] = i32::from(value as i16) as u32;
            }
            31 => {
                self.control[31] = value & 0x7fff_f000;
                self.summarize_flags();
            }
            n => self.control[n] = value,
        }
    }

    fn summarize_flags(&mut self) {
        self.control[31] = (self.control[31] & 0x7fff_ffff)
            | (u32::from(self.control[31] & 0x7f87_e000 != 0) << 31);
    }

    fn clamp(&mut self, value: i64, min: i64, max: i64, flag: u32) -> i64 {
        if value < min || value > max {
            self.control[31] |= 1 << flag;
        }
        value.clamp(min, max)
    }

    fn ir(&mut self, n: usize, value: i64, positive: bool) {
        self.data[9 + n] = self.clamp(
            value,
            if positive { 0 } else { -32768 },
            32767,
            24 - n as u32,
        ) as u32;
    }

    fn wide(&mut self, n: usize, value: i64) -> i64 {
        if value >= 1_i64 << 43 {
            self.control[31] |= 1 << (30 - n);
        }
        if value < -(1_i64 << 43) {
            self.control[31] |= 1 << (27 - n);
        }
        (value << 20) >> 20 // Sign-extend the internal 44-bit accumulator at each addition.
    }

    fn result(&mut self, n: usize, value: i64, mode: Mode) {
        let value = (self.wide(n, value) >> mode.shift) as i32;
        self.data[25 + n] = value as u32;
        self.ir(n, i64::from(value), mode.positive);
    }

    fn mac0(&mut self, value: i64) {
        if value > i64::from(i32::MAX) {
            self.control[31] |= 1 << 16;
        }
        if value < i64::from(i32::MIN) {
            self.control[31] |= 1 << 15;
        }
        self.data[24] = value as u32;
    }

    fn vector(&self, n: usize) -> [i64; 3] {
        if n == 3 {
            return std::array::from_fn(|n| signed(self.data[9 + n]));
        }
        [
            signed(self.data[n * 2]),
            signed(self.data[n * 2] >> 16),
            signed(self.data[n * 2 + 1]),
        ]
    }

    fn matrix(&self, selection: usize) -> [[i64; 3]; 3] {
        if selection == 3 {
            let red = i64::from(self.data[6] & 255) << 4;
            return [
                [-red, red, signed(self.data[8])],
                [signed(self.control[1]); 3],
                [signed(self.control[2]); 3],
            ];
        }
        std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                let n = row * 3 + column;
                signed(self.control[selection * 8 + n / 2] >> ((n & 1) * 16))
            })
        })
    }

    fn translation(&self, selection: usize) -> [i64; 3] {
        if selection == 3 {
            return [0; 3];
        }
        std::array::from_fn(|n| i64::from(self.control[selection * 8 + 5 + n] as i32))
    }

    fn multiply(
        &mut self,
        matrix: [[i64; 3]; 3],
        vector: [i64; 3],
        translation: [i64; 3],
        mode: Mode,
        far_bug: bool,
    ) -> [i64; 3] {
        std::array::from_fn(|row| {
            let mut accumulator =
                self.wide(row, (translation[row] << 12) + matrix[row][0] * vector[0]);
            if far_bug {
                // FC selection discards the first partial sum, but preserves its error flags.
                self.ir(row, i64::from((accumulator >> mode.shift) as i32), false);
                accumulator = 0;
            }
            for column in 1..3 {
                accumulator = self.wide(row, accumulator + matrix[row][column] * vector[column]);
            }
            self.result(row, accumulator, mode);
            accumulator
        })
    }

    fn perspective(&mut self, vertex: usize, mode: Mode, depth_cue: bool) {
        let old_ir3_flag = self.control[31] & (1 << 22);
        let values = self.multiply(
            self.matrix(0),
            self.vector(vertex),
            self.translation(0),
            mode,
            false,
        );
        // The IR3 flag tests unshifted Z / 4096 with lm=0, independently of stored IR3.
        self.control[31] = (self.control[31] & !(1 << 22)) | old_ir3_flag;
        self.clamp(values[2] >> 12, -32768, 32767, 22);
        let depth = self.clamp(values[2] >> 12, 0, 65535, 18) as u32;
        self.data.copy_within(17..20, 16);
        self.data[19] = depth;
        let numerator = self.control[26] & 0xffff;
        let ratio = if numerator >= depth * 2 {
            self.control[31] |= 1 << 17;
            0x1ffff
        } else {
            reciprocal_divide(numerator, depth)
        };
        let mut screen = [0; 2];
        for (n, coordinate) in screen.iter_mut().enumerate() {
            let value = i64::from(ratio) * signed(self.data[9 + n])
                + i64::from(self.control[24 + n] as i32);
            self.mac0(value);
            *coordinate = self.clamp(value >> 16, -1024, 1023, 14 - n as u32) as u16;
        }
        self.write_data(15, u32::from(screen[0]) | (u32::from(screen[1]) << 16));
        if depth_cue {
            let value =
                i64::from(ratio) * signed(self.control[27]) + i64::from(self.control[28] as i32);
            self.mac0(value);
            self.data[8] = self.clamp(value >> 12, 0, 4096, 12) as u32;
        }
    }

    fn push_color(&mut self) {
        let mut color = self.data[6] & 0xff00_0000;
        for n in 0..3 {
            color |= (self.clamp(
                i64::from(self.data[25 + n] as i32) >> 4,
                0,
                255,
                21 - n as u32,
            ) as u32)
                << (n * 8);
        }
        self.data[20] = self.data[21];
        self.data[21] = self.data[22];
        self.data[22] = color;
    }

    fn modulated_color(&self) -> [i64; 3] {
        std::array::from_fn(|n| {
            i64::from((self.data[6] >> (n * 8)) & 255) * signed(self.data[9 + n]) * 16
        })
    }

    fn interpolate(&mut self, base: [i64; 3], mode: Mode) {
        let far = self.translation(2);
        let factor = signed(self.data[8]);
        for n in 0..3 {
            let delta = (self.wide(n, (far[n] << 12) - base[n]) >> mode.shift) as i32;
            self.ir(n, i64::from(delta), false);
            self.result(n, base[n] + signed(self.data[9 + n]) * factor, mode);
        }
    }

    /// Returns false for undocumented opcodes, so the CPU can report the faulting word.
    #[allow(clippy::too_many_lines)]
    pub fn execute(&mut self, instruction: u32) -> bool {
        let opcode = instruction & 63;
        if !matches!(opcode, 1 | 6 | 0x0c | 0x10..=0x14 | 0x16 | 0x1b | 0x1c | 0x1e | 0x20 | 0x28..=0x2a | 0x2d | 0x2e | 0x30 | 0x3d..=0x3f)
        {
            return false;
        }
        self.control[31] = 0;
        self.commands += 1;
        let mode = Mode {
            shift: if instruction & (1 << 19) != 0 { 12 } else { 0 },
            positive: instruction & (1 << 10) != 0,
        };
        match opcode {
            1 => self.perspective(0, mode, true),
            0x30 => {
                for n in 0..3 {
                    self.perspective(n, mode, n == 2);
                }
            }
            6 => {
                let xy: [(i64, i64); 3] = std::array::from_fn(|n| {
                    (signed(self.data[12 + n]), signed(self.data[12 + n] >> 16))
                });
                self.mac0(
                    xy[0].0 * (xy[1].1 - xy[2].1)
                        + xy[1].0 * (xy[2].1 - xy[0].1)
                        + xy[2].0 * (xy[0].1 - xy[1].1),
                );
            }
            0x2d | 0x2e => {
                let start = if opcode == 0x2d { 17 } else { 16 };
                let sum: i64 = self.data[start..20].iter().map(|n| i64::from(*n)).sum();
                let value = sum * signed(self.control[if opcode == 0x2d { 29 } else { 30 }]);
                self.mac0(value);
                self.data[7] = self.clamp(value >> 12, 0, 65535, 18) as u32;
            }
            0x12 => {
                let matrix = self.matrix(((instruction >> 17) & 3) as usize);
                let vector = self.vector(((instruction >> 15) & 3) as usize);
                let selection = ((instruction >> 13) & 3) as usize;
                self.multiply(
                    matrix,
                    vector,
                    self.translation(selection),
                    mode,
                    selection == 2,
                );
            }
            0x0c => {
                let vector = self.vector(3);
                let diagonal = [
                    signed(self.control[0]),
                    signed(self.control[2]),
                    signed(self.control[4]),
                ];
                for n in 0..3 {
                    let a = (n + 1) % 3;
                    let b = (n + 2) % 3;
                    self.result(n, vector[b] * diagonal[a] - vector[a] * diagonal[b], mode);
                }
            }
            0x28 => {
                for (n, value) in self.vector(3).into_iter().enumerate() {
                    self.result(n, value * value, mode);
                }
            }
            0x3d | 0x3e => {
                let factor = signed(self.data[8]);
                for (n, value) in self.vector(3).into_iter().enumerate() {
                    let base = if opcode == 0x3e {
                        i64::from(self.data[25 + n] as i32) << mode.shift
                    } else {
                        0
                    };
                    self.result(n, base + value * factor, mode);
                }
                self.push_color();
            }
            0x10 | 0x11 | 0x29 | 0x2a => {
                for _ in 0..if opcode == 0x2a { 3 } else { 1 } {
                    let color = self.data[if opcode == 0x2a { 20 } else { 6 }];
                    let base = match opcode {
                        0x11 => self.vector(3).map(|n| n << 12),
                        0x29 => self.modulated_color(),
                        _ => std::array::from_fn(|n| i64::from((color >> (n * 8)) & 255) << 16),
                    };
                    self.interpolate(base, mode);
                    self.push_color();
                }
            }
            _ => {
                let triple = matches!(opcode, 0x16 | 0x20 | 0x3f);
                for vertex in 0..if triple { 3 } else { 1 } {
                    if !matches!(opcode, 0x14 | 0x1c) {
                        self.multiply(self.matrix(1), self.vector(vertex), [0; 3], mode, false);
                    }
                    self.multiply(
                        self.matrix(2),
                        self.vector(3),
                        self.translation(1),
                        mode,
                        false,
                    );
                    if !matches!(opcode, 0x1e | 0x20) {
                        let base = self.modulated_color();
                        if matches!(opcode, 0x13 | 0x14 | 0x16) {
                            self.interpolate(base, mode);
                        } else {
                            for (n, value) in base.into_iter().enumerate() {
                                self.result(n, value, mode);
                            }
                        }
                    }
                    self.push_color();
                }
            }
        }
        self.summarize_flags();
        true
    }
}

// Normalized fixed-point reciprocal refined with integer Newton-Raphson steps.
// Generate the 9-bit seed arithmetically instead of embedding a copied lookup table.
fn reciprocal_divide(numerator: u32, denominator: u32) -> u32 {
    let shift = (denominator as u16).leading_zeros();
    let normalized = denominator << shift;
    let index = (normalized - 0x7fc0) >> 7;
    let seed = (0x40000 / (index + 0x100))
        .div_ceil(2)
        .saturating_sub(0x101)
        + 0x101;
    let correction = (0x0200_0080 - normalized * seed) >> 8;
    let reciprocal = (0x80 + correction * seed) >> 8;
    (((u64::from(numerator << shift) * u64::from(reciprocal) + 0x8000) >> 16) as u32).min(0x1ffff)
}
