//! Embedded project artwork and interface icons.
use gpui::{AssetSource, RenderImage, SharedString, Styled, svg};
use std::{
    borrow::Cow,
    sync::{Arc, LazyLock},
};

pub(super) fn app_icon() -> Option<Arc<RenderImage>> {
    static ICON: LazyLock<Option<Arc<RenderImage>>> = LazyLock::new(|| {
        let artwork = image::load_from_memory_with_format(
            include_bytes!("../../assets/eidoryn.png"),
            image::ImageFormat::Png,
        )
        .inspect_err(|error| eprintln!("Cannot decode the Eidoryn icon: {error}"))
        .ok()?;
        // Retain a high-DPI thumbnail of the packaging artwork in GPUI's BGRA format.
        let mut pixels = artwork
            .resize(256, 256, image::imageops::FilterType::Triangle)
            .into_rgba8();
        for pixel in pixels.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
    });
    ICON.clone()
}

pub(super) struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "menu" => include_bytes!("../../assets/icons/menu.svg"),
            "grid" => include_bytes!("../../assets/icons/grid.svg"),
            "screen" => include_bytes!("../../assets/icons/screen.svg"),
            "code" => include_bytes!("../../assets/icons/code.svg"),
            "disc" => include_bytes!("../../assets/icons/disc.svg"),
            "settings" => include_bytes!("../../assets/icons/settings.svg"),
            "play" => include_bytes!("../../assets/icons/play.svg"),
            "pause" => include_bytes!("../../assets/icons/pause.svg"),
            "reset" => include_bytes!("../../assets/icons/reset.svg"),
            "expand" => include_bytes!("../../assets/icons/expand.svg"),
            "back" => include_bytes!("../../assets/icons/back.svg"),
            "volume" => include_bytes!("../../assets/icons/volume.svg"),
            "mute" => include_bytes!("../../assets/icons/mute.svg"),
            "plus" => include_bytes!("../../assets/icons/plus.svg"),
            "info" => include_bytes!("../../assets/icons/info.svg"),
            "close" => include_bytes!("../../assets/icons/close.svg"),
            "skip" => include_bytes!("../../assets/icons/skip.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub(super) fn icon(name: &'static str, color: u32) -> gpui::Svg {
    svg()
        .path(name)
        .size(gpui::px(17.))
        .text_color(gpui::rgb(color))
}
