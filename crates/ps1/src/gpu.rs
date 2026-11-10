//! Original GPU command processor, ordered software/Vulkan rendering and scanout timing.

use crate::{EmuError, timers::Timers};
use std::sync::Arc;
mod rasterizer;
mod renderer;
mod renderer_engine;
#[cfg(feature = "vulkan")]
mod vulkan;
use rasterizer::{DrawState, RasterCommand};
pub use renderer::RendererMode;
use renderer::{FrameReadback, Renderer};

#[derive(Clone, Copy)]
struct DisplaySettings {
    dimensions: (usize, usize),
    start: (usize, usize),
    disabled: bool,
    rgb24: bool,
}

pub const VRAM_WIDTH: usize = 1024;
pub const VRAM_HEIGHT: usize = 512;
/// Immutable pixels from the end of the visible field, before the guest's
/// VBlank handler can swap or clear its drawing buffers.
#[derive(Clone)]
pub struct DisplayFrame {
    pub dimensions: (usize, usize),
    pub rgb: Arc<[u8]>,
}

#[derive(Clone, Copy, Default)]
struct Vertex {
    position: (i32, i32),
    color: [i64; 3],
    uv: [i64; 2],
}

#[derive(Clone, Copy)]
struct Texture {
    clut: u16,
    raw: bool,
}

#[derive(Clone, Copy)]
struct DrawStyle {
    texture: Option<Texture>,
    transparent: bool,
    dither: bool,
}

#[derive(Clone, Copy)]
struct Polyline {
    previous: Vertex,
    next_color: Option<[i64; 3]>,
    shaded: bool,
    style: DrawStyle,
}

#[derive(Clone, Copy, Debug)]
struct Transfer {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    index: usize,
}

impl Transfer {
    fn new(position: u32, size: u32) -> Self {
        Self {
            x: (position & 1023) as usize,
            y: ((position >> 16) & 511) as usize,
            width: ((size.wrapping_sub(1) & 1023) + 1) as usize,
            height: (((size >> 16).wrapping_sub(1) & 511) + 1) as usize,
            index: 0,
        }
    }

    fn next(&mut self) -> Option<usize> {
        if self.index == self.width * self.height {
            return None;
        }
        let x = (self.x + self.index % self.width) & 1023;
        let y = (self.y + self.index / self.width) & 511;
        self.index += 1;
        Some(y * VRAM_WIDTH + x)
    }
}

#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)] // Independent GPU hardware control bits.
pub struct Gpu {
    renderer: Renderer,
    pub commands: u64,
    pub revision: u64,
    /// Completed scanout frames, independent of host slices or texture uploads.
    pub frames: u64,
    completed_frame: Option<FrameReadback>,
    pub irq: bool,
    packet: Vec<u32>,
    packet_words: usize,
    polyline: Option<Polyline>,
    upload: Option<Transfer>,
    download: Option<Transfer>,
    draw_min: (i32, i32),
    draw_max: (i32, i32),
    offset: (i32, i32),
    mask_set: bool,
    mask_check: bool,
    display_start: (usize, usize),
    display_mode: u32,
    display_disabled: bool,
    draw_mode: u32,
    texture_window: u32,
    dma_direction: u32,
    read_latch: u32,
    horizontal_range: (u32, u32),
    vertical_range: (u16, u16),
    video_fraction: u64,
    line_clock: u32,
    scanline: u16,
    field_odd: bool,
    dot_divider: u32,
    next_dot: u32,
    blank_sync_pending: bool,
}

impl Default for Gpu {
    fn default() -> Self {
        Self {
            renderer: Renderer::default(),
            commands: 0,
            revision: 0,
            frames: 0,
            completed_frame: None,
            irq: false,
            packet: Vec::with_capacity(12),
            packet_words: 0,
            polyline: None,
            upload: None,
            download: None,
            draw_min: (0, 0),
            draw_max: (0, 0),
            offset: (0, 0),
            mask_set: false,
            mask_check: false,
            display_start: (0, 0),
            display_mode: 0,
            display_disabled: true,
            draw_mode: 0,
            texture_window: 0,
            dma_direction: 0,
            read_latch: 0,
            horizontal_range: (512, 3072),
            vertical_range: (16, 256),
            video_fraction: 0,
            line_clock: 0,
            scanline: 0,
            field_odd: false,
            dot_divider: 10,
            next_dot: 10,
            blank_sync_pending: true,
        }
    }
}

impl Gpu {
    pub fn status(&self) -> u32 {
        let dma_request = match self.dma_direction {
            1 | 2 => true,
            3 => self.download.is_some(),
            _ => false,
        };
        (self.draw_mode & 0x7ff)
            | (u32::from(self.mask_set) << 11)
            | (u32::from(self.mask_check) << 12)
            | (u32::from(self.display_mode & 0x20 == 0 || self.field_odd) << 13)
            | ((self.display_mode & 0x40) << 10)
            | ((self.display_mode & 3) << 17)
            | ((self.display_mode & 0x3c) << 17)
            | (u32::from(self.display_disabled) << 23)
            | (u32::from(self.irq) << 24)
            | (u32::from(dma_request) << 25)
            | (1 << 26)
            | (u32::from(self.download.is_some()) << 27)
            | (1 << 28)
            | (self.dma_direction << 29)
            | (u32::from(
                !self.vblank()
                    && if self.display_mode & 0x24 == 0x24 {
                        self.field_odd
                    } else {
                        self.scanline & 1 != 0
                    },
            ) << 31)
    }

    fn hblank(&self) -> bool {
        self.line_clock < self.horizontal_range.0 || self.line_clock >= self.horizontal_range.1
    }

    fn vblank(&self) -> bool {
        self.scanline < self.vertical_range.0 || self.scanline >= self.vertical_range.1
    }

    /// Advance scanout and the root counters, returning I_STAT interrupt edges.
    /// The interpreter supplies approximate CPU clocks. Integer scanlines model
    /// non-interlaced NTSC/PAL; interlace half-lines and GPU busy timing remain TODO.
    /// Reference: https://psx-spx.consoledev.net/graphicsprocessingunitgpu/#gpu-timings
    #[allow(clippy::similar_names)] // Hardware names distinguish horizontal and vertical blank.
    #[inline]
    pub fn tick(&mut self, clocks: u32, timers: &mut Timers) -> u32 {
        let pal = self.display_mode & 8 != 0;
        let line_clocks = if pal { 3406 } else { 3413 };
        let lines = if pal { 314 } else { 263 };
        let mut interrupts = 0;
        if self.blank_sync_pending {
            timers.set_blank(0, self.hblank());
            timers.set_blank(1, self.vblank());
            self.blank_sync_pending = false;
        }
        for _ in 0..clocks {
            interrupts |= timers.tick(1);
            // 53.693175 / 33.868800 MHz (NTSC), 53.203425 / 33.868800 (PAL).
            // Each CPU clock produces one or two GPU clocks. Keep the same
            // fixed-point phase without a division in the instruction hot path.
            self.video_fraction += if pal { 709_379 } else { 715_909 };
            let video_clocks = 1 + u32::from(self.video_fraction >= 2 * 451_584);
            self.video_fraction -= u64::from(video_clocks) * 451_584;
            let end = self.line_clock + video_clocks;
            if end < line_clocks
                && ![self.horizontal_range.0, self.horizontal_range.1]
                    .into_iter()
                    .any(|edge| self.line_clock < edge && end >= edge)
            {
                // At most two GPU clocks per CPU clock: no blank edge and at
                // most one dot pulse. Avoid per-dot divisions and blank updates.
                self.line_clock = end;
                if self.line_clock >= self.next_dot {
                    self.next_dot += self.dot_divider;
                    interrupts |= timers.external_clock(0, 1);
                }
                continue;
            }
            for _ in 0..video_clocks {
                let old_hblank = self.hblank();
                let old_vblank = self.vblank();
                self.line_clock += 1;
                if self.line_clock >= line_clocks {
                    self.line_clock = 0;
                    self.next_dot = self.dot_divider;
                    self.scanline += 1;
                    if self.scanline >= lines {
                        self.scanline = 0;
                        self.field_odd = !self.field_odd;
                        self.frames += 1;
                    }
                } else if self.line_clock == self.next_dot {
                    self.next_dot += self.dot_divider;
                    interrupts |= timers.external_clock(0, 1);
                }
                let hblank = self.hblank();
                let vblank = self.vblank();
                timers.set_blank(0, hblank);
                timers.set_blank(1, vblank);
                if hblank && !old_hblank {
                    interrupts |= timers.external_clock(1, 1);
                }
                if vblank && !old_vblank {
                    // Capture before exposing IRQ0 to the CPU. Host slices can
                    // otherwise end between a buffer clear and its draw calls.
                    self.latch_display();
                    interrupts |= 1;
                }
            }
        }
        interrupts
    }

    pub fn nominal_fps(&self) -> f64 {
        if self.display_mode & 8 != 0 {
            53_203_425.0 / (3406.0 * 314.0)
        } else {
            53_693_175.0 / (3413.0 * 263.0)
        }
    }

    #[cfg(test)]
    #[allow(clippy::similar_names)] // Preserve the original scanout algorithm as the timing oracle.
    fn tick_reference(&mut self, clocks: u32, timers: &mut Timers) -> u32 {
        let pal = self.display_mode & 8 != 0;
        let line_clocks = if pal { 3406 } else { 3413 };
        let lines = if pal { 314 } else { 263 };
        let divider = if self.display_mode & 0x40 != 0 {
            7
        } else {
            [10, 8, 5, 4][(self.display_mode & 3) as usize]
        };
        let mut interrupts = 0;
        timers.set_blank(0, self.hblank());
        timers.set_blank(1, self.vblank());
        for _ in 0..clocks {
            interrupts |= timers.tick(1);
            // 53.693175 / 33.868800 MHz (NTSC), 53.203425 / 33.868800 (PAL).
            self.video_fraction += if pal { 709_379 } else { 715_909 };
            let video_clocks = self.video_fraction / 451_584;
            self.video_fraction %= 451_584;
            for _ in 0..video_clocks {
                let old_hblank = self.hblank();
                let old_vblank = self.vblank();
                self.line_clock += 1;
                if self.line_clock >= line_clocks {
                    self.line_clock = 0;
                    self.scanline += 1;
                    if self.scanline >= lines {
                        self.scanline = 0;
                        self.field_odd = !self.field_odd;
                        self.frames += 1;
                    }
                } else if self.line_clock.is_multiple_of(divider) {
                    interrupts |= timers.external_clock(0, 1);
                }
                let hblank = self.hblank();
                let vblank = self.vblank();
                timers.set_blank(0, hblank);
                timers.set_blank(1, vblank);
                if hblank && !old_hblank {
                    interrupts |= timers.external_clock(1, 1);
                }
                if vblank && !old_vblank {
                    interrupts |= 1;
                }
            }
        }
        interrupts
    }

    pub fn dimensions(&self) -> (usize, usize) {
        let width = if self.display_mode & 0x40 != 0 {
            368
        } else {
            [256, 320, 512, 640][(self.display_mode & 3) as usize]
        };
        let height = if self.display_mode & 0x24 == 0x24 {
            480
        } else {
            240
        };
        (width, height)
    }

    pub fn display_rgb(&self) -> Vec<u8> {
        self.renderer.display(self.display_settings())
    }

    fn display_settings(&self) -> DisplaySettings {
        DisplaySettings {
            dimensions: self.dimensions(),
            start: self.display_start,
            disabled: self.display_disabled,
            rgb24: self.display_mode & 0x10 != 0,
        }
    }

    /// Debug readback, synchronized with all prior rendering commands.
    pub fn vram(&self) -> Vec<u16> {
        self.renderer.snapshot().vram
    }

    /// Direct debug editing switches to serial rendering until explicitly enabled again.
    pub fn vram_mut(&mut self) -> &mut [u16] {
        self.renderer.vram_mut()
    }

    pub fn renderer_mode(&self) -> RendererMode {
        self.renderer.mode()
    }
    pub fn renderer_device(&self) -> String {
        self.renderer.device_name()
    }
    pub fn renderer_error(&self) -> Option<String> {
        self.renderer.error()
    }
    pub fn set_renderer(&mut self, mode: RendererMode) -> std::io::Result<()> {
        self.renderer.set_mode(mode)
    }
    pub fn synchronize_renderer(&self) {
        self.renderer.synchronize();
    }

    /// Submit a short DMA batch now so the GPU overlaps the following CPU work.
    /// This does not wait for completion or alter emulated DMA/IRQ timing.
    pub(crate) fn dispatch_pending(&mut self) {
        self.renderer.dispatch_pending();
    }

    /// Last completed field. Live VRAM remains available through `display_rgb`
    /// for debugger steps and explicit framebuffer dumps.
    pub fn completed_frame(&self) -> Option<&DisplayFrame> {
        self.completed_frame.as_ref().map(FrameReadback::get)
    }

    /// Capture current VRAM for a manual debugger step without advancing video time.
    /// Running frontends should use the field latched by normal GPU timing instead.
    pub fn latch_display(&mut self) {
        self.completed_frame = Some(self.renderer.capture(self.display_settings()));
    }

    pub fn gp1(&mut self, word: u32) -> Result<(), EmuError> {
        match word >> 24 {
            0 => {
                let renderer = std::mem::take(&mut self.renderer);
                let revision = self.revision + 1;
                let frames = self.frames;
                let completed_frame = self.completed_frame.take();
                *self = Self::default();
                self.renderer = renderer;
                self.revision = revision;
                self.frames = frames;
                // A GP1 reset changes registers, not pixels already scanned out.
                self.completed_frame = completed_frame;
            }
            1 => {
                self.packet.clear();
                self.polyline = None;
                self.upload = None;
                self.download = None;
            }
            2 => self.irq = false,
            3 => {
                self.display_disabled = word & 1 != 0;
                self.revision += 1;
            }
            4 => self.dma_direction = word & 3,
            5 => {
                self.display_start = ((word & 1023) as usize, ((word >> 10) & 511) as usize);
                self.revision += 1;
            }
            6 => {
                self.horizontal_range = (word & 4095, (word >> 12) & 4095);
                self.blank_sync_pending = true;
            }
            7 => {
                self.vertical_range = ((word & 1023) as u16, ((word >> 10) & 1023) as u16);
                self.blank_sync_pending = true;
            }
            8 => {
                self.display_mode = word & 0x7f;
                self.dot_divider = if word & 0x40 != 0 {
                    7
                } else {
                    [10, 8, 5, 4][(word & 3) as usize]
                };
                self.next_dot = (self.line_clock / self.dot_divider + 1) * self.dot_divider;
                self.revision += 1;
            }
            0x10..=0x1f => {
                self.read_latch = match word & 15 {
                    2 => self.texture_window,
                    3 => self.draw_min.0 as u32 | ((self.draw_min.1 as u32) << 10),
                    4 => self.draw_max.0 as u32 | ((self.draw_max.1 as u32) << 10),
                    5 => (self.offset.0 as u32 & 2047) | ((self.offset.1 as u32 & 2047) << 11),
                    7 => 2,
                    _ => self.read_latch,
                };
            }
            _ => return Err(EmuError::GpuCommand((word >> 24) as u8)),
        }
        Ok(())
    }

    pub fn read(&mut self) -> u32 {
        let Some(mut transfer) = self.download.take() else {
            return self.read_latch;
        };
        let [low, high] = self.renderer.read([transfer.next(), transfer.next()]);
        if transfer.index < transfer.width * transfer.height {
            self.download = Some(transfer);
        }
        self.read_latch = low | (high << 16);
        self.read_latch
    }

    pub fn gp0(&mut self, word: u32) -> Result<(), EmuError> {
        if let Some(mut transfer) = self.upload.take() {
            for color in [word as u16, (word >> 16) as u16] {
                if let Some(index) = transfer.next() {
                    self.renderer
                        .submit(RasterCommand::Pixel(index, color, self.draw_state()));
                }
            }
            if transfer.index < transfer.width * transfer.height {
                self.upload = Some(transfer);
            }
            self.revision += 1;
            return Ok(());
        }
        if let Some(mut line) = self.polyline.take() {
            // The terminator is valid only at the first word of a new vertex,
            // and only after the two vertices consumed by the initial packet.
            if line.next_color.is_none() && word & 0xf000_f000 == 0x5000_5000 {
                return Ok(());
            }
            if line.shaded && line.next_color.is_none() {
                line.next_color = Some(color_channels(word));
            } else {
                let next = Vertex {
                    position: self.position(word),
                    color: line.next_color.take().unwrap_or(line.previous.color),
                    uv: [0; 2],
                };
                self.line([line.previous, next], line.style);
                line.previous = next;
                self.revision += 1;
            }
            self.polyline = Some(line);
            return Ok(());
        }
        if self.packet.is_empty() {
            self.packet_words = match word >> 24 {
                0x00 | 0x01 | 0x04..=0x1f | 0xe0..=0xef => 1,
                0x02 | 0xa0 | 0xc0 => 3,
                0x80 => 4,
                opcode @ 0x20..=0x3f => {
                    let count = if opcode & 8 != 0 { 4 } else { 3 };
                    1 + count * (1 + usize::from(opcode & 4 != 0))
                        + if opcode & 16 != 0 { count - 1 } else { 0 }
                }
                opcode @ 0x40..=0x5f => 3 + usize::from(opcode & 16 != 0),
                opcode @ 0x60..=0x7f => {
                    2 + usize::from(opcode & 4 != 0) + usize::from(opcode & 0x18 == 0)
                }
                _ => return Err(EmuError::GpuCommand((word >> 24) as u8)),
            };
        }
        self.packet.push(word);
        if self.packet.len() == self.packet_words {
            // Small fixed packet copy avoids allocations on every draw command.
            let mut packet = [0; 12];
            packet[..self.packet.len()].copy_from_slice(&self.packet);
            self.packet.clear();
            self.execute(&packet);
            self.commands += 1;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Keep the small GP0 command dispatcher together.
    fn execute(&mut self, p: &[u32; 12]) {
        match p[0] >> 24 {
            0x02 | 0x60..=0x7f | 0x80 => {
                self.renderer
                    .submit(RasterCommand::Packet(*p, self.draw_state()));
                self.revision += 1;
            }
            0x1f => self.irq = true,
            0x20..=0x3f => {
                if p[0] & (1 << 26) != 0 {
                    let stride = 2 + usize::from(p[0] & (1 << 28) != 0);
                    self.draw_mode = (self.draw_mode & !0x1ff) | ((p[2 + stride] >> 16) & 0x1ff);
                }
                self.renderer
                    .submit(RasterCommand::Packet(*p, self.draw_state()));
                self.revision += 1;
            }
            0x40..=0x5f => {
                let shaded = p[0] & (1 << 28) != 0;
                let first = Vertex {
                    position: self.position(p[1]),
                    color: color_channels(p[0]),
                    uv: [0; 2],
                };
                let last = Vertex {
                    position: self.position(p[2 + usize::from(shaded)]),
                    color: color_channels(p[if shaded { 2 } else { 0 }]),
                    uv: [0; 2],
                };
                let style = DrawStyle {
                    texture: None,
                    transparent: p[0] & (1 << 25) != 0,
                    // Unlike flat polygons, even flat lines use dithering.
                    dither: self.draw_mode & 0x200 != 0,
                };
                self.line([first, last], style);
                if p[0] & (1 << 27) != 0 {
                    self.polyline = Some(Polyline {
                        previous: last,
                        next_color: None,
                        shaded,
                        style,
                    });
                }
                self.revision += 1;
            }
            0xa0 => self.upload = Some(Transfer::new(p[1], p[2])),
            0xc0 => self.download = Some(Transfer::new(p[1], p[2])),
            0xe1 => self.draw_mode = p[0] & 0x3fff,
            0xe2 => self.texture_window = p[0] & 0xfffff,
            0xe3 => self.draw_min = ((p[0] & 1023) as i32, ((p[0] >> 10) & 511) as i32),
            0xe4 => self.draw_max = ((p[0] & 1023) as i32, ((p[0] >> 10) & 511) as i32),
            0xe5 => self.offset = (sign11(p[0]), sign11(p[0] >> 11)),
            0xe6 => {
                self.mask_set = p[0] & 1 != 0;
                self.mask_check = p[0] & 2 != 0;
            }
            _ => {}
        }
    }

    fn position(&self, word: u32) -> (i32, i32) {
        (
            sign11(word) + self.offset.0,
            sign11(word >> 16) + self.offset.1,
        )
    }

    fn line(&mut self, vertices: [Vertex; 2], style: DrawStyle) {
        self.renderer
            .submit(RasterCommand::Line(vertices, style, self.draw_state()));
    }
    fn draw_state(&self) -> DrawState {
        DrawState {
            draw_min: self.draw_min,
            draw_max: self.draw_max,
            offset: self.offset,
            mask_set: self.mask_set,
            mask_check: self.mask_check,
            draw_mode: self.draw_mode,
            texture_window: self.texture_window,
        }
    }
}

fn color_channels(word: u32) -> [i64; 3] {
    std::array::from_fn(|n| i64::from((word >> (8 * n)) & 255))
}

fn sign11(value: u32) -> i32 {
    ((value << 21) as i32) >> 21
}
fn color15(value: u32) -> u16 {
    (((value >> 3) & 31) | ((value >> 6) & 0x3e0) | ((value >> 9) & 0x7c00)) as u16
}
#[cfg(test)]
mod timing_regression {
    use super::*;

    #[test]
    fn optimized_scanout_matches_original_clock_by_clock_reference() {
        let mut fast = Gpu::default();
        let mut reference = fast.clone();
        let mut fast_timers = Timers::default();
        let mut reference_timers = fast_timers.clone();
        let mut random = 0x6d2b_79f5_u32;
        for iteration in 0..20_000 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            if iteration % 127 == 0 {
                let word = 0x0800_0000 | (random & 0x7f);
                fast.gp1(word).unwrap();
                reference.gp1(word).unwrap();
                let register = 0x1f80_1104 + ((random >> 8) % 3) * 16;
                fast_timers.write(register, (random >> 12) as u16);
                reference_timers.write(register, (random >> 12) as u16);
            }
            if iteration % 191 == 0 {
                let word = 0x0600_0000 | (random & 0x00ff_ffff);
                fast.gp1(word).unwrap();
                reference.gp1(word).unwrap();
            }
            let clocks = 1 + random % 137;
            assert_eq!(
                fast.tick(clocks, &mut fast_timers),
                reference.tick_reference(clocks, &mut reference_timers)
            );
            assert_eq!(fast.status(), reference.status());
            assert_eq!(fast.frames, reference.frames);
            assert_eq!(fast_timers, reference_timers);
        }
    }
}
