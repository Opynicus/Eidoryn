//! PlayStation application adapter: media, persistent cards, and worker/inspector state.
//! Hardware lives in `eidoryn-ps1`; these services belong to the application.
pub use eidoryn_ps1::{
    EmuError, bus, cdrom, cpu, cpu_engine, demo, disc, gpu, gte, machine, mdec, memory_card, sio,
    spu, timers,
};

pub mod card_file;
pub mod media;
pub mod runner;
