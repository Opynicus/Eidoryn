//! Independent streaming RLE / inverse-DCT video decoder.
//! Hardware reference: https://psx-spx.consoledev.net/macroblockdecodermdec/
//! Integer rounding and decode latency are approximate. Input is bounded to 32 words,
//! output to one 8x8 block; DMA pauses when either side cannot make progress.
use crate::EmuError;
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Idle,
    Quant,
    Scale,
    Decode,
}

#[derive(Clone)]
pub struct Mdec {
    operation: Operation,
    command: u32,
    remaining: u32,
    status_count: u16,
    table_index: usize,
    dma_control: u32,
    quant: [[u8; 64]; 2],
    scale: [i16; 64],
    input: VecDeque<u16>,
    output: VecDeque<u32>,
    coefficients: [i32; 64],
    coefficient_index: Option<usize>,
    quant_scale: i32,
    chroma: [[i32; 64]; 2],
    block: usize,
    output_block: usize,
    output_depth: u32,
    output_position: usize,
    pub commands: u64,
    pub blocks_decoded: u64,
}

impl Default for Mdec {
    fn default() -> Self {
        Self {
            operation: Operation::Idle,
            command: 0,
            remaining: 0,
            status_count: 0,
            table_index: 0,
            dma_control: 0,
            quant: [[0; 64]; 2],
            scale: [0; 64],
            input: VecDeque::new(),
            output: VecDeque::new(),
            coefficients: [0; 64],
            coefficient_index: None,
            quant_scale: 0,
            chroma: [[0; 64]; 2],
            block: 4,
            output_block: 4,
            output_depth: 0,
            output_position: 0,
            commands: 0,
            blocks_decoded: 0,
        }
    }
}

impl Mdec {
    pub fn write_control(&mut self, value: u32) {
        if value & 0x8000_0000 != 0 {
            // Reset aborts the stream; quantization and scale tables remain programmed.
            *self = Self {
                quant: self.quant,
                scale: self.scale,
                commands: self.commands,
                blocks_decoded: self.blocks_decoded,
                ..Self::default()
            };
        }
        self.dma_control = value & 0x6000_0000;
    }

    pub fn dma_input_ready(&self) -> bool {
        self.dma_control & (1 << 30) != 0 && self.remaining != 0 && self.input.len() <= 62
    }

    pub fn dma_output_ready(&self) -> bool {
        self.dma_control & (1 << 29) != 0 && !self.output.is_empty()
    }

    pub fn status(&self) -> u32 {
        (u32::from(self.output.is_empty()) << 31)
            | (u32::from(self.input.len() >= 64) << 30)
            | (u32::from(self.operation != Operation::Idle) << 29)
            | (u32::from(self.dma_input_ready()) << 28)
            | (u32::from(self.dma_output_ready()) << 27)
            | ((self.command >> 2) & 0x0780_0000)
            | ((if self.output.is_empty() {
                self.block
            } else {
                self.output_block
            } as u32)
                << 16)
            | u32::from(self.status_count)
    }

    pub fn write_data(&mut self, value: u32) -> Result<(), EmuError> {
        if self.operation == Operation::Idle {
            self.commands += 1;
            self.command = value;
            self.table_index = 0;
            self.block = 4;
            self.coefficient_index = None;
            self.operation = match value >> 29 {
                1 => Operation::Decode,
                2 => Operation::Quant,
                3 => Operation::Scale,
                _ => Operation::Idle,
            };
            self.remaining = match self.operation {
                Operation::Decode => value & 0xffff,
                Operation::Quant => {
                    if value & 1 == 0 {
                        16
                    } else {
                        32
                    }
                }
                Operation::Scale => 32,
                Operation::Idle => 0,
            };
            self.status_count = if self.operation == Operation::Idle {
                value as u16
            } else {
                self.remaining.wrapping_sub(1) as u16
            };
            if self.remaining == 0 {
                self.operation = Operation::Idle;
            }
            return Ok(());
        }
        if self.remaining == 0 || self.input.len() > 62 {
            return Err(EmuError::Mdec(
                "input FIFO full; DMA/CPU must wait for output to drain",
            ));
        }
        self.remaining -= 1;
        self.status_count = self.remaining.wrapping_sub(1) as u16;
        match self.operation {
            Operation::Quant => {
                for byte in value.to_le_bytes() {
                    self.quant[self.table_index / 64][self.table_index % 64] = byte;
                    self.table_index += 1;
                }
            }
            Operation::Scale => {
                for half in [value as i16, (value >> 16) as i16] {
                    self.scale[self.table_index] = half;
                    self.table_index += 1;
                }
            }
            Operation::Decode => {
                self.input.push_back(value as u16);
                self.input.push_back((value >> 16) as u16);
            }
            Operation::Idle => unreachable!(),
        }
        self.pump();
        Ok(())
    }

    fn pump(&mut self) {
        if self.operation == Operation::Decode {
            while self.output.is_empty() {
                let Some(code) = self.input.pop_front() else {
                    break;
                };
                let value = i32::from((code << 6) as i16 >> 6);
                let first = self.coefficient_index.is_none();
                let index = if let Some(previous) = self.coefficient_index {
                    previous + usize::from(code >> 10) + 1
                } else {
                    if code == 0xfe00 {
                        continue;
                    }
                    self.quant_scale = i32::from(code >> 10);
                    self.coefficients.fill(0);
                    0
                };
                if index < 64 {
                    let chroma = self.command & (1 << 28) != 0 && self.block >= 4;
                    let quant = i32::from(self.quant[usize::from(chroma)][index]);
                    let coefficient = if self.quant_scale == 0 {
                        value * 2
                    } else if first {
                        value * quant
                    } else {
                        (value * quant * self.quant_scale + 4) >> 3
                    };
                    self.coefficients[if self.quant_scale == 0 {
                        index
                    } else {
                        SCAN[index]
                    }] = coefficient.clamp(-1024, 1023);
                    self.coefficient_index = Some(index);
                }
                if index >= 63 {
                    self.finish_block();
                }
            }
        }
        if self.remaining == 0 && self.input.is_empty() {
            self.operation = Operation::Idle;
        }
    }

    fn finish_block(&mut self) {
        self.coefficient_index = None;
        self.blocks_decoded += 1;
        let values = idct(self.coefficients, &self.scale);
        let depth = (self.command >> 27) & 3;
        if depth >= 2 && self.block >= 4 {
            self.chroma[self.block - 4] = values;
            self.block = (self.block + 1) % 6;
            return;
        }
        self.output_block = self.block;
        self.output_depth = depth;
        self.output_position = 0;
        let unsigned = self.command & (1 << 26) == 0;
        let sample =
            |value: i32| -> u8 { (value.clamp(-128, 127) as u8) ^ if unsigned { 128 } else { 0 } };
        // One 8x8 block is at most 192 RGB24 bytes. Packing on the stack
        // avoids a heap allocation for every decoded output block.
        let mut bytes = [0; 192];
        let length = [32, 64, 192, 128][depth as usize];
        for (n, luminance) in values.into_iter().enumerate() {
            if depth < 2 {
                let luminance = (luminance << 23) >> 23; // Mono output wraps to signed 9 bits.
                let value = sample(luminance);
                if depth == 1 {
                    bytes[n] = value;
                } else if n & 1 == 0 {
                    bytes[n / 2] = value >> 4;
                } else {
                    bytes[n / 2] |= value & 0xf0;
                }
            } else {
                let x = n % 8 + (self.block % 2) * 8;
                let y = n / 8 + (self.block / 2) * 8;
                let chroma_index = x / 2 + (y / 2) * 8;
                let red = self.chroma[0][chroma_index];
                let blue = self.chroma[1][chroma_index];
                let rgb = [
                    sample(luminance + ((359 * red + 128) >> 8)),
                    sample(luminance + ((-183 * red - 88 * blue + 128) >> 8)),
                    sample(luminance + ((454 * blue + 128) >> 8)),
                ];
                if depth == 2 {
                    bytes[n * 3..n * 3 + 3].copy_from_slice(&rgb);
                } else {
                    let rgb = rgb.map(|v| ((u16::from(v) + 4) >> 3).min(31));
                    let pixel = rgb[0]
                        | (rgb[1] << 5)
                        | (rgb[2] << 10)
                        | (((self.command >> 25) as u16 & 1) << 15);
                    bytes[n * 2..n * 2 + 2].copy_from_slice(&pixel.to_le_bytes());
                }
            }
        }
        self.output.extend(
            bytes[..length]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b)),
        );
        if depth >= 2 {
            self.block += 1;
        }
    }

    pub fn read_data(&mut self) -> u32 {
        let word = self.output.pop_front().unwrap_or(u32::MAX);
        self.output_position += 1;
        self.pump();
        word
    }

    /// DMA1 scatters the four 8x8 color blocks into one contiguous 16x16 bitmap.
    pub fn read_dma(&mut self) -> (u32, i32) {
        let offset = if self.output_depth >= 2 {
            let row_words = if self.output_depth == 2 { 6 } else { 4 };
            let sequential = self.output_block * row_words * 8 + self.output_position;
            let row = self.output_block / 2 * 8 + self.output_position / row_words;
            let column = self.output_block % 2 * row_words + self.output_position % row_words;
            (row * row_words * 2 + column) as i32 - sequential as i32
        } else {
            0
        };
        (self.read_data(), offset * 4)
    }
}

fn idct(mut source: [i32; 64], scale: &[i16; 64]) -> [i32; 64] {
    for _ in 0..2 {
        source = std::array::from_fn(|position| {
            let row = position / 8;
            let column = position % 8;
            let sum: i64 = (0..8)
                .map(|n| i64::from(source[row + n * 8]) * i64::from(scale[column + n * 8] >> 3))
                .sum();
            ((sum + 0xfff) >> 13) as i32
        });
    }
    source
}

const SCAN: [usize; 64] = {
    let mut result = [0; 64];
    let mut index = 0;
    let mut diagonal: usize = 0;
    while diagonal < 15 {
        let low = diagonal.saturating_sub(7);
        let high = if diagonal < 8 { diagonal } else { 7 };
        let mut step = 0;
        while step <= high - low {
            let x = if diagonal.is_multiple_of(2) {
                low + step
            } else {
                high - step
            };
            result[index] = (diagonal - x) * 8 + x;
            index += 1;
            step += 1;
        }
        diagonal += 1;
    }
    result
};
