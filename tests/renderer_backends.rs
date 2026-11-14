use eidoryn_psx::gpu::{Gpu, RendererMode};

fn fill(gpu: &mut Gpu, color: u32) {
    for word in [0x0200_0000 | color, 0, 16 | (1 << 16)] {
        gpu.gp0(word).unwrap();
    }
}

#[test]
fn unsupported_backends_preserve_current_software_modes() {
    let unavailable = [
        #[cfg(any(not(feature = "dx12"), not(target_os = "windows")))]
        RendererMode::Dx12,
        #[cfg(not(feature = "vulkan"))]
        RendererMode::Vulkan,
        #[cfg(not(feature = "opengl"))]
        RendererMode::OpenGl,
        #[cfg(any(not(feature = "metal"), not(target_os = "macos")))]
        RendererMode::Metal,
        #[cfg(not(any(
            all(feature = "dx12", target_os = "windows"),
            feature = "vulkan",
            feature = "opengl",
            all(feature = "metal", target_os = "macos")
        )))]
        RendererMode::Hardware,
    ];
    for mode in [RendererMode::Serial, RendererMode::Threaded] {
        let mut gpu = Gpu::default();
        gpu.set_renderer(mode).unwrap();
        for backend in unavailable {
            fill(&mut gpu, 0x0000_00ff);
            let before = gpu.vram();
            let error = gpu.set_renderer(backend).unwrap_err();
            assert!(!error.to_string().is_empty());
            assert_eq!(gpu.renderer_mode(), mode);
            assert_eq!(gpu.vram(), before);
            assert_eq!(gpu.renderer_error(), None);
            fill(&mut gpu, 0x0000_ff00);
            assert_eq!(gpu.vram()[0], 31 << 5);
            assert_eq!(gpu.renderer_error(), None);
        }
    }
}

#[cfg(not(any(
    feature = "dx12",
    feature = "vulkan",
    feature = "opengl",
    feature = "metal"
)))]
#[test]
fn software_only_build_reports_every_hardware_backend_unavailable() {
    let reported = eidoryn_psx::gpu::hardware_backends();
    assert_eq!(reported.len(), 4);
    for (status, mode) in reported.iter().zip([
        RendererMode::Dx12,
        RendererMode::Vulkan,
        RendererMode::OpenGl,
        RendererMode::Metal,
    ]) {
        assert_eq!(status.mode, mode);
        assert_eq!(status.device, None);
        assert!(
            status
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty())
        );
    }
}

#[cfg(any(
    feature = "dx12",
    feature = "vulkan",
    feature = "opengl",
    feature = "metal"
))]
#[test]
#[ignore = "requires at least one physical GPU supported by this build"]
fn detected_hardware_backends_can_actually_draw() {
    let reported = eidoryn_psx::gpu::hardware_backends();
    assert_eq!(reported.len(), 4);
    assert!(reported.iter().any(|status| status.device.is_some()));
    let mut software = Gpu::default();
    fill(&mut software, 0x0000_00ff);
    for status in reported {
        assert!(status.mode.is_hardware());
        assert_ne!(status.mode, RendererMode::Hardware);
        if let Some(device) = status.device {
            assert_eq!(status.reason, None);
            assert!(!device.is_empty());
            let mut gpu = software.clone();
            gpu.set_renderer(status.mode).unwrap();
            assert_eq!(gpu.renderer_mode(), status.mode);
            assert_eq!(gpu.renderer_device(), device);
            assert_eq!(gpu.vram(), software.vram());
            fill(&mut gpu, 0x0000_ff00);
            gpu.synchronize_renderer();
            assert_eq!(gpu.renderer_error(), None);
            assert_eq!(gpu.vram()[0], 31 << 5);
        } else {
            assert!(status.reason.is_some_and(|reason| !reason.is_empty()));
            let mut gpu = software.clone();
            assert!(gpu.set_renderer(status.mode).is_err());
            assert_eq!(gpu.renderer_mode(), RendererMode::Serial);
            assert_eq!(gpu.vram(), software.vram());
            assert_eq!(gpu.renderer_error(), None);
        }
    }
}

#[cfg(all(feature = "dx12", target_os = "windows"))]
#[test]
#[ignore = "requires a physical DirectX 12 GPU on Windows"]
fn windows_automatic_hardware_prefers_directx12() {
    let reported = eidoryn_psx::gpu::hardware_backends();
    let dx12 = reported
        .iter()
        .find(|status| status.mode == RendererMode::Dx12)
        .unwrap();
    assert!(
        dx12.device.is_some(),
        "DirectX 12 unavailable: {:?}",
        dx12.reason
    );
    assert_eq!(dx12.reason, None);
    let mut gpu = Gpu::default();
    fill(&mut gpu, 0x0000_00ff);
    let before = gpu.vram();
    gpu.set_renderer(RendererMode::Hardware).unwrap();
    assert_eq!(gpu.renderer_mode(), RendererMode::Hardware);
    assert!(gpu.renderer_device().contains(RendererMode::Dx12.label()));
    assert_eq!(gpu.vram(), before);
    fill(&mut gpu, 0x0000_ff00);
    gpu.synchronize_renderer();
    assert_eq!(gpu.renderer_error(), None);
    assert_eq!(gpu.vram()[0], 31 << 5);
    gpu.set_renderer(RendererMode::Serial).unwrap();
    assert_eq!(gpu.renderer_mode(), RendererMode::Serial);
    assert_eq!(gpu.vram()[0], 31 << 5);
}
