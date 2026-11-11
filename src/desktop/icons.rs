//! Embedded vector icons from the interface prototype.
use gpui::{AssetSource, SharedString, Styled, svg};
use std::borrow::Cow;

pub(super) struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
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
