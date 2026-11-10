//! Reusable host audio and presentation services, independent of any emulated system.
#![allow(clippy::doc_markdown)]

pub mod audio;
#[cfg(feature = "themes")]
pub mod theme;
pub mod video_filter;
