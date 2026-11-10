//! Vulkan compute owns VRAM and performs PS1 rasterization, not just scanout.
//! Tile command lists preserve draw order at each pixel. Cross-pixel reads
//! require a batch barrier; feedback uses an ordered compute invocation.
mod commands;
mod pipelines;
use super::{
    DisplaySettings, VRAM_HEIGHT, VRAM_WIDTH,
    rasterizer::{RasterCommand, Rasterizer},
};
use commands::{Primitive, TILE_COUNT, UPLOAD};
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

pub(super) struct VulkanRenderer {
    pub name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: [wgpu::Buffer; 6],
    staging: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    raster: wgpu::ComputePipeline,
    ordered: wgpu::ComputePipeline,
    scanout: wgpu::ComputePipeline,
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

impl VulkanRenderer {
    pub(super) fn new(error: Arc<OnceLock<String>>) -> io::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
        adapters.retain(|a| {
            matches!(
                a.get_info().device_type,
                wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
            )
        });
        adapters.sort_by_key(|a| a.get_info().device_type != wgpu::DeviceType::DiscreteGpu);
        let adapter = adapters.into_iter().next().ok_or_else(|| io::Error::other(
            "No physical Vulkan GPU available (CPU/software adapters are excluded); use --renderer serial"))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Eidoryn PS1 rasterizer"),
            ..Default::default()
        }))
        .map_err(io::Error::other)?;
        let uncaptured = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |failure: wgpu::Error| {
            let _ = uncaptured.set(format!("Vulkan: {failure}"));
        }));
        let lost = Arc::clone(&error);
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                let _ = lost.set(format!("Vulkan device lost: {reason:?}: {message}"));
            }
        });
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sizes = [
            VRAM_BYTES,
            (MAX_COMMANDS * 128) as u64,
            ((TILE_COUNT * 4 + MAX_REFERENCES) * 4) as u64,
            (MAX_UPLOADS * 4) as u64,
            VRAM_BYTES,
            32,
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
            size: VRAM_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (bindings, raster, ordered, scanout) = pipelines::create(&device, &buffers);
        if let Some(failure) = pollster::block_on(validation.pop()) {
            return Err(io::Error::other(failure));
        }
        if let Some(failure) = error.get() {
            return Err(io::Error::other(failure.clone()));
        }
        Ok(Self {
            name: format!("{} ({}, Vulkan)", info.name, info.driver),
            device,
            queue,
            buffers,
            staging,
            bindings,
            raster,
            ordered,
            scanout,
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

    pub(super) fn start(&mut self, raster: Rasterizer) {
        let words: Vec<u32> = raster.vram.iter().map(|&v| u32::from(v)).collect();
        self.queue
            .write_buffer(&self.buffers[0], 0, bytemuck::cast_slice(&words));
        self.read_cache = Some(raster.vram);
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
            self.queue
                .write_buffer(&self.buffers[1], 0, bytemuck::cast_slice(&p.0));
            self.dispatch(&self.ordered, 1);
            self.limit_submissions()?;
            return self.check();
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
        self.dispatch(&self.raster, count as u32);
        self.commands.clear();
        self.uploads.clear();
        self.reads.fill(false);
        self.writes.fill(false);
        self.references = 0;
        self.limit_submissions()?;
        self.check()
    }

    fn dispatch(&self, pipeline: &wgpu::ComputePipeline, groups: u32) {
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
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
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
            .map_err(|e| format!("Vulkan wait failed: {e}"))?;
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
                .map_err(|e| format!("Vulkan map callback: {e}"))?
                .map_err(|e| format!("Vulkan readback: {e}"))?;
            let mapped = self.staging.slice(..bytes).get_mapped_range();
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
        let (width, height) = display.dimensions;
        if display.disabled {
            return Ok(vec![0; width * height * 3]);
        }
        let params = [
            width as u32,
            height as u32,
            display.start.0 as u32,
            display.start.1 as u32,
            u32::from(display.disabled),
            u32::from(display.rgb24),
            0,
            0,
        ];
        self.queue
            .write_buffer(&self.buffers[5], 0, bytemuck::cast_slice(&params));
        self.dispatch(&self.scanout, (width * height).div_ceil(64) as u32);
        self.readback(4, (width * height * 4) as u64, |bytes| {
            let mut rgb = Vec::with_capacity(width * height * 3);
            for pixel in bytes.as_chunks::<4>().0 {
                rgb.extend_from_slice(&pixel[..3]);
            }
            rgb
        })
    }
}
