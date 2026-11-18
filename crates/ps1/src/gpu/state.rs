//! Frozen V1 layout of GPU command, raster and scanout state.
use super::{
    DisplayFrame, DrawStyle, FrameReadback, Gpu, Polyline, Renderer, Texture, Transfer, Vertex,
    rasterizer::RasterizerV1,
};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Vertex")]
struct VertexV1 {
    position: (i32, i32),
    color: [i64; 3],
    uv: [i64; 2],
}

impl From<Vertex> for VertexV1 {
    fn from(value: Vertex) -> Self {
        Self {
            position: value.position,
            color: value.color,
            uv: value.uv,
        }
    }
}

impl From<VertexV1> for Vertex {
    fn from(value: VertexV1) -> Self {
        Self {
            position: value.position,
            color: value.color,
            uv: value.uv,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Texture")]
struct TextureV1 {
    clut: u16,
    raw: bool,
}

impl From<Texture> for TextureV1 {
    fn from(value: Texture) -> Self {
        Self {
            clut: value.clut,
            raw: value.raw,
        }
    }
}

impl From<TextureV1> for Texture {
    fn from(value: TextureV1) -> Self {
        Self {
            clut: value.clut,
            raw: value.raw,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "DrawStyle")]
struct DrawStyleV1 {
    texture: Option<TextureV1>,
    transparent: bool,
    dither: bool,
}

impl From<DrawStyle> for DrawStyleV1 {
    fn from(value: DrawStyle) -> Self {
        Self {
            texture: value.texture.map(Into::into),
            transparent: value.transparent,
            dither: value.dither,
        }
    }
}

impl From<DrawStyleV1> for DrawStyle {
    fn from(value: DrawStyleV1) -> Self {
        Self {
            texture: value.texture.map(Into::into),
            transparent: value.transparent,
            dither: value.dither,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Polyline")]
struct PolylineV1 {
    previous: VertexV1,
    next_color: Option<[i64; 3]>,
    shaded: bool,
    style: DrawStyleV1,
}

impl From<Polyline> for PolylineV1 {
    fn from(value: Polyline) -> Self {
        Self {
            previous: value.previous.into(),
            next_color: value.next_color,
            shaded: value.shaded,
            style: value.style.into(),
        }
    }
}

impl From<PolylineV1> for Polyline {
    fn from(value: PolylineV1) -> Self {
        Self {
            previous: value.previous.into(),
            next_color: value.next_color,
            shaded: value.shaded,
            style: value.style.into(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Transfer")]
struct TransferV1 {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    index: usize,
}

impl From<Transfer> for TransferV1 {
    fn from(value: Transfer) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            index: value.index,
        }
    }
}

impl From<TransferV1> for Transfer {
    fn from(value: TransferV1) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            index: value.index,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "DisplayFrame")]
struct DisplayFrameV1 {
    dimensions: (usize, usize),
    rgb: Arc<[u8]>,
}

impl From<&DisplayFrame> for DisplayFrameV1 {
    fn from(value: &DisplayFrame) -> Self {
        Self {
            dimensions: value.dimensions,
            rgb: Arc::clone(&value.rgb),
        }
    }
}

impl TryFrom<DisplayFrameV1> for DisplayFrame {
    type Error = String;

    fn try_from(value: DisplayFrameV1) -> Result<Self, Self::Error> {
        let (width, height) = value.dimensions;
        if ![256, 320, 368, 512, 640].contains(&width)
            || ![240, 480].contains(&height)
            || value.rgb.len() != width * height * 3
        {
            return Err("Invalid completed display frame".into());
        }
        Ok(Self {
            dimensions: value.dimensions,
            rgb: value.rgb,
        })
    }
}

crate::state::v1::impl_serde_v1!(DisplayFrame, DisplayFrameV1);

#[allow(clippy::struct_excessive_bools)] // V1 stores independent GPU control bits.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename = "Gpu")]
pub(crate) struct GpuV1 {
    renderer: RasterizerV1,
    commands: u64,
    revision: u64,
    frames: u64,
    completed_frame: Option<DisplayFrameV1>,
    irq: bool,
    packet: Vec<u32>,
    packet_words: usize,
    polyline: Option<PolylineV1>,
    upload: Option<TransferV1>,
    download: Option<TransferV1>,
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

impl TryFrom<&Gpu> for GpuV1 {
    type Error = String;

    fn try_from(value: &Gpu) -> Result<Self, Self::Error> {
        let raster = value.renderer.snapshot();
        if let Some(error) = value.renderer.error() {
            return Err(error);
        }
        Ok(Self {
            renderer: raster.into(),
            commands: value.commands,
            revision: value.revision,
            frames: value.frames,
            completed_frame: value.completed_frame().map(Into::into),
            irq: value.irq,
            packet: value.packet.clone(),
            packet_words: value.packet_words,
            polyline: value.polyline.map(Into::into),
            upload: value.upload.map(Into::into),
            download: value.download.map(Into::into),
            draw_min: value.draw_min,
            draw_max: value.draw_max,
            offset: value.offset,
            mask_set: value.mask_set,
            mask_check: value.mask_check,
            display_start: value.display_start,
            display_mode: value.display_mode,
            display_disabled: value.display_disabled,
            draw_mode: value.draw_mode,
            texture_window: value.texture_window,
            dma_direction: value.dma_direction,
            read_latch: value.read_latch,
            horizontal_range: value.horizontal_range,
            vertical_range: value.vertical_range,
            video_fraction: value.video_fraction,
            line_clock: value.line_clock,
            scanline: value.scanline,
            field_odd: value.field_odd,
            dot_divider: value.dot_divider,
            next_dot: value.next_dot,
            blank_sync_pending: value.blank_sync_pending,
        })
    }
}

impl TryFrom<GpuV1> for Gpu {
    type Error = String;

    fn try_from(value: GpuV1) -> Result<Self, Self::Error> {
        let state = Self {
            renderer: Renderer::from_raster(value.renderer.try_into()?),
            commands: value.commands,
            revision: value.revision,
            frames: value.frames,
            completed_frame: value
                .completed_frame
                .map(DisplayFrame::try_from)
                .transpose()?
                .map(FrameReadback::from),
            irq: value.irq,
            packet: value.packet,
            packet_words: value.packet_words,
            polyline: value.polyline.map(Into::into),
            upload: value.upload.map(Into::into),
            download: value.download.map(Into::into),
            draw_min: value.draw_min,
            draw_max: value.draw_max,
            offset: value.offset,
            mask_set: value.mask_set,
            mask_check: value.mask_check,
            display_start: value.display_start,
            display_mode: value.display_mode,
            display_disabled: value.display_disabled,
            draw_mode: value.draw_mode,
            texture_window: value.texture_window,
            dma_direction: value.dma_direction,
            read_latch: value.read_latch,
            horizontal_range: value.horizontal_range,
            vertical_range: value.vertical_range,
            video_fraction: value.video_fraction,
            line_clock: value.line_clock,
            scanline: value.scanline,
            field_odd: value.field_odd,
            dot_divider: value.dot_divider,
            next_dot: value.next_dot,
            blank_sync_pending: value.blank_sync_pending,
        };
        state.validate_state()?;
        Ok(state)
    }
}

crate::state::v1::impl_serde_v1!(Gpu, GpuV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_progress_and_command_reset_survive_restore() {
        let mut gpu = Gpu::default();
        // Upload and download may overlap. Both have one pixel left in an odd-sized region.
        for word in [
            0xc000_0000,
            0,
            0x0001_0003,
            0xa000_0000,
            0,
            0x0001_0003,
            0x1234,
        ] {
            gpu.gp0(word).unwrap();
        }
        gpu.read();
        let mut restored = Gpu::try_from(GpuV1::try_from(&gpu).unwrap()).unwrap();
        assert_eq!(restored.read(), gpu.read());
        for state in [&mut gpu, &mut restored] {
            state.gp0(0x5678).unwrap();
            state.gp0(0xa000_0000).unwrap();
            state.gp1(0x0100_0000).unwrap();
            // GP1 aborts the packet and leaves its old word count latched.
            assert!(state.gp0(0xff00_0000).is_err());
            state.validate_state().unwrap();
        }
        assert_eq!(gpu.vram(), restored.vram());

        for word in [0xc000_0000, 0, 0x0001_0002] {
            gpu.gp0(word).unwrap();
        }
        let mut invalid = GpuV1::try_from(&gpu).unwrap();
        invalid.download.as_mut().unwrap().index = 2;
        assert!(Gpu::try_from(invalid).is_err());
    }

    #[test]
    fn direct_deserialization_checks_partial_packet_and_completed_frame() {
        let mut gpu = Gpu::default();
        gpu.gp0(0xa000_0000).unwrap();
        gpu.packet_words = 4;
        let bytes = bincode::serialize(&gpu).unwrap();
        assert!(bincode::deserialize::<Gpu>(&bytes).is_err());

        let frame = DisplayFrame {
            dimensions: (320, 240),
            rgb: Arc::from([0; 3]),
        };
        let bytes = bincode::serialize(&frame).unwrap();
        assert!(bincode::deserialize::<DisplayFrame>(&bytes).is_err());
    }
}
