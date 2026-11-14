//! Explicit hardware tests: `cargo test --features dx12 --test dx12_gpu -- --ignored --test-threads=1`
//! A missing physical DirectX 12 device is a failure, never a skipped software run.
#![cfg(all(feature = "dx12", target_os = "windows"))]

const HARDWARE_MODE: eidoryn_psx::gpu::RendererMode = eidoryn_psx::gpu::RendererMode::Dx12;

#[path = "support/hardware_gpu.rs"]
mod hardware_gpu;
