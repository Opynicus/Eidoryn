//! Explicit hardware tests: `cargo test --test vulkan_gpu -- --ignored --test-threads=1`
//! A missing physical Vulkan device is a failure, never a skipped software run.
#![cfg(feature = "vulkan")]

const HARDWARE_MODE: eidoryn_psx::gpu::RendererMode = eidoryn_psx::gpu::RendererMode::Vulkan;

#[path = "support/hardware_gpu.rs"]
mod hardware_gpu;
