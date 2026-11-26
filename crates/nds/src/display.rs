//! Software output for the DS main and sub 2D engines, in physical LCD order.
mod bg;
pub(crate) mod geometry;
mod sprites;
mod texture;
mod vram;
use serde::{Deserialize, Serialize};
use vram::{ARM7, BG_A, BG_B, OBJ_A, OBJ_B, Vram};

const W: usize = 256;
const H: usize = 192;
const FRAME: usize = W * H * 3;
const REG_SIZE: usize = 0x70;

#[derive(Serialize, Deserialize)]
pub(crate) struct Display {
    vram: Vram,
    #[serde(skip)]
    pub(crate) geometry: geometry::Geometry,
    palette: Vec<u8>,
    oam: Vec<u8>,
    regs: Vec<u8>,
    power: u16,
    pixels: Vec<u8>,
    dirty: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            vram: Vram::default(),
            geometry: geometry::Geometry::default(),
            palette: vec![0; 2048],
            oam: vec![0; 2048],
            regs: vec![0; 2 * REG_SIZE],
            power: 0x820f,
            pixels: vec![255; 2 * FRAME],
            dirty: true,
        }
    }
}

fn half(data: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([data[i], data[i + 1]])
}
fn word(data: &[u8], i: usize) -> u32 {
    u32::from(half(data, i)) | u32::from(half(data, i + 2)) << 16
}
fn rgb(color: u16) -> [u8; 3] {
    [0, 5, 10].map(|shift| {
        let v = ((color >> shift) & 31u16) as u8;
        (v << 3) | (v >> 2)
    })
}

fn bank_control(addr: u32) -> Option<usize> {
    match addr {
        0x0400_0240..=0x0400_0246 => Some((addr - 0x0400_0240) as usize),
        0x0400_0248..=0x0400_0249 => Some((addr - 0x0400_0241) as usize),
        _ => None,
    }
}

fn engine_reg(addr: u32) -> Option<usize> {
    match addr {
        0x0400_0000..=0x0400_006f => Some((addr & 0xff) as usize),
        0x0400_1000..=0x0400_106f => Some(REG_SIZE + (addr & 0xff) as usize),
        _ => None,
    }
}

fn window(addr: u32) -> Option<(usize, usize)> {
    let (target, mask) = match addr >> 21 {
        0x30 => (BG_A, 0x7ffff),
        0x31 => (BG_B, 0x1ffff),
        0x32 => (OBJ_A, 0x3ffff),
        0x33 => (OBJ_B, 0x1ffff),
        _ => return None,
    };
    Some((target, addr as usize & mask))
}

impl Display {
    pub(crate) fn valid_state(&self) -> bool {
        self.vram.valid()
            && self.geometry.valid()
            && self.palette.len() == 2048
            && self.oam.len() == 2048
            && self.regs.len() == 2 * REG_SIZE
            && self.pixels.len() == 2 * FRAME
    }

    pub(crate) fn arm7_read8(&self, addr: u32) -> u8 {
        match addr {
            0x0400_0240 => {
                u8::from(self.vram.control(2) & 0x87 == 0x82)
                    | (u8::from(self.vram.control(3) & 0x87 == 0x82) << 1)
            }
            0x0600_0000..=0x06ff_ffff => self.vram.read(ARM7, addr as usize & 0x3ffff),
            _ => 0,
        }
    }

    pub(crate) fn arm7_write8(&mut self, addr: u32, value: u8) {
        if (0x0600_0000..=0x06ff_ffff).contains(&addr) {
            self.vram.write(ARM7, addr as usize & 0x3ffff, value);
            self.dirty = true;
        }
    }

    pub(crate) fn gx_fifo_irq(&self) -> bool {
        self.geometry.irq()
    }

    pub(crate) fn read8(&self, addr: u32) -> u8 {
        if (0x0400_0340..=0x0400_06a3).contains(&addr) {
            return self.geometry.read8(addr);
        }
        if let Some(i) = engine_reg(addr) {
            return self.regs[i];
        }
        if let Some(bank) = bank_control(addr) {
            return self.vram.control(bank);
        }
        if let Some((target, offset)) = window(addr) {
            return self.vram.read(target, offset);
        }
        match addr {
            0x0400_0304 => self.power as u8,
            0x0400_0305 => (self.power >> 8) as u8,
            0x0500_0000..=0x05ff_ffff => self.palette[addr as usize & 2047],
            0x0680_0000..=0x06ff_ffff => self.vram.lcdc_read(addr as usize & 0xfffff),
            0x0700_0000..=0x07ff_ffff => self.oam[addr as usize & 2047],
            _ => 0,
        }
    }

    pub(crate) fn write8(&mut self, addr: u32, value: u8) {
        self.dirty = true;
        if (0x0400_0340..=0x0400_06a3).contains(&addr) {
            self.geometry.write8(addr, value);
            return;
        }
        if let Some(i) = engine_reg(addr) {
            self.regs[i] = value;
            return;
        }
        if let Some(bank) = bank_control(addr) {
            self.vram.set_control(bank, value);
            return;
        }
        if let Some((target, offset)) = window(addr) {
            self.vram.write(target, offset, value);
            return;
        }
        match addr {
            0x0400_0304 => self.power = (self.power & 0xff00) | u16::from(value),
            0x0400_0305 => self.power = (self.power & 255) | (u16::from(value) << 8),
            0x0500_0000..=0x05ff_ffff => self.palette[addr as usize & 2047] = value,
            0x0680_0000..=0x06ff_ffff => self.vram.lcdc_write(addr as usize & 0xfffff, value),
            0x0700_0000..=0x07ff_ffff => self.oam[addr as usize & 2047] = value,
            _ => {}
        }
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub(crate) fn render(&mut self) {
        if !self.dirty {
            return;
        }
        let mut frame = std::mem::take(&mut self.pixels);
        let (top, bottom) = frame.split_at_mut(FRAME);
        let (main, sub) = if self.power & 0x8000 != 0 {
            (top, bottom)
        } else {
            (bottom, top)
        };
        self.render_engine(0, main);
        self.render_engine(1, sub);
        self.pixels = frame;
        self.dirty = false;
    }

    fn render_engine(&self, engine: usize, out: &mut [u8]) {
        let regs = &self.regs[engine * REG_SIZE..(engine + 1) * REG_SIZE];
        let control = word(regs, 0);
        if self.power & 1 == 0 || self.power & [2, 0x200][engine] == 0 || control & 0x80 != 0 {
            out.fill(255);
            return;
        }
        match (control >> 16) & if engine == 0 { 3 } else { 1 } {
            1 => {
                let backdrop = rgb(half(&self.palette, engine * 1024));
                out.as_chunks_mut::<3>().0.fill(backdrop);
                for priority in (0..4).rev() {
                    for layer in (0..4).rev() {
                        let cnt = half(regs, 8 + 2 * layer);
                        if control & (0x100 << layer) != 0 && cnt & 3 == priority {
                            if engine == 0 && layer == 0 && control & 8 != 0 {
                                self.geometry.render(&self.vram, half(regs, 0x60), out);
                            } else {
                                self.draw_bg(engine, layer, cnt, control, out);
                            }
                        }
                    }
                    if control & (1 << 12) != 0 {
                        self.draw_sprites(engine, control, priority, out);
                    }
                }
            }
            2 => {
                let base = ((control as usize >> 18) & 3) * 0x20000;
                for (i, pixel) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let at = base + i * 2;
                    let color =
                        u16::from_le_bytes([self.vram.lcdc_read(at), self.vram.lcdc_read(at + 1)]);
                    *pixel = rgb(color);
                }
            }
            _ => out.fill(255),
        }
        let master = half(regs, 0x6c);
        let level = (master & 31).min(16);
        if level != 0 && matches!(master >> 14, 1 | 2) {
            for channel in out {
                let v = u16::from(*channel);
                *channel = if master >> 14 == 1 {
                    v + (255 - v) * level / 16
                } else {
                    v - v * level / 16
                } as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests;
