//! Semantic colors shared by every desktop component.
#![allow(clippy::unreadable_literal)]

mod components;
pub use components::ComponentColors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub surface_variant: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub primary: u32,
    pub on_primary: u32,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub tonal: u32,
    pub on_tonal: u32,
    pub tonal_hover: u32,
    pub tonal_pressed: u32,
    pub error: u32,
    pub on_error: u32,
    pub success: u32,
    pub on_success: u32,
    pub video_background: u32,
    /// Optional component colors. Themes without overrides derive them from the base colors.
    pub components: Option<ComponentColors>,
}

impl Palette {
    pub const LAVENDER: Self = Self {
        components: None,
        background: 0xf7f5fb,
        surface: 0xfffbff,
        surface_variant: 0xf0edf5,
        border: 0xe7e0ec,
        text: 0x1d1b20,
        muted: 0x625b71,
        primary: 0x6750a4,
        on_primary: 0xffffff,
        primary_hover: 0x7962b4,
        primary_pressed: 0x533c8d,
        tonal: 0xeaddff,
        on_tonal: 0x21005d,
        tonal_hover: 0xdecdf7,
        tonal_pressed: 0xcfb9ed,
        error: 0xf9dedc,
        on_error: 0x8c1d18,
        success: 0xcde8df,
        on_success: 0x175345,
        video_background: 0x15131a,
    };

    pub const MIDNIGHT: Self = Self {
        components: Some(ComponentColors::MIDNIGHT),
        background: 0x101317,
        surface: 0x1c2228,
        surface_variant: 0x222a31,
        border: 0x333d45,
        text: 0xeef2ef,
        muted: 0xaab6b3,
        primary: 0xb7e4c8,
        on_primary: 0x12251a,
        primary_hover: 0xd2f3dc,
        primary_pressed: 0x9cccaf,
        tonal: 0x29332f,
        on_tonal: 0xc5e9d0,
        tonal_hover: 0x354149,
        tonal_pressed: 0x3d4b50,
        error: 0x3c2929,
        on_error: 0xffd4c9,
        success: 0x22372d,
        on_success: 0xd2f4dd,
        video_background: 0x11181b,
    };

    /// Charcoal surfaces and the lemon-green accent from the game library prototype.
    pub const LEMON_GREEN: Self = {
        let mut palette = Self {
            background: 0x101114,
            surface: 0x1a1c21,
            surface_variant: 0x212329,
            border: 0x303238,
            text: 0xf4f4f2,
            muted: 0xaaaeb7,
            primary: 0xd3f572,
            on_primary: 0x182008,
            primary_hover: 0xe0ff94,
            primary_pressed: 0xb9d95c,
            tonal: 0x30352a,
            on_tonal: 0xd3f572,
            tonal_hover: 0x34373d,
            tonal_pressed: 0x3b3d44,
            error: 0x352525,
            on_error: 0xf0a49e,
            success: 0x1b1f1b,
            on_success: 0xd3f572,
            video_background: 0x090a0c,
            components: None,
        };
        palette.components = Some(ComponentColors {
            sidebar: 0x141519,
            soft_text: 0x787e89,
            button_surface: 0x25272d,
            ghost_hover: 0x282b30,
            paused_indicator: 0xe7b974,
            warning_text: 0xdfbf83,
            warning_border: 0x61523b,
            info_text: 0xaabfdb,
            info_border: 0x4a586b,
            danger_text: 0xffc3be,
            success_border: 0x333b2d,
            error_border: 0x79504f,
            history_selected: 0x22251f,
            dialog_surface: 0x202227,
            dialog_border: 0x4a4d52,
            input_surface: 0x1b1d22,
            input_border: 0x3a3d43,
            rewind_track: 0x384037,
            switch_off: 0x555a60,
            switch_thumb_off: 0xf5f5f0,
            switch_thumb_on: 0x263118,
            ..ComponentColors::inherited(&palette)
        });
        palette
    };

    pub const OCEAN: Self = Self {
        background: 0xf1f7fc,
        surface: 0xfafdff,
        surface_variant: 0xe8f0f7,
        border: 0xd3e1eb,
        text: 0x17232c,
        muted: 0x4e6474,
        primary: 0x006493,
        on_primary: 0xffffff,
        primary_hover: 0x167ba8,
        primary_pressed: 0x004c70,
        tonal: 0xcce5ff,
        on_tonal: 0x00344f,
        tonal_hover: 0xb8d9f6,
        tonal_pressed: 0xa3caec,
        ..Self::LAVENDER
    };

    pub const FOREST: Self = Self {
        components: None,
        background: 0x101b16,
        surface: 0x1a2921,
        surface_variant: 0x26372c,
        border: 0x3f5546,
        text: 0xe0ece1,
        muted: 0xb4c9b8,
        primary: 0x9ad5a8,
        on_primary: 0x07391b,
        primary_hover: 0xb4e6c0,
        primary_pressed: 0x80bf90,
        tonal: 0x334e3b,
        on_tonal: 0xd2ebd6,
        tonal_hover: 0x43614c,
        tonal_pressed: 0x527358,
        ..Self::MIDNIGHT
    };

    pub(super) fn set_base(&mut self, key: &str, value: u32) -> bool {
        let color = match key {
            "background" => &mut self.background,
            "surface" => &mut self.surface,
            "surface_variant" => &mut self.surface_variant,
            "border" => &mut self.border,
            "text" => &mut self.text,
            "muted" => &mut self.muted,
            "primary" => &mut self.primary,
            "on_primary" => &mut self.on_primary,
            "primary_hover" => &mut self.primary_hover,
            "primary_pressed" => &mut self.primary_pressed,
            "tonal" => &mut self.tonal,
            "on_tonal" => &mut self.on_tonal,
            "tonal_hover" => &mut self.tonal_hover,
            "tonal_pressed" => &mut self.tonal_pressed,
            "error" => &mut self.error,
            "on_error" => &mut self.on_error,
            "success" => &mut self.success,
            "on_success" => &mut self.on_success,
            "video_background" => &mut self.video_background,
            _ => return false,
        };
        *color = value;
        true
    }
}
