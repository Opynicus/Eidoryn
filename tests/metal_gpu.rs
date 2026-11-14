//! Explicit hardware tests: `cargo test --features metal --test metal_gpu -- --ignored --test-threads=1`
//! A missing physical Metal device is a failure, never a skipped software run.
#![cfg(all(feature = "metal", target_os = "macos"))]

const HARDWARE_MODE: eidoryn_psx::gpu::RendererMode = eidoryn_psx::gpu::RendererMode::Metal;

#[path = "support/hardware_gpu.rs"]
mod hardware_gpu;
