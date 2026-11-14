//! Explicit hardware tests: `cargo test --features opengl --test opengl_gpu -- --ignored --test-threads=1`
//! A missing physical OpenGL device is a failure, never a skipped software run.
#![cfg(feature = "opengl")]

const HARDWARE_MODE: eidoryn_psx::gpu::RendererMode = eidoryn_psx::gpu::RendererMode::OpenGl;

#[path = "support/hardware_gpu.rs"]
mod hardware_gpu;
