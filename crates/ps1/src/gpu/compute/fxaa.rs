//! Optional post-scanout pass. A filter setup error leaves the PS1 renderer usable.
use super::pipelines::encode_pass;

#[derive(Default)]
pub(super) enum Fxaa {
    #[default]
    Cold,
    Ready(Pass),
    Unavailable(Option<String>),
}

pub(super) struct Pass {
    pipeline: wgpu::ComputePipeline,
    dimensions: wgpu::Buffer,
    last_size: [u32; 2],
    target: Target,
}

struct Target {
    output: wgpu::Buffer,
    bindings: wgpu::BindGroup,
}

impl Target {
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::ComputePipeline,
        input: &wgpu::Buffer,
        dimensions: &wgpu::Buffer,
        bytes: u64,
    ) -> Self {
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PS1 FXAA output"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let entries = [input, &output, dimensions];
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PS1 FXAA bindings"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &std::array::from_fn::<_, 3, _>(|i| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: entries[i].as_entire_binding(),
            }),
        });
        Self { output, bindings }
    }
}

// Catch optional resource errors before they can reach the device's fatal error latch.
// Only first use and growth enter these scopes.
fn checked<T>(device: &wgpu::Device, create: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = create();
    let errors = [validation, memory, internal].map(|scope| pollster::block_on(scope.pop()));
    errors.into_iter().flatten().next().map_or(Ok(value), Err)
}

impl Fxaa {
    fn prepare(&mut self, device: &wgpu::Device, input: &wgpu::Buffer, bytes: u64) {
        let allocation = match self {
            Self::Cold => checked(device, || {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("PS1 FXAA Quality 12"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("fxaa.wgsl").into()),
                });
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("PS1 FXAA Quality 12"),
                    layout: None,
                    module: &module,
                    entry_point: Some("fxaa"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });
                let dimensions = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("PS1 FXAA dimensions"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let target = Target::new(device, &pipeline, input, &dimensions, bytes);
                Pass {
                    pipeline,
                    dimensions,
                    last_size: [0; 2],
                    target,
                }
            })
            .map(|pass| *self = Self::Ready(pass)),
            Self::Ready(pass) if pass.target.output.size() < bytes => checked(device, || {
                Target::new(device, &pass.pipeline, input, &pass.dimensions, bytes)
            })
            .map(|target| pass.target = target),
            _ => return,
        };
        if let Err(error) = allocation {
            *self = Self::Unavailable(Some(format!(
                "FXAA unavailable. Showing the unfiltered picture. {error}"
            )));
        }
    }

    pub(super) fn render<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: &'a wgpu::Buffer,
        size: [u32; 2],
    ) -> &'a wgpu::Buffer {
        self.prepare(device, input, u64::from(size[0]) * u64::from(size[1]) * 4);
        let Self::Ready(pass) = self else {
            return input;
        };
        if pass.last_size != size {
            queue.write_buffer(&pass.dimensions, 0, bytemuck::cast_slice(&size));
            pass.last_size = size;
        }
        encode_pass(
            encoder,
            &pass.pipeline,
            &pass.target.bindings,
            size.map(|n| n.div_ceil(8)),
        );
        &pass.target.output
    }

    pub(super) fn take_warning(&mut self) -> Option<String> {
        match self {
            Self::Unavailable(message) => message.take(),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(super) fn target(&self) -> Option<&wgpu::Buffer> {
        match self {
            Self::Ready(pass) => Some(&pass.target.output),
            _ => None,
        }
    }
}
