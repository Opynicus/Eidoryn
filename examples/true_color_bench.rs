//! Compare presentation precision from a fixed, read-only PS1 checkpoint.
#[path = "support/frames.rs"]
mod frames;
#[path = "support/presentation.rs"]
mod presentation;

use eidoryn_psx::gpu::{AntiAliasing, ColorDepth};
use frames::Capture;

fn main() -> Result<(), String> {
    presentation::compare(
        [
            (ColorDepth::Native, AntiAliasing::Off, "native"),
            (ColorDepth::TrueColor, AntiAliasing::Off, "true-color"),
        ],
        Capture {
            warmup: 0,
            final_fields: 1,
        },
    )
}
