//! The geometry engine uses row vectors and signed 20.12 matrix entries.
use serde::{Deserialize, Serialize};

const ONE: i32 = 1 << 12;
type Matrix = [[i32; 4]; 4];
const IDENTITY: Matrix = [
    [ONE, 0, 0, 0],
    [0, ONE, 0, 0],
    [0, 0, ONE, 0],
    [0, 0, 0, ONE],
];

fn dot(row: [i32; 4], matrix: &Matrix, column: usize) -> i64 {
    row.into_iter().zip(matrix).fold(0_i64, |sum, (x, m)| {
        sum.wrapping_add(i64::from(x) * i64::from(m[column]))
    })
}

fn transform(row: [i32; 4], matrix: &Matrix) -> [i32; 4] {
    std::array::from_fn(|col| (dot(row, matrix, col) >> 12) as i32)
}

fn multiply(left: &Matrix, right: &Matrix) -> Matrix {
    left.map(|row| transform(row, right))
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
enum Mode {
    #[default]
    Projection,
    Position,
    PositionVector,
    Texture,
}

#[derive(Deserialize, Serialize)]
pub(super) struct MatrixState {
    mode: Mode,
    projection: Matrix,
    position: Matrix,
    vector: Matrix,
    texture: Matrix,
    projection_slot: Matrix,
    texture_slot: Matrix,
    position_slots: [(Matrix, Matrix); 32],
    projection_level: bool,
    texture_level: bool,
    position_level: u8,
    error: bool,
}

impl Default for MatrixState {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            projection: IDENTITY,
            position: IDENTITY,
            vector: IDENTITY,
            texture: IDENTITY,
            projection_slot: IDENTITY,
            texture_slot: IDENTITY,
            position_slots: [(IDENTITY, IDENTITY); 32],
            projection_level: false,
            texture_level: false,
            position_level: 0,
            error: false,
        }
    }
}

impl MatrixState {
    pub(super) fn command(&mut self, command: u8, params: &[u32]) {
        match command {
            0x10 => {
                self.mode = match params[0] & 3 {
                    0 => Mode::Projection,
                    1 => Mode::Position,
                    2 => Mode::PositionVector,
                    _ => Mode::Texture,
                }
            }
            0x11..=0x14 => self.stack(command, params.first().copied().unwrap_or(0) as u8),
            0x15..=0x1c => {
                let mut operand = IDENTITY;
                match command {
                    0x16 | 0x18 => {
                        for (entry, &word) in operand.iter_mut().flatten().zip(params) {
                            *entry = word as i32;
                        }
                    }
                    0x17 | 0x19 | 0x1a => {
                        for (row, words) in operand.iter_mut().zip(params.as_chunks::<3>().0) {
                            row[..3].copy_from_slice(&[
                                words[0] as i32,
                                words[1] as i32,
                                words[2] as i32,
                            ]);
                        }
                    }
                    0x1b => {
                        for i in 0..3 {
                            operand[i][i] = params[i] as i32;
                        }
                    }
                    0x1c => {
                        operand[3][..3].copy_from_slice(&[
                            params[0] as i32,
                            params[1] as i32,
                            params[2] as i32,
                        ]);
                    }
                    _ => {}
                }
                let apply = |current: &mut Matrix| {
                    *current = if command < 0x18 {
                        operand
                    } else {
                        multiply(&operand, current)
                    };
                };
                match self.mode {
                    Mode::Projection => apply(&mut self.projection),
                    Mode::Position => apply(&mut self.position),
                    Mode::PositionVector => {
                        apply(&mut self.position);
                        if command != 0x1b {
                            apply(&mut self.vector);
                        }
                    }
                    Mode::Texture => apply(&mut self.texture),
                }
            }
            _ => {}
        }
    }

    fn stack(&mut self, command: u8, arg: u8) {
        let store = matches!(command, 0x11 | 0x13);
        let indexed = matches!(command, 0x13 | 0x14);
        match self.mode {
            Mode::Position | Mode::PositionVector => {
                // The subtract wraps modulo 64, so a signed six-bit pop offset
                // also permits reloading a previously discarded position.
                if command == 0x12 {
                    self.position_level = self.position_level.wrapping_sub(arg) & 63;
                }
                let address = if indexed {
                    arg & 31
                } else {
                    self.position_level
                };
                self.error |= address >= 31;
                let slot = &mut self.position_slots[usize::from(address & 31)];
                if store {
                    *slot = (self.position, self.vector);
                } else {
                    (self.position, self.vector) = *slot;
                }
                if command == 0x11 {
                    self.position_level = self.position_level.wrapping_add(1) & 63;
                }
            }
            Mode::Projection | Mode::Texture => {
                let (current, slot, level) = if matches!(self.mode, Mode::Projection) {
                    (
                        &mut self.projection,
                        &mut self.projection_slot,
                        &mut self.projection_level,
                    )
                } else {
                    (
                        &mut self.texture,
                        &mut self.texture_slot,
                        &mut self.texture_level,
                    )
                };
                if !indexed {
                    self.error |= *level == store;
                    *level = !*level;
                }
                if store {
                    *slot = *current;
                } else {
                    *current = *slot;
                }
            }
        }
    }

    pub(super) fn levels(&self) -> (u8, bool, bool) {
        (self.position_level & 31, self.projection_level, self.error)
    }

    pub(super) fn clear_error(&mut self) {
        self.error = false;
        self.projection_level = false;
    }

    pub(super) fn clip_word(&self, index: usize) -> u32 {
        (dot(self.position[index / 4], &self.projection, index % 4) >> 12) as u32
    }

    pub(super) fn vector_word(&self, index: usize) -> u32 {
        self.vector[index / 3][index % 3] as u32
    }

    pub(super) fn vertex(&self, pos: [i16; 3]) -> [f32; 4] {
        let [x, y, z] = pos.map(i32::from);
        let clip = multiply(&self.position, &self.projection);
        transform([x, y, z, ONE], &clip).map(|v| v as f32 / ONE as f32)
    }

    pub(super) fn texcoord(
        &self,
        input: [i16; 2],
        normal: [i16; 3],
        pos: [i16; 3],
        mode: u32,
    ) -> [f32; 2] {
        let [coord_s, coord_t] = input.map(i32::from);
        std::array::from_fn(|axis| {
            let transformed = match mode & 3 {
                1 => dot([coord_s, coord_t, 1, 1], &self.texture, axis) >> 12,
                // Convert 1.9 normals or 4.12 vertices to 12.4 texture coordinates.
                2 | 3 => {
                    let (source, shift) = if mode & 3 == 2 {
                        (normal, 17)
                    } else {
                        (pos, 20)
                    };
                    let [x, y, z] = source.map(i32::from);
                    i64::from(input[axis]) + (dot([x, y, z, 0], &self.texture, axis) >> shift)
                }
                _ => i64::from(input[axis]),
            };
            f32::from(transformed as i16) / 16.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_and_scale_pre_multiply_before_orthographic_projection() {
        let mut m = MatrixState::default();
        let one = ONE as u32;
        m.command(0x1b, &[one / 2, one, one]);
        m.command(0x10, &[2]);
        m.command(0x1c, &[one, 0, 0]);
        m.command(0x1b, &[2 * one, one, one]);
        let vertex = m.vertex([ONE as i16, 0, 0]);
        assert_eq!(
            vertex.map(f32::to_bits),
            [1.5, 0.0, 0.0, 1.0_f32].map(f32::to_bits)
        );
        assert_eq!(m.clip_word(12), one / 2);
        assert_eq!(m.vector_word(0), one);
    }

    #[test]
    fn position_stack_restores_paired_matrices_across_modes() {
        let mut m = MatrixState::default();
        m.command(0x10, &[2]);
        m.command(0x1a, &[0, 4096, 0, (-4096_i32) as u32, 0, 0, 0, 0, 4096]);
        m.command(0x10, &[1]);
        m.command(0x11, &[]);
        m.command(0x10, &[2]);
        m.command(0x15, &[]);
        m.command(0x12, &[1]);
        assert_eq!(m.vector_word(1), 4096);
        assert_eq!(m.clip_word(4), (-4096_i32) as u32);
        assert_eq!(m.levels(), (0, false, false));
        m.command(0x12, &[1]); // Wraps to 63, which mirrors physical slot 31.
        assert_eq!(m.levels(), (31, false, true));
        m.clear_error();
        assert_eq!(m.levels(), (31, false, false));
    }

    #[test]
    fn texture_vectors_preserve_fractional_signed_guest_units() {
        let mut m = MatrixState::default();
        m.command(0x10, &[3]);
        m.command(0x1b, &[8 << 12, 4 << 12, 4096]);
        m.command(0x1c, &[2 << 12, 4 << 12, 0]);
        let coordinates = [16, -32];
        let normal = [256, -256, 0];
        let vertex = [4096, 2048, 0];
        for (mode, expected) in [
            (0, [1.0, -2.0]),
            (1, [9.0, -7.0]),
            (2, [5.0, -4.0]),
            (3, [9.0, 0.0]),
        ] {
            assert_eq!(
                m.texcoord(coordinates, normal, vertex, mode)
                    .map(f32::to_bits),
                expected.map(f32::to_bits)
            );
        }
    }
}
