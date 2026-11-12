//! Worker-owned rendering engines. Device failures are returned to the worker,
//! which latches the error for the emulation loop and drains pending replies.
use super::{
    DisplaySettings, RendererMode,
    rasterizer::{RasterCommand, Rasterizer},
};
use std::{
    io,
    sync::{Arc, OnceLock},
};

pub(super) enum Engine {
    Software(Rasterizer),
    #[cfg(feature = "vulkan")]
    Vulkan(Box<super::vulkan::VulkanRenderer>),
}

impl Engine {
    pub(super) fn new(mode: RendererMode, error: &Arc<OnceLock<String>>) -> io::Result<Self> {
        if mode == RendererMode::Vulkan {
            #[cfg(feature = "vulkan")]
            {
                return super::vulkan::VulkanRenderer::new(Arc::clone(error))
                    .map(|gpu| Self::Vulkan(Box::new(gpu)));
            }
            #[cfg(not(feature = "vulkan"))]
            {
                let _ = error;
                return Err(io::Error::other(
                    "Vulkan support is not compiled in; build with --features vulkan",
                ));
            }
        }
        Ok(Self::Software(Rasterizer::empty()))
    }

    pub(super) fn name(&self) -> String {
        match self {
            Self::Software(_) => "CPU software rasterizer".into(),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.name.clone(),
        }
    }

    pub(super) fn start(&mut self, raster: Rasterizer) -> Result<(), String> {
        match self {
            Self::Software(current) => *current = raster,
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => {
                gpu.start(raster)?;
                gpu.synchronize()?;
            }
        }
        Ok(())
    }

    pub(super) fn execute(&mut self, command: RasterCommand) -> Result<(), String> {
        match self {
            Self::Software(raster) => {
                raster.execute(command);
                Ok(())
            }
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.execute(command),
        }
    }

    pub(super) fn read(&mut self, indices: [Option<usize>; 2]) -> Result<[u32; 2], String> {
        match self {
            Self::Software(raster) => {
                Ok(indices.map(|i| i.map_or(0, |i| u32::from(raster.vram[i]))))
            }
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.read(indices),
        }
    }

    pub(super) fn display(&mut self, display: DisplaySettings) -> Result<Vec<u8>, String> {
        match self {
            Self::Software(raster) => Ok(raster.display_rgb(display)),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.display(display),
        }
    }

    pub(super) fn snapshot(&mut self) -> Result<Rasterizer, String> {
        match self {
            Self::Software(raster) => Ok(raster.clone()),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.snapshot(),
        }
    }

    pub(super) fn synchronize(&mut self) -> Result<(), String> {
        match self {
            Self::Software(_) => Ok(()),
            #[cfg(feature = "vulkan")]
            Self::Vulkan(gpu) => gpu.synchronize(),
        }
    }
}
