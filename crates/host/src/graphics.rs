//! Adapter identification shared by hardware renderers.

pub fn physical_adapter(info: &wgpu_types::AdapterInfo) -> bool {
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
        wgpu_types::DeviceType::DiscreteGpu | wgpu_types::DeviceType::IntegratedGpu
    ) || (info.backend == wgpu_types::Backend::Gl
        && info.device_type == wgpu_types::DeviceType::Other)
    // OpenGL cannot query PCI device type. wgpu reports physical NVIDIA/AMD
    // adapters as Other; a zero vendor ID can also describe an AMD WGL driver.
}
