//! wgpu compute owns VRAM and performs PS1 rasterization, not just scanout.
//! Tile command lists preserve draw order at each pixel. Cross-pixel reads
//! require a batch barrier; feedback uses an ordered compute invocation.
mod commands;
mod pipelines;
#[cfg(test)]
mod tests;
use super::{
    ColorDepth, DisplaySettings, DisplaySource, RenderScale, RendererMode, VRAM_HEIGHT, VRAM_WIDTH,
    rasterizer::{RasterCommand, Rasterizer},
    renderer_engine::{ErrorLatch, select_candidates},
};
use commands::{COPY, Primitive, TILE_COUNT, UPLOAD};
use std::{
    io,
    sync::{Arc, OnceLock, mpsc},
    time::Duration,
};

const MAX_COMMANDS: usize = 2048;
const MAX_REFERENCES: usize = 262_144;
const MAX_UPLOADS: usize = 65_536;
const VRAM_BYTES: u64 = (VRAM_WIDTH * VRAM_HEIGHT * 4) as u64;
const WAIT: Duration = Duration::from_secs(10);

// Native words always occupy the first plane. When present, the second
// plane stores a contiguous block of S*S encoded samples for each word.
const fn has_samples(scale: RenderScale, depth: ColorDepth) -> bool {
    !matches!((scale, depth), (RenderScale::Native, ColorDepth::Native))
}

fn raster_bytes(scale: RenderScale, depth: ColorDepth) -> u64 {
    let samples = if has_samples(scale, depth) {
        (scale.factor() as u64).pow(2)
    } else {
        0
    };
    VRAM_BYTES * (1 + samples)
}

fn required_limits(scale: RenderScale, depth: ColorDepth) -> wgpu::Limits {
    // Request only the storage/compute resources used by the shared shaders.
    // WebGPU defaults demand eight storage buffers; this rasterizer needs five.
    wgpu::Limits {
        max_storage_buffers_per_shader_stage: 5,
        max_storage_buffer_binding_size: raster_bytes(scale, depth),
        max_compute_invocations_per_workgroup: 64,
        max_compute_workgroup_size_x: 64,
        max_compute_workgroup_size_y: 8,
        max_compute_workgroup_size_z: 1,
        max_compute_workgroups_per_dimension: TILE_COUNT as u32,
        ..wgpu::Limits::downlevel_defaults()
    }
}

fn physical_adapter(info: &wgpu::AdapterInfo) -> bool {
    let description = format!("{} {} {}", info.name, info.driver, info.driver_info).to_lowercase();
    if [
        "software",
        "llvmpipe",
        "lavapipe",
        "softpipe",
        "swiftshader",
        "mesa offscreen",
        "gdi generic",
        "microsoft basic",
    ]
    .iter()
    .any(|name| description.contains(name))
    {
        return false;
    }
    matches!(
        info.device_type,
        wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
    ) || (info.backend == wgpu::Backend::Gl && info.device_type == wgpu::DeviceType::Other)
    // OpenGL cannot query PCI device type. wgpu reports physical NVIDIA/AMD
    // adapters as Other; a zero vendor ID can also describe an AMD WGL driver.
}

pub(super) struct ComputeRenderer {
    pub name: String,
    backend: &'static str,
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: [wgpu::Buffer; 6],
    staging: wgpu::Buffer,
    pipelines: pipelines::Pipelines,
    scale: RenderScale,
    depth: ColorDepth,
    error: Arc<OnceLock<String>>,
    decoded: Vec<Primitive>,
    commands: Vec<[u32; 32]>,
    uploads: Vec<u32>,
    bins: Vec<Vec<u32>>,
    packed_bins: Vec<u32>,
    references: usize,
    reads: Box<[bool; TILE_COUNT]>,
    writes: Box<[bool; TILE_COUNT]>,
    scratch_writes: Vec<usize>,
    scratch_reads: Vec<usize>,
    read_cache: Option<Vec<u16>>,
    submissions: usize,
}

impl ComputeRenderer {
    pub(super) fn new(
        mode: RendererMode,
        scale: RenderScale,
        depth: ColorDepth,
    ) -> io::Result<(Self, ErrorLatch)> {
        let backends = match mode {
            RendererMode::Dx12 => wgpu::Backends::DX12,
            RendererMode::Vulkan => wgpu::Backends::VULKAN,
            RendererMode::OpenGl => wgpu::Backends::GL,
            RendererMode::Metal => wgpu::Backends::METAL,
            _ => return Err(io::Error::other("A hardware renderer must be selected")),
        };
        let backend = mode.label();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let mut adapters = pollster::block_on(instance.enumerate_adapters(backends));
        adapters.retain(|a| physical_adapter(&a.get_info()));
        adapters.sort_by_key(|a| a.get_info().device_type != wgpu::DeviceType::DiscreteGpu);
        let limits = required_limits(scale, depth);
        let requirement = if mode == RendererMode::OpenGl {
            " (OpenGL 4.3+ on Windows)"
        } else {
            ""
        };
        adapters.retain(|adapter| {
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
                && limits.check_limits(&adapter.limits())
        });
        if adapters.is_empty() {
            return Err(io::Error::other(format!(
                "No physical {backend} GPU meets the {}x rasterizer requirements{requirement} (compute shaders, five storage buffers and a {} MiB storage binding)",
                scale.factor(),
                raster_bytes(scale, depth) / (1024 * 1024)
            )));
        }
        select_candidates(
            adapters
                .into_iter()
                .map(|adapter| (adapter.get_info().name, adapter)),
            |adapter, error| {
                let mut gpu =
                    Self::from_adapter(&adapter, backend, Arc::clone(error), scale, depth)?;
                // Complete resource allocation before an API/device is selected.
                // A failure here can still fall back to another physical adapter.
                gpu.synchronize().map_err(io::Error::other)?;
                Ok(gpu)
            },
        )
    }

    fn from_adapter(
        adapter: &wgpu::Adapter,
        backend: &'static str,
        error: ErrorLatch,
        scale: RenderScale,
        depth: ColorDepth,
    ) -> io::Result<Self> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Eidoryn PS1 rasterizer"),
            required_limits: required_limits(scale, depth),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(io::Error::other)?;
        let uncaptured = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |failure: wgpu::Error| {
            let _ = uncaptured.set(format!("{backend}: {failure}"));
        }));
        let lost = Arc::clone(&error);
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                let _ = lost.set(format!("{backend} device lost: {reason:?}: {message}"));
            }
        });
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        // The CRTC supports at most 640x480. Reuse the staging buffer for
        // either a guest VRAM snapshot or a presented RGB scanout.
        let scanout_bytes = 640 * 480 * 4 * (scale.factor() as u64).pow(2);
        let sizes = [
            raster_bytes(scale, depth),
            (MAX_COMMANDS * 128) as u64,
            ((TILE_COUNT * 4 + MAX_REFERENCES) * 4) as u64,
            (MAX_UPLOADS * 4) as u64,
            scanout_bytes,
            48,
        ];
        let buffers = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(
                    [
                        "VRAM",
                        "primitives",
                        "tile lists",
                        "uploads",
                        "scanout",
                        "display registers",
                    ][i],
                ),
                size: sizes[i],
                usage: if i == 5 {
                    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST
                } else {
                    wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC
                },
                mapped_at_creation: false,
            })
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("VRAM/scanout readback"),
            size: VRAM_BYTES.max(scanout_bytes),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipelines = pipelines::create(&device, &buffers);
        if let Some(failure) = pollster::block_on(validation.pop()) {
            return Err(io::Error::other(failure));
        }
        if let Some(failure) = error.get() {
            return Err(io::Error::other(failure.clone()));
        }
        Ok(Self {
            name: if info.driver.is_empty() {
                format!("{} ({backend})", info.name)
            } else {
                format!("{} ({}, {backend})", info.name, info.driver)
            },
            backend,
            device,
            queue,
            buffers,
            staging,
            pipelines,
            scale,
            depth,
            error,
            decoded: Vec::with_capacity(4),
            commands: Vec::with_capacity(MAX_COMMANDS),
            uploads: Vec::with_capacity(MAX_UPLOADS),
            bins: (0..TILE_COUNT).map(|_| Vec::new()).collect(),
            packed_bins: Vec::with_capacity(TILE_COUNT * 4 + MAX_REFERENCES),
            references: 0,
            reads: Box::new([false; TILE_COUNT]),
            writes: Box::new([false; TILE_COUNT]),
            scratch_writes: Vec::new(),
            scratch_reads: Vec::new(),
            read_cache: None,
            submissions: 0,
        })
    }

    pub(super) fn start(&mut self, raster: Rasterizer) -> Result<(), String> {
        self.queue.write_buffer(
            &self.buffers[5],
            24,
            bytemuck::cast_slice(&[
                self.scale.factor() as u32,
                1,
                u32::from(self.depth == ColorDepth::TrueColor),
            ]),
        );
        // Expand directly into staging memory: a temporary Vec<u32> would
        // duplicate this entire 2 MiB upload on every renderer switch.
        {
            let mut upload = self
                .queue
                .write_buffer_with(
                    &self.buffers[0],
                    0,
                    wgpu::BufferSize::new(VRAM_BYTES).expect("VRAM is not empty"),
                )
                .ok_or_else(|| format!("{} VRAM upload staging allocation failed", self.backend))?;
            upload
                .slice(..)
                .write_iter(raster.vram.iter().flat_map(|&v| u32::from(v).to_ne_bytes()));
        } // Schedule the upload before Engine::start synchronizes the queue.
        if has_samples(self.scale, self.depth) {
            self.dispatch(&self.pipelines.seed_enhanced, [128, 64]);
        }
        self.read_cache = Some(raster.vram);
        self.check()
    }

    pub(super) fn execute(&mut self, command: RasterCommand) -> Result<(), String> {
        self.check()?;
        self.read_cache = None;
        self.decoded.clear();
        commands::decode(command, &mut self.decoded);
        for i in 0..self.decoded.len() {
            self.append(self.decoded[i])?;
        }
        Ok(())
    }

    fn append(&mut self, mut p: Primitive) -> Result<(), String> {
        if self.commands.len() == MAX_COMMANDS || self.uploads.len() == MAX_UPLOADS {
            self.flush()?;
        }
        self.scratch_writes.clear();
        self.scratch_reads.clear();
        p.write_tiles(|i| self.scratch_writes.push(i));
        p.read_tiles(|i| self.scratch_reads.push(i));
        if self.references + self.scratch_writes.len() > MAX_REFERENCES
            || self.scratch_reads.iter().any(|&i| self.writes[i])
            || self.scratch_writes.iter().any(|&i| self.reads[i])
        {
            self.flush()?;
        }
        // Write bins form a rectangle, so intersection is O(read tiles), not
        // a quadratic search through two potentially full-VRAM tile lists.
        let feedback = self.scratch_reads.iter().any(|&i| {
            let (x, y) = ((i % 128) as u32, (i / 128) as u32);
            x >= p.0[1] / 8 && x <= p.0[3] / 8 && y >= p.0[2] / 8 && y <= p.0[4] / 8
        });
        if feedback {
            self.flush()?;
            return self.dispatch_ordered(p);
        }
        if p.0[0] == UPLOAD {
            let offset = self.uploads.len() as u32;
            self.uploads.push(p.0[9]);
            p.0[25] = offset;
            if let Some(last) = self.commands.last_mut()
                && last[0] == UPLOAD
                && last[7] & 3 == p.0[7] & 3
                && last[2] == p.0[2]
                && last[3] + 1 == p.0[1]
            {
                let old_tile = last[3] / 8;
                last[3] = p.0[1];
                if old_tile != p.0[1] / 8 {
                    let tile = self.scratch_writes[0];
                    self.bins[tile].push((self.commands.len() - 1) as u32);
                    self.writes[tile] = true;
                    self.references += 1;
                }
                return Ok(());
            }
        }
        let index = self.commands.len() as u32;
        self.commands.push(p.0);
        for &tile in &self.scratch_writes {
            self.bins[tile].push(index);
            self.writes[tile] = true;
        }
        for &tile in &self.scratch_reads {
            self.reads[tile] = true;
        }
        self.references += self.scratch_writes.len();
        Ok(())
    }

    fn dispatch_ordered(&mut self, mut p: Primitive) -> Result<(), String> {
        // Word 25 selects the first row for copies or the first native pixel
        // for draws. These chunk sizes match the ordered WGSL entry points.
        const COPY_ROWS: usize = 32;
        const DRAW_PIXELS: usize = 1024;
        let copy = p.0[0] == COPY;
        let (count, stride, groups) = if copy {
            let rows = p.0[31];
            if p.0[27] == p.0[29] {
                // All rows are independent. Each lane handles one complete row.
                (rows, rows as usize, rows.div_ceil(64))
            } else {
                // All columns in a row are independent. One group synchronizes
                // between rows so a later source can see earlier writes.
                (rows, COPY_ROWS, 1)
            }
        } else {
            let pixels = (p.0[3] - p.0[1] + 1) * (p.0[4] - p.0[2] + 1);
            (pixels, DRAW_PIXELS, 1)
        };
        for first in (0..count).step_by(stride) {
            p.0[25] = first;
            self.queue
                .write_buffer(&self.buffers[1], 0, bytemuck::cast_slice(&p.0));
            let pipeline = if copy {
                &self.pipelines.ordered_copy
            } else {
                &self.pipelines.ordered
            };
            self.dispatch(pipeline, [groups, 1]);
            self.limit_submissions()?;
        }
        self.check()
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.commands.is_empty() {
            return self.check();
        }
        let count = self.bins.iter().filter(|bin| !bin.is_empty()).count();
        self.packed_bins.clear();
        self.packed_bins.resize(count * 4, 0);
        let mut header = 0;
        for (tile, bin) in self
            .bins
            .iter_mut()
            .enumerate()
            .filter(|(_, b)| !b.is_empty())
        {
            let offset = self.packed_bins.len() as u32;
            self.packed_bins[header..header + 4].copy_from_slice(&[
                (tile % 128) as u32,
                (tile / 128) as u32,
                offset,
                bin.len() as u32,
            ]);
            self.packed_bins.append(bin);
            header += 4;
        }
        self.queue
            .write_buffer(&self.buffers[1], 0, bytemuck::cast_slice(&self.commands));
        self.queue
            .write_buffer(&self.buffers[2], 0, bytemuck::cast_slice(&self.packed_bins));
        if !self.uploads.is_empty() {
            self.queue
                .write_buffer(&self.buffers[3], 0, bytemuck::cast_slice(&self.uploads));
        }
        self.dispatch(&self.pipelines.raster, [count as u32, 1]);
        self.commands.clear();
        self.uploads.clear();
        self.reads.fill(false);
        self.writes.fill(false);
        self.references = 0;
        self.limit_submissions()?;
        self.check()
    }

    fn dispatch(&self, pipeline: &wgpu::ComputePipeline, groups: [u32; 2]) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("PS1 batch"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("PS1 batch"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.pipelines.bindings, &[]);
            pass.dispatch_workgroups(groups[0], groups[1], 1);
        }
        self.queue.submit([encoder.finish()]);
    }

    fn limit_submissions(&mut self) -> Result<(), String> {
        self.submissions += 1;
        // Bound queued driver resources even for guest programs without VBlank.
        if self.submissions >= 64 {
            self.wait()?;
        }
        Ok(())
    }

    fn check(&self) -> Result<(), String> {
        self.error.get().map_or(Ok(()), |e| Err(e.clone()))
    }

    fn wait(&mut self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(WAIT),
            })
            .map_err(|e| format!("{} wait failed: {e}", self.backend))?;
        self.submissions = 0;
        self.check()
    }

    pub(super) fn synchronize(&mut self) -> Result<(), String> {
        self.flush()?;
        // Submit pending initial VRAM upload too, even without a drawing batch.
        self.queue.submit([]);
        self.wait()
    }

    fn readback<T>(
        &mut self,
        buffer: usize,
        bytes: u64,
        convert: impl FnOnce(&[u8]) -> T,
    ) -> Result<T, String> {
        self.check()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("PS1 readback"),
            });
        encoder.copy_buffer_to_buffer(&self.buffers[buffer], 0, &self.staging, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let (send, receive) = mpsc::sync_channel(1);
        self.staging
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        let result = self.wait().and_then(|()| {
            receive
                .recv_timeout(WAIT)
                .map_err(|e| format!("{} map callback: {e}", self.backend))?
                .map_err(|e| format!("{} readback: {e}", self.backend))?;
            let mapped = self
                .staging
                .slice(..bytes)
                .get_mapped_range()
                .map_err(|e| format!("{} readback: {e}", self.backend))?;
            // Convert while mapped: callers allocate only their final output,
            // rather than copying the complete readback through a Vec<u32>.
            Ok(convert(&mapped))
        });
        self.staging.unmap();
        result
    }

    pub(super) fn snapshot(&mut self) -> Result<Rasterizer, String> {
        self.cache_vram()?;
        Ok(Rasterizer::from_vram(
            self.read_cache
                .as_ref()
                .expect("VRAM readback populated")
                .clone(),
        ))
    }

    fn cache_vram(&mut self) -> Result<(), String> {
        self.flush()?;
        if self.read_cache.is_none() {
            self.read_cache = Some(self.readback(0, VRAM_BYTES, |bytes| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect()
            })?);
        }
        Ok(())
    }

    pub(super) fn read(&mut self, indices: [Option<usize>; 2]) -> Result<[u32; 2], String> {
        self.cache_vram()?;
        let vram = self.read_cache.as_ref().expect("VRAM readback populated");
        Ok(indices.map(|i| i.map_or(0, |i| u32::from(vram[i]))))
    }

    pub(super) fn display(&mut self, display: DisplaySettings) -> Result<Vec<u8>, String> {
        self.flush()?;
        let (width, height) = display.output_dimensions();
        if display.disabled {
            return Ok(vec![0; width * height * 3]);
        }
        let params = [
            display.dimensions.0 as u32,
            display.dimensions.1 as u32,
            display.start.0 as u32,
            display.start.1 as u32,
            u32::from(display.disabled),
            u32::from(display.rgb24),
            self.scale.factor() as u32,
            display.output_scale().factor() as u32,
            u32::from(self.depth == ColorDepth::TrueColor),
            u32::from(display.source == DisplaySource::Presentation),
        ];
        self.queue
            .write_buffer(&self.buffers[5], 0, bytemuck::cast_slice(&params));
        self.dispatch(
            &self.pipelines.scanout,
            [width.div_ceil(8) as u32, height.div_ceil(8) as u32],
        );
        self.readback(4, (width * height * 4) as u64, |bytes| {
            let mut rgb = Vec::with_capacity(width * height * 3);
            for pixel in bytes.as_chunks::<4>().0 {
                rgb.extend_from_slice(&pixel[..3]);
            }
            rgb
        })
    }
}
