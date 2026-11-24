mod compositor;
#[cfg(feature = "gpu")]
mod hardware;
#[cfg(test)]
mod tests;
use crate::{HEIGHT, WIDTH};
use compositor::{Effects, OBJ, Pixel, Stack, window_masks};

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Video {
    #[serde(with = "serde_bytes")]
    pub palette: Box<[u8]>,
    #[serde(with = "serde_bytes")]
    pub vram: Box<[u8]>,
    #[serde(with = "serde_bytes")]
    pub oam: Box<[u8]>,
    #[serde(skip)]
    pub pixels: Box<[u8]>,
    #[cfg(feature = "gpu")]
    #[serde(skip)]
    accelerator: Option<hardware::Compositor>,
    #[serde(skip)]
    pub graphics_error: Option<String>,
    affine_origin: [[i32; 2]; 2],
}

pub(super) fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn rgb888(color: u16) -> [u8; 3] {
    [0, 5, 10].map(|shift| {
        let channel = ((color >> shift) & 31u16) as u8;
        (channel << 3) | (channel >> 2)
    })
}

fn fixed_reference(io: &[u8], offset: usize) -> i32 {
    let word = i32::from(half(io, offset)) | (i32::from(half(io, offset + 2)) << 16);
    // The affine reference registers hold signed 28-bit, 20.8 fixed-point values.
    (word << 4) >> 4
}

fn coefficient(bytes: &[u8], offset: usize) -> i32 {
    i32::from(half(bytes, offset) as i16)
}

fn mosaic_size(enabled: bool, register: u8) -> (usize, usize) {
    let size = if enabled { register } else { 0 };
    (usize::from(size & 15) + 1, usize::from(size >> 4) + 1)
}

fn background_line(
    line: &mut [Stack; WIDTH],
    masks: &[u8; WIDTH],
    width: usize,
    rank: u8,
    layer: u8,
    sample: impl Fn(usize) -> Option<u16>,
) {
    for (group, pixels) in line.chunks_mut(width).enumerate() {
        let x = group * width;
        if let Some(color) = sample(x) {
            let pixel = Pixel {
                color,
                rank,
                layer,
                semi: false,
            };
            for (stack, mask) in pixels.iter_mut().zip(&masks[x..]) {
                if mask & layer != 0 {
                    stack.insert(pixel);
                }
            }
        }
    }
}

impl Video {
    /// Materialize scanlines still held by the GPU when saving mid-field in the debugger.
    pub(super) fn append_snapshot_pixels(&self, bytes: &mut Vec<u8>) {
        #[cfg(feature = "gpu")]
        let start = bytes.len();
        bytes.extend_from_slice(&self.pixels);
        #[cfg(feature = "gpu")]
        if let Some(accelerator) = &self.accelerator {
            accelerator.finish_in_software(&mut bytes[start..]);
        }
    }

    pub fn new() -> Self {
        Self {
            palette: vec![0; 1024].into_boxed_slice(),
            vram: vec![0; 96 * 1024].into_boxed_slice(),
            oam: vec![0; 1024].into_boxed_slice(),
            pixels: vec![0; WIDTH * HEIGHT * 3].into_boxed_slice(),
            #[cfg(feature = "gpu")]
            accelerator: None,
            graphics_error: None,
            affine_origin: [[0; 2]; 2],
        }
    }

    pub fn set_hardware_acceleration(&mut self, enabled: bool) -> Result<(), String> {
        #[cfg(feature = "gpu")]
        {
            if enabled && self.accelerator.is_none() {
                self.accelerator = Some(hardware::Compositor::open()?);
            } else if !enabled && let Some(accelerator) = self.accelerator.take() {
                accelerator.finish_in_software(&mut self.pixels);
            }
        }
        #[cfg(not(feature = "gpu"))]
        if enabled {
            return Err("GBA hardware rendering is unavailable in this build".into());
        }
        self.graphics_error = None;
        Ok(())
    }

    #[cfg_attr(
        not(feature = "gpu"),
        expect(clippy::unused_self, reason = "the compositor is compiled out")
    )]
    pub fn graphics_device(&self) -> Option<&str> {
        #[cfg(feature = "gpu")]
        return self.accelerator.as_ref().map(|a| a.name.as_str());
        #[cfg(not(feature = "gpu"))]
        None
    }

    #[cfg(feature = "gpu")]
    pub fn retain_renderer(&mut self, previous: &mut Self) {
        self.accelerator = previous.accelerator.take();
        if let Some(accelerator) = &mut self.accelerator {
            accelerator.discard_partial_frame();
        }
    }

    pub fn reload_reference(&mut self, io: &[u8], offset: usize) {
        let bg = (offset - 0x28) / 16;
        let axis = ((offset - 0x28) & 4) / 4;
        self.affine_origin[bg][axis] = fixed_reference(io, offset & !3);
    }

    pub fn advance_reference(&mut self, io: &[u8]) {
        for (bg, origin) in self.affine_origin.iter_mut().enumerate() {
            let base = 0x20 + bg * 16;
            origin[0] += coefficient(io, base + 2);
            origin[1] += coefficient(io, base + 6);
        }
    }

    pub fn render_line(&mut self, y: usize, io: &[u8]) {
        let control = half(io, 0);
        if control & 0x80 != 0 {
            #[cfg(feature = "gpu")]
            if self.accelerator.is_some()
                && self.capture(
                    y,
                    &Effects::default(),
                    &[Stack::backdrop(0x7fff); WIDTH],
                    &[0; WIDTH],
                )
            {
                return;
            }
            let line_offset = y * WIDTH * 3;
            self.pixels[line_offset..line_offset + WIDTH * 3].fill(255);
            return;
        }

        let mode = control & 7;
        let mut objects = [Pixel::EMPTY; WIDTH];
        let mut obj_window = [false; WIDTH];
        if control & 0x1000 != 0 {
            self.objects(y, control, io[0x4d], &mut objects, &mut obj_window);
        }
        let masks = window_masks(control, y, io, &obj_window);
        let mut line = [Stack::backdrop(half(&self.palette, 0)); WIDTH];
        for ((stack, mask), object) in line.iter_mut().zip(masks).zip(objects) {
            if object.layer & mask != 0 {
                stack.insert(object);
            }
        }
        for bg in 0..4 {
            if control & (0x100 << bg) == 0 {
                continue;
            }
            let settings = half(io, 8 + bg * 2);
            let rank = ((settings & 3) as u8) * 8 + 1 + bg as u8;
            let layer = 1 << bg;
            let (width, height) = mosaic_size(settings & 0x40 != 0, io[0x4c]);
            let sample_y = y - y % height;
            if mode == 0 || (mode == 1 && bg < 2) {
                background_line(&mut line, &masks, width, rank, layer, |x| {
                    self.text_index(bg, x, sample_y, settings, io)
                        .map(|index| half(&self.palette, index * 2))
                });
            } else if matches!((mode, bg), (1 | 3..=5, 2) | (2, 2 | 3)) {
                let affine = 0x20 + (bg - 2) * 16;
                let [sx, sy] = self.affine_origin[bg - 2];
                let missed_lines = (y - sample_y) as i32;
                let sx = sx - missed_lines * coefficient(io, affine + 2);
                let sy = sy - missed_lines * coefficient(io, affine + 6);
                let dx = coefficient(io, affine);
                let dy = coefficient(io, affine + 4);
                background_line(&mut line, &masks, width, rank, layer, |x| {
                    self.affine_color(
                        mode,
                        control,
                        settings,
                        (sx + x as i32 * dx) >> 8,
                        (sy + x as i32 * dy) >> 8,
                    )
                });
            }
        }
        let effects = Effects::read(io);
        #[cfg(feature = "gpu")]
        if self.capture(y, &effects, &line, &masks) {
            return;
        }
        let line_offset = y * WIDTH * 3;
        for ((stack, mask), rgb) in line
            .into_iter()
            .zip(masks)
            .zip(self.pixels[line_offset..].as_chunks_mut::<3>().0)
        {
            *rgb = rgb888(stack.color(&effects, mask));
        }
    }

    #[cfg(feature = "gpu")]
    fn capture(
        &mut self,
        y: usize,
        effects: &Effects,
        line: &[Stack; WIDTH],
        masks: &[u8; WIDTH],
    ) -> bool {
        let Some(accelerator) = &mut self.accelerator else {
            return false;
        };
        if !accelerator.capture(y, effects, line, masks) {
            return false;
        }
        if y == HEIGHT - 1
            && let Err(error) = accelerator.finish(&mut self.pixels)
        {
            accelerator.finish_in_software(&mut self.pixels);
            self.graphics_error = Some(error);
            self.accelerator = None;
        }
        true
    }

    fn text_index(&self, bg: usize, x: usize, y: usize, settings: u16, io: &[u8]) -> Option<usize> {
        let large_x = settings & 0x4000 != 0;
        let large_y = settings & 0x8000 != 0;
        let x = (x + half(io, 0x10 + bg * 4) as usize) & if large_x { 511 } else { 255 };
        let y = (y + half(io, 0x12 + bg * 4) as usize) & if large_y { 511 } else { 255 };
        let block = (x / 256) + (y / 256) * (1 + usize::from(large_x));
        let map = usize::from((settings >> 8) & 31) * 2048 + block * 2048;
        let entry = half(
            &self.vram,
            (map + 2 * (((y / 8) & 31) * 32 + ((x / 8) & 31))) & 0xfffe,
        );
        let tx = (x & 7) ^ if entry & 0x400 != 0 { 7 } else { 0 };
        let ty = (y & 7) ^ if entry & 0x800 != 0 { 7 } else { 0 };
        let chars = usize::from((settings >> 2) & 3) * 0x4000;
        let tile = usize::from(entry & 1023);
        let dot = ty * 8 + tx;
        let (index, bank) = if settings & 0x80 != 0 {
            (self.vram[(chars + tile * 64 + dot) & 0xffff], 0)
        } else {
            let byte = self.vram[(chars + tile * 32 + dot / 2) & 0xffff];
            ((byte >> ((tx & 1) * 4)) & 15, usize::from(entry >> 12) * 16)
        };
        (index != 0).then_some(usize::from(index) + bank)
    }

    fn affine_color(&self, mode: u16, control: u16, settings: u16, x: i32, y: i32) -> Option<u16> {
        if mode < 3 {
            let size = 128 << (settings >> 14);
            if settings & 0x2000 == 0 && (!(0..size).contains(&x) || !(0..size).contains(&y)) {
                return None;
            }
            let x = (x & (size - 1)) as usize;
            let y = (y & (size - 1)) as usize;
            let map = usize::from((settings >> 8) & 31) * 2048;
            let tile = usize::from(self.vram[(map + y / 8 * (size as usize / 8) + x / 8) & 0xffff]);
            let chars = usize::from((settings >> 2) & 3) * 0x4000;
            let index = self.vram[(chars + tile * 64 + (y & 7) * 8 + (x & 7)) & 0xffff];
            return (index != 0).then(|| half(&self.palette, usize::from(index) * 2));
        }
        let (width, height) = if mode == 5 { (160, 128) } else { (240, 160) };
        if !(0..width).contains(&x) || !(0..height).contains(&y) {
            return None;
        }
        let page = if mode != 3 && control & 0x10 != 0 {
            0xa000
        } else {
            0
        };
        let index = (y * width + x) as usize;
        Some(if mode == 4 {
            let entry = self.vram[page + index];
            if entry == 0 {
                return None;
            }
            half(&self.palette, usize::from(entry) * 2)
        } else {
            half(&self.vram, page + index * 2)
        })
    }

    fn objects(
        &self,
        y: usize,
        control: u16,
        mosaic: u8,
        line: &mut [Pixel; WIDTH],
        obj_window: &mut [bool; WIDTH],
    ) {
        const SIZES: [[(i32, i32); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        // The lowest OAM index wins between objects. Its BG priority then
        // determines whether the surviving object pixel covers the background.
        for obj in self.oam.as_chunks::<8>().0 {
            let attr0 = half(obj, 0);
            let attr1 = half(obj, 2);
            let attr2 = half(obj, 4);
            let affine = attr0 & 0x100 != 0;
            let double = attr0 & 0x200 != 0;
            let mode = (attr0 >> 10) & 3;
            if (!affine && double) || mode == 3 || (mode == 2 && control & 0x8000 == 0) {
                continue;
            }
            let Some(sizes) = SIZES.get(usize::from(attr0 >> 14)) else {
                continue;
            };
            let (w, h) = sizes[usize::from(attr1 >> 14)];
            let scale = if double { 2 } else { 1 };
            let dy = (y as i32 - i32::from(attr0 & 255)) & 255;
            if dy >= h * scale {
                continue;
            }
            let (block_width, block_height) = mosaic_size(attr0 & 0x1000 != 0, mosaic);
            let dy = (dy - (y % block_height) as i32).max(0);
            let priority = (((attr2 >> 10) & 3) * 8) as u8;
            let depth8 = attr0 & 0x2000 != 0;
            let units = if depth8 { 2 } else { 1 };
            let base = usize::from(attr2 & if depth8 { 1022 } else { 1023 });
            if (control & 7) >= 3 && base < 512 {
                continue;
            }
            let stride = if control & 0x40 != 0 {
                w as usize / 8 * units
            } else {
                32
            };
            // Treat the 9-bit X coordinate as signed to include wrapped sprites at the left edge.
            let left = (i32::from(attr1 & 511) << 23) >> 23;
            let right = left + w * scale;
            let block_width = block_width as i32;
            let end = right + (-right).rem_euclid(block_width);
            let matrix = usize::from((attr1 >> 9) & 31) * 32 + 6;
            for screen_x in left.max(0)..end.min(WIDTH as i32) {
                let dx = (screen_x - screen_x % block_width - left).max(0);
                let screen_x = screen_x as usize;
                let pixel = &mut line[screen_x];
                if mode != 2 && pixel.layer != 0 {
                    continue;
                }
                let (tx, ty) = if affine {
                    let cx = dx - w * scale / 2;
                    let cy = dy - h * scale / 2;
                    (
                        ((coefficient(&self.oam, matrix) * cx
                            + coefficient(&self.oam, matrix + 8) * cy)
                            >> 8)
                            + w / 2,
                        ((coefficient(&self.oam, matrix + 16) * cx
                            + coefficient(&self.oam, matrix + 24) * cy)
                            >> 8)
                            + h / 2,
                    )
                } else {
                    (
                        if attr1 & 0x1000 != 0 { w - 1 - dx } else { dx },
                        if attr1 & 0x2000 != 0 { h - 1 - dy } else { dy },
                    )
                };
                if !(0..w).contains(&tx) || !(0..h).contains(&ty) {
                    continue;
                }
                let (tx, ty) = (tx as usize, ty as usize);
                let tile = (base + ty / 8 * stride + tx / 8 * units) & 1023;
                let dot = (ty & 7) * 8 + (tx & 7);
                let byte = self.vram[0x10000 + ((tile * 32 + dot * units / 2) & 0x7fff)];
                let (index, bank) = if depth8 {
                    (byte, 0)
                } else {
                    ((byte >> ((tx & 1) * 4)) & 15, usize::from(attr2 >> 12) * 16)
                };
                if index != 0 {
                    if mode == 2 {
                        obj_window[screen_x] = true;
                    } else {
                        *pixel = Pixel {
                            color: half(&self.palette, 0x200 + (usize::from(index) + bank) * 2),
                            rank: priority,
                            layer: OBJ,
                            semi: mode == 1,
                        };
                    }
                }
            }
        }
    }
}
