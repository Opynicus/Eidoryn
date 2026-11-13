//! Eidoryn application services and backward-compatible PlayStation library entry points.
#![allow(clippy::doc_markdown)]

pub mod app;
pub mod i18n;
#[cfg(feature = "desktop")]
pub mod keybindings;
#[cfg(feature = "desktop")]
pub mod preferences;
pub mod systems;
#[cfg(feature = "desktop")]
pub use eidoryn_host::theme;
pub use eidoryn_host::{audio, video_filter};

// Keep existing clients working while new code uses an explicit system namespace.
pub use systems::ps1::{
    EmuError, bus, card_file, cdrom, cpu, cpu_engine, demo, disc, gpu, gte, machine, mdec, media,
    memory_card, runner, sio, spu, timers, timing,
};
