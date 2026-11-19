//! Explicit shared bindings keep raster, feedback and scanout pipelines compatible.
pub(super) struct Pipelines {
    pub bindings: wgpu::BindGroup,
    pub raster: wgpu::ComputePipeline,
    pub ordered: wgpu::ComputePipeline,
    pub ordered_copy: wgpu::ComputePipeline,
    pub scanout: wgpu::ComputePipeline,
    pub seed_enhanced: wgpu::ComputePipeline,
}

pub(super) fn create(device: &wgpu::Device, buffers: &[wgpu::Buffer; 6]) -> Pipelines {
    let entries: [_; 6] = std::array::from_fn(|i| wgpu::BindGroupLayoutEntry {
        binding: i as u32,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: if i == 5 {
                wgpu::BufferBindingType::Uniform
            } else {
                wgpu::BufferBindingType::Storage {
                    read_only: i != 0 && i != 4,
                }
            },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("PS1 raster resources"),
        entries: &entries,
    });
    let entries: [_; 6] = std::array::from_fn(|i| wgpu::BindGroupEntry {
        binding: i as u32,
        resource: buffers[i].as_entire_binding(),
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("PS1 raster resources"),
        layout: &layout,
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("PS1 raster layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Eidoryn integer rasterizer"),
        source: wgpu::ShaderSource::Wgsl(include_str!("raster.wgsl").into()),
    });
    let pipeline = |entry: &'static str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    };
    Pipelines {
        bindings,
        raster: pipeline("raster"),
        ordered: pipeline("ordered"),
        ordered_copy: pipeline("ordered_copy"),
        scanout: pipeline("scanout"),
        seed_enhanced: pipeline("seed_enhanced"),
    }
}
