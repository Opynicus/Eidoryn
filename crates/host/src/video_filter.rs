//! Original, static CRT transmission mask for GPU alpha compositing.
//! No emulation state or per-frame image processing is involved.
use std::{f32::consts::PI, str::FromStr};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrtFilter {
    #[default]
    Off,
    Soft,
    Classic,
}

impl CrtFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Soft => "Soft",
            Self::Classic => "Classic",
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Soft,
            Self::Soft => Self::Classic,
            Self::Classic => Self::Off,
        }
    }
}

impl FromStr for CrtFilter {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "off" => Ok(Self::Off),
            "soft" => Ok(Self::Soft),
            "classic" => Ok(Self::Classic),
            _ => Err("--crt expects off, soft, or classic"),
        }
    }
}

/// A single bounded texture, cached until viewport size, DPI, line count or
/// preset changes. Capping either axis at 2048 limits storage to 16 MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayKey {
    width: u32,
    height: u32,
    lines: u16,
    filter: CrtFilter,
}

impl OverlayKey {
    pub fn dimensions(self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn new(width: u32, height: u32, lines: usize, filter: CrtFilter) -> Option<Self> {
        if filter == CrtFilter::Off || width == 0 || height == 0 || lines == 0 {
            return None;
        }
        let largest = u64::from(width.max(height)).max(2048);
        Some(Self {
            width: (u64::from(width) * 2048 / largest).max(1) as u32,
            height: (u64::from(height) * 2048 / largest).max(1) as u32,
            lines: lines.min(1024) as u16,
            filter,
        })
    }

    /// Black BGRA with varying alpha: attenuates the video without lifting its
    /// blacks or tinting it. The grille is monochrome, not RGB subpixel emulation.
    pub fn bgra(self) -> Vec<u8> {
        let (scanlines, grille, vignette) = match self.filter {
            CrtFilter::Off => (0.0, 0.0, 0.0),
            CrtFilter::Soft => (0.13, 0.025, 0.12),
            CrtFilter::Classic => (0.27, 0.06, 0.22),
        };
        let width = self.width as usize;
        let height = self.height as usize;
        let rows_per_line = self.height as f32 / f32::from(self.lines);
        // Fade unresolved scanlines instead of producing alternating Moire bands.
        let resolved = ((rows_per_line - 1.0) / 2.0).clamp(0.0, 1.0);
        let pixel_phase = PI / rows_per_line;
        let coverage = pixel_phase.sin() / pixel_phase;
        let columns: Vec<_> = (0..width)
            .map(|x| {
                let nx = 2.0 * (x as f32 + 0.5) / self.width as f32 - 1.0;
                (nx * nx, if x % 3 == 0 { grille * resolved } else { 0.0 })
            })
            .collect();
        let mut pixels = vec![0; width * height * 4];
        for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
            let ny = 2.0 * (y as f32 + 0.5) / self.height as f32 - 1.0;
            // Average the beam profile over a destination pixel, keeping its
            // phase locked to source lines even at fractional window scales.
            // A quarter-line phase keeps alternating rows distinguishable at
            // exactly 2x scaling instead of sampling two equal beam averages.
            let phase = 2.0 * PI * ((y as f32 + 0.5) / rows_per_line - 0.25);
            let scan = scanlines * resolved * (0.5 + 0.5 * phase.cos() * coverage);
            for (pixel, &(nx2, grid)) in row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
                let radius2 = nx2.midpoint(ny * ny);
                let edge = vignette * radius2 * radius2;
                let transmission = (1.0 - scan) * (1.0 - grid) * (1.0 - edge);
                pixel[3] = ((1.0 - transmission) * 255.0).round() as u8;
            }
        }
        pixels
    }
}
