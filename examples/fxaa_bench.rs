//! Compare unfiltered and FXAA scanout at the same checkpoint, RGB888 depth and input.
#[path = "support/frames.rs"]
mod frames;
#[path = "support/presentation.rs"]
mod presentation;

use eidoryn_psx::gpu::{AntiAliasing, ColorDepth};
use frames::Capture;

fn main() -> Result<(), String> {
    presentation::compare(
        [
            (ColorDepth::TrueColor, AntiAliasing::Off, "off"),
            (ColorDepth::TrueColor, AntiAliasing::Fxaa, "fxaa"),
        ],
        Capture {
            warmup: 32,
            final_fields: 12,
        },
    )
}
