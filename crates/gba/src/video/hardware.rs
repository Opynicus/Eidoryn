//! Collect LCD-time layer samples, then run the final GBA color compositor once per field.
use super::{Effects, HEIGHT, Stack, WIDTH, rgb888};
use eidoryn_host::graphics::physical_adapter;
use std::{
    sync::{Arc, OnceLock, mpsc},
    time::Duration,
};
use wgpu::BufferUsages as Usage;

const ROW: usize = WIDTH + 1;
const WAIT: Duration = Duration::from_secs(2);

pub(super) struct Compositor {
    pub name: String,
    samples: Box<[[u32; 2]]>,
    rows: usize,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bindings: wgpu::BindGroup,
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
    error: Arc<OnceLock<String>>,
}

impl Compositor {
    pub fn open() -> Result<Self, String> {
        let adapters = adapters();
        let mut errors = Vec::new();
        for adapter in adapters.iter().filter(|a| physical_adapter(&a.get_info())) {
            match Self::from_adapter(adapter) {
                Ok(compositor) => return Ok(compositor),
                Err(error) => errors.push(format!("{}: {error}", adapter.get_info().name)),
            }
        }
        Err(if errors.is_empty() {
            "No supported hardware adapter for GBA LCD composition".into()
        } else {
            errors.join(", ")
        })
    }

    fn from_adapter(adapter: &wgpu::Adapter) -> Result<Self, String> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("GBA LCD compositor"),
            required_limits: wgpu::Limits {
                max_storage_buffers_per_shader_stage: 2,
                max_compute_workgroup_size_x: 8,
                max_compute_workgroup_size_y: 8,
                max_compute_workgroup_size_z: 1,
                max_compute_invocations_per_workgroup: 64,
                max_compute_workgroups_per_dimension: 20,
                ..wgpu::Limits::downlevel_defaults()
            },
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let error = Arc::<OnceLock<String>>::default();
        let lost = Arc::clone(&error);
        device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                let _ = lost.set(format!("GBA compositor device lost: {message}"));
            }
        });
        let failure = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |err| {
            let _ = failure.set(format!("GBA compositor: {err}"));
        }));

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GBA color effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("GBA LCD compositor"),
            layout: None,
            module: &shader,
            entry_point: Some("compose"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = error.get() {
            return Err(error.clone());
        }

        let buffer = |name, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(name),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let bytes = (WIDTH * HEIGHT * 3) as u64;
        let input = buffer(
            "GBA LCD layers",
            (ROW * HEIGHT * 8) as u64,
            Usage::STORAGE | Usage::COPY_DST,
        );
        let output = buffer("GBA RGB888", bytes, Usage::STORAGE | Usage::COPY_SRC);
        let staging = buffer("GBA LCD readback", bytes, Usage::COPY_DST | Usage::MAP_READ);
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GBA LCD compositor"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        if let Some(error) = error.get() {
            return Err(error.clone());
        }
        Ok(Self {
            name: format!("{} · {}", info.backend, info.name),
            samples: vec![[0; 2]; ROW * HEIGHT].into_boxed_slice(),
            rows: 0,
            device,
            queue,
            pipeline,
            bindings,
            input,
            output,
            staging,
            error,
        })
    }

    pub fn discard_partial_frame(&mut self) {
        self.rows = 0;
    }

    pub fn capture(
        &mut self,
        y: usize,
        effects: &Effects,
        line: &[Stack; WIDTH],
        masks: &[u8; WIDTH],
    ) -> bool {
        // Activation may happen while the debugger is between scanlines.
        // Finish that partial field in software until the next complete field starts.
        if self.rows != y {
            return false;
        }
        let row = &mut self.samples[y * ROW..(y + 1) * ROW];
        row[0] = effects.packed();
        for ((sample, stack), &mask) in row[1..].iter_mut().zip(line).zip(masks) {
            *sample = stack.packed(mask);
        }
        self.rows += 1;
        true
    }

    pub fn finish_in_software(&self, pixels: &mut [u8]) {
        for (row, rgb) in self
            .samples
            .as_chunks::<ROW>()
            .0
            .iter()
            .take(self.rows)
            .zip(pixels.as_chunks_mut::<{ WIDTH * 3 }>().0)
        {
            let effects = Effects::unpack(row[0]);
            for (&sample, dst) in row[1..].iter().zip(rgb.as_chunks_mut::<3>().0) {
                let (stack, window) = Stack::unpack(sample);
                *dst = rgb888(stack.color(&effects, window));
            }
        }
    }

    pub fn finish(&mut self, pixels: &mut [u8]) -> Result<(), String> {
        if let Some(error) = self.error.get() {
            return Err(error.clone());
        }
        self.queue
            .write_buffer(&self.input, 0, bytemuck::cast_slice(&self.samples));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bindings, &[]);
            pass.dispatch_workgroups(
                (WIDTH as u32 / 4).div_ceil(8),
                (HEIGHT as u32).div_ceil(8),
                1,
            );
        }
        encoder.copy_buffer_to_buffer(&self.output, 0, &self.staging, 0, self.staging.size());
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        let slice = self.staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(WAIT),
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(WAIT)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if let Some(error) = self.error.get() {
            return Err(error.clone());
        }
        pixels.copy_from_slice(&slice.get_mapped_range().map_err(|e| e.to_string())?);
        self.staging.unmap();
        self.rows = 0;
        Ok(())
    }
}

fn adapters() -> Vec<wgpu::Adapter> {
    let mut backends = wgpu::Backends::empty();
    backends.set(wgpu::Backends::VULKAN, cfg!(feature = "vulkan"));
    backends.set(wgpu::Backends::GL, cfg!(feature = "opengl"));
    backends.set(wgpu::Backends::DX12, cfg!(feature = "dx12"));
    backends.set(wgpu::Backends::METAL, cfg!(feature = "metal"));
    let backends = backends & wgpu::Instance::enabled_backend_features();
    if backends.is_empty() {
        return Vec::new();
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let mut adapters = pollster::block_on(instance.enumerate_adapters(backends));
    adapters.retain(|a| {
        a.get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
    });
    adapters.sort_by_key(|a| {
        let info = a.get_info();
        (
            info.device_type != wgpu::DeviceType::DiscreteGpu,
            info.backend == wgpu::Backend::Gl,
        )
    });
    adapters
}

#[cfg(test)]
mod tests;
