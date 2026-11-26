//! ARM9 geometry command processor. FIFO writes are consumed synchronously;
//! `SWAP_BUFFERS` publishes a completed primitive list for the next display pass.
mod matrix;
mod raster;
use super::{H, W, vram::Vram};
use matrix::MatrixState;
use serde::{Deserialize, Serialize};

const MAX_VERTICES: u16 = 6_144;
const MAX_POLYGONS: usize = 2_048;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Vertex {
    clip: [f32; 4],
    uv: [f32; 2],
    color: [f32; 3],
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct Polygon {
    vertices: [Vertex; 4],
    count: u8,
    attributes: u32,
    texture: u32,
    palette: u32,
    viewport: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct Decoder {
    latch: [u8; 4],
    packed: u32,
    opcode: u8,
    args: Vec<u32>,
}

impl Decoder {
    fn valid(&self) -> bool {
        if self.opcode == 0 {
            self.args.is_empty()
        } else {
            self.args.len() < usize::from(parameters(self.opcode))
        }
    }
}

// Parameter counts also allow the parser to skip unsupported lighting and
// geometry-test operations without treating their parameters as new opcodes.
fn parameters(command: u8) -> u8 {
    match command {
        0x10
        | 0x12..=0x14
        | 0x20..=0x22
        | 0x24..=0x2b
        | 0x30..=0x33
        | 0x40
        | 0x50
        | 0x60
        | 0x72 => 1,
        0x23 | 0x71 => 2,
        0x1b | 0x1c | 0x70 => 3,
        0x1a => 9,
        0x17 | 0x19 => 12,
        0x16 | 0x18 => 16,
        0x34 => 32,
        _ => 0,
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Geometry {
    matrices: MatrixState,
    decoder: Decoder,
    irq_mode: u8,
    pos: [i16; 3],
    normal: [i16; 3],
    uv: [i16; 2],
    color: [f32; 3],
    mode: u32,
    viewport: u32,
    attributes: u32,
    active_attributes: u32,
    texture: u32,
    palette: u32,
    assembly: Vec<Vertex>,
    strip_odd: bool,
    vertices: u16,
    pending: Vec<Polygon>,
    front: Vec<Polygon>,
    swap: u32,
    clear_color: u32,
    clear_depth: u16,
    alpha_ref: u8,
    box_visible: bool,
    position_result: [i32; 4],
}
impl Default for Geometry {
    fn default() -> Self {
        Self {
            matrices: MatrixState::default(),
            decoder: Decoder::default(),
            irq_mode: 0,
            pos: [0; 3],
            normal: [0; 3],
            uv: [0; 2],
            color: [31.; 3],
            mode: 0,
            viewport: 0xbfff_0000,
            attributes: 0,
            active_attributes: 0,
            texture: 0,
            palette: 0,
            assembly: Vec::new(),
            strip_odd: false,
            vertices: 0,
            pending: Vec::new(),
            front: Vec::new(),
            swap: 0,
            clear_color: 0,
            clear_depth: 0x7fff,
            alpha_ref: 0,
            box_visible: false,
            position_result: [0; 4],
        }
    }
}

impl Geometry {
    pub(crate) fn valid(&self) -> bool {
        self.decoder.valid()
            && self.vertices <= MAX_VERTICES
            && self.assembly.len() < if self.mode & 1 == 0 { 3 } else { 4 }
            && self.pending.len() <= MAX_POLYGONS
            && self.front.len() <= MAX_POLYGONS
            && self
                .front
                .iter()
                .chain(&self.pending)
                .all(|p| (3..=4).contains(&p.count))
            && self.mode < 4
            && self.irq_mode < 4
    }

    pub(super) fn irq(&self) -> bool {
        matches!(self.irq_mode, 1 | 2)
    }

    pub(super) fn read8(&self, addr: u32) -> u8 {
        let word = match addr & !3 {
            0x0400_0340 => u32::from(self.alpha_ref),
            0x0400_0350 => self.clear_color,
            0x0400_0354 => u32::from(self.clear_depth),
            0x0400_0600 => {
                let (position, projection, error) = self.matrices.levels();
                u32::from(self.box_visible) << 1
                    | u32::from(position) << 8
                    | u32::from(projection) << 13
                    | u32::from(error) << 15
                    | (3 << 25)
                    | u32::from(self.irq_mode) << 30
            }
            0x0400_0604 => self.pending.len() as u32 | u32::from(self.vertices) << 16,
            0x0400_0620..=0x0400_062c => {
                self.position_result[((addr - 0x0400_0620) / 4) as usize] as u32
            }
            0x0400_0640..=0x0400_067c => {
                self.matrices.clip_word(((addr - 0x0400_0640) / 4) as usize)
            }
            0x0400_0680..=0x0400_06a0 => self
                .matrices
                .vector_word(((addr - 0x0400_0680) / 4) as usize),
            _ => 0,
        };
        word.to_le_bytes()[(addr & 3) as usize]
    }

    pub(super) fn write8(&mut self, addr: u32, byte: u8) {
        match addr {
            0x0400_0340 => self.alpha_ref = byte & 31,
            0x0400_0350..=0x0400_0353 => {
                let mut color = self.clear_color.to_le_bytes();
                color[(addr & 3) as usize] = byte;
                self.clear_color = u32::from_le_bytes(color);
            }
            0x0400_0354..=0x0400_0355 => {
                let mut depth = self.clear_depth.to_le_bytes();
                depth[(addr & 1) as usize] = byte;
                self.clear_depth = u16::from_le_bytes(depth) & 0x7fff;
            }
            0x0400_0601 if byte & 0x80 != 0 => self.matrices.clear_error(),
            0x0400_0603 => self.irq_mode = byte >> 6,
            0x0400_0400..=0x0400_05ff => {
                self.decoder.latch[(addr & 3) as usize] = byte;
                if addr & 3 == 3 {
                    let value = u32::from_le_bytes(self.decoder.latch);
                    if addr < 0x0400_0440 {
                        self.fifo(value);
                    } else {
                        self.direct(((addr - 0x0400_0400) >> 2) as u8, value);
                    }
                }
            }
            _ => {}
        }
    }
    fn fifo(&mut self, word: u32) {
        if self.decoder.opcode != 0 {
            self.push_arg(word);
        } else {
            self.decoder.packed = word;
        }
        while self.decoder.opcode == 0 && self.decoder.packed != 0 {
            let op = self.decoder.packed as u8;
            self.decoder.packed >>= 8;
            if parameters(op) == 0 {
                self.execute(op, &[]);
            } else {
                self.decoder.opcode = op;
            }
        }
    }
    fn direct(&mut self, op: u8, word: u32) {
        if parameters(op) == 0 {
            self.execute(op, &[]);
            return;
        }
        if self.decoder.opcode != op {
            self.decoder.args.clear();
            self.decoder.opcode = op;
        }
        self.push_arg(word);
    }
    fn push_arg(&mut self, word: u32) {
        self.decoder.args.push(word);
        let op = self.decoder.opcode;
        if self.decoder.args.len() == usize::from(parameters(op)) {
            let mut args = std::mem::take(&mut self.decoder.args);
            self.decoder.opcode = 0;
            self.execute(op, &args);
            args.clear();
            self.decoder.args = args;
        }
    }
    fn execute(&mut self, op: u8, args: &[u32]) {
        let arg = args.first().copied().unwrap_or(0);
        let first = arg as i16;
        let second = (arg >> 16) as i16;
        match op {
            0x10..=0x1c => self.matrices.command(op, args),
            0x20 => self.color = [0, 5, 10].map(|i| ((arg >> i) & 31) as f32),
            0x21 => self.normal = unpack10(arg),
            0x22 => self.uv = [first, second],
            0x23 => self.vertex([first, second, args[1] as i16]),
            0x24 => self.vertex(unpack10(arg).map(|n| n << 6)),
            0x25 => self.vertex([first, second, self.pos[2]]),
            0x26 => self.vertex([first, self.pos[1], second]),
            0x27 => self.vertex([self.pos[0], first, second]),
            0x28 => {
                let delta = unpack10(arg);
                self.vertex(std::array::from_fn(|i| self.pos[i].wrapping_add(delta[i])));
            }
            0x29 => self.attributes = arg,
            0x2a => self.texture = arg,
            0x2b => self.palette = arg & 0x1fff,
            0x30 if arg & (1 << 15) != 0 => {
                self.color = [0, 5, 10].map(|i| ((arg >> i) & 31) as f32);
            }
            0x40 => {
                self.active_attributes = self.attributes;
                self.mode = arg & 3;
                self.assembly.clear();
                self.strip_odd = false;
            }
            0x41 => self.assembly.clear(),
            0x50 => {
                std::mem::swap(&mut self.pending, &mut self.front);
                self.pending.clear();
                self.vertices = 0;
                self.assembly.clear();
                self.swap = arg;
            }
            0x60 => self.viewport = arg,
            0x70 => self.test_box(args),
            0x71 => {
                self.position_result = self
                    .matrices
                    .vertex([first, second, args[1] as i16])
                    .map(|v| (v * 4096.) as i32);
            }
            _ => {}
        }
    }
    fn test_box(&mut self, args: &[u32]) {
        let origin = [args[0] as i16, (args[0] >> 16) as i16, args[1] as i16];
        let size = [
            (args[1] >> 16) as i16,
            args[2] as i16,
            (args[2] >> 16) as i16,
        ];
        let corners: [_; 8] = std::array::from_fn(|i| {
            self.matrices.vertex(std::array::from_fn(|axis| {
                if (i >> axis) & 1 == 0 {
                    origin[axis]
                } else {
                    origin[axis].wrapping_add(size[axis])
                }
            }))
        });
        self.box_visible = !(0..3).any(|axis| {
            corners.iter().all(|v| v[axis] < -v[3]) || corners.iter().all(|v| v[axis] > v[3])
        });
    }
    fn vertex(&mut self, pos: [i16; 3]) {
        self.pos = pos;
        if self.vertices >= MAX_VERTICES {
            return;
        }
        self.vertices += 1;
        self.assembly.push(Vertex {
            clip: self.matrices.vertex(pos),
            uv: self
                .matrices
                .texcoord(self.uv, self.normal, pos, self.texture >> 30),
            color: self.color,
        });
        let count = if self.mode & 1 == 0 { 3 } else { 4 };
        if self.assembly.len() < count {
            return;
        }
        let mut vertices = [Vertex::default(); 4];
        vertices[..count].copy_from_slice(&self.assembly);
        match self.mode {
            2 => {
                if self.strip_odd {
                    vertices.swap(0, 1);
                }
                self.strip_odd = !self.strip_odd;
                self.assembly.remove(0);
            }
            3 => {
                vertices.swap(2, 3);
                self.assembly.drain(..2);
            }
            _ => self.assembly.clear(),
        }
        if self.pending.len() < MAX_POLYGONS {
            self.pending.push(Polygon {
                vertices,
                count: count as u8,
                attributes: self.active_attributes,
                texture: self.texture,
                palette: self.palette,
                viewport: self.viewport,
            });
        }
    }
}
fn unpack10(word: u32) -> [i16; 3] {
    [0, 10, 20].map(|shift| (((word >> shift) as i16) << 6) >> 6)
}
