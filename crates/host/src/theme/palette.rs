//! Semantic colors shared by every desktop component.
#![allow(clippy::unreadable_literal)]

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
}

impl Palette {
    pub const LAVENDER: Self = Self {
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
        background: 0x141218,
        surface: 0x211f26,
        surface_variant: 0x2b2930,
        border: 0x49454f,
        text: 0xe6e0e9,
        muted: 0xcac4d0,
        primary: 0xd0bcff,
        on_primary: 0x381e72,
        primary_hover: 0xddcdff,
        primary_pressed: 0xbba5ed,
        tonal: 0x4a4458,
        on_tonal: 0xe8def8,
        tonal_hover: 0x5b536a,
        tonal_pressed: 0x625977,
        error: 0x601410,
        on_error: 0xffdad6,
        success: 0x163c30,
        on_success: 0xb7eed5,
        video_background: 0x08070b,
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

    pub(super) fn set(&mut self, key: &str, value: u32) -> Result<(), String> {
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
            _ => return Err(format!("Unknown theme color: {key}")),
        };
        *color = value;
        Ok(())
    }
}
