//! Component paints, their Midnight values, and their sources in other palettes.
use super::Palette;
use std::collections::BTreeMap;

macro_rules! component_colors {
    (|$p:ident| $( $role:ident: $midnight:literal => $fallback:expr, )+) => {
        /// Resolved component paints for a desktop theme, in 24-bit RGB.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct ComponentColors { $( pub $role: u32, )+ }

        impl ComponentColors {
            pub(super) const MIDNIGHT: Self = Self { $( $role: $midnight, )+ };

            fn inherited($p: &Palette) -> Self {
                Self { $( $role: $fallback, )+ }
            }
        }

        impl Palette {
            $(
                pub const fn $role(&self) -> u32 {
                    let $p = self;
                    match &self.components {
                        Some(colors) => colors.$role,
                        None => $fallback,
                    }
                }
            )+

            pub(in crate::theme) fn apply_component_overrides(
                &mut self,
                base: &Self,
                mut overrides: BTreeMap<String, u32>,
            ) -> Result<(), String> {
                if base.components.is_none() && overrides.is_empty() {
                    return Ok(());
                }
                let old = ComponentColors::inherited(base);
                let new = ComponentColors::inherited(self);
                // Old plugins customize the base colors only. Follow those edits while
                // retaining the base theme's component paints everywhere else.
                let colors = ComponentColors {
                    $( $role: overrides.remove(stringify!($role)).unwrap_or_else(|| {
                        if old.$role == new.$role { base.$role() } else { new.$role }
                    }), )+
                };
                if let Some((unknown, _)) = overrides.first_key_value() {
                    return Err(format!("Unknown theme color: {unknown}"));
                }
                self.components = Some(colors);
                Ok(())
            }
        }
    };
}

component_colors! { |p|
    sidebar: 0x15191e => sidebar_blend(p.background, p.surface),
    soft_text: 0x7f918d => p.muted,
    brand_border: 0xa7d7bc => p.primary,
    resize_handle: 0x758a80 => p.muted,
    button_surface: 0x283138 => p.surface_variant,
    ghost_hover: 0x293138 => p.tonal_hover,
    running_indicator: 0xa8d9ba => p.on_success,
    paused_indicator: 0xe6bf79 => p.muted,
    danger_text: 0xffc6ba => p.on_error,
    success_border: 0x405f4d => p.on_success,
    error_border: 0x795048 => p.on_error,
    inset_surface: 0x151b1d => p.background,
    inset_border: 0x34443d => p.border,
    stage_border: 0x3a4645 => p.border,
    standby_surface: 0x11181b => p.background,
    standby_title: 0xc4d7ce => p.primary,
    standby_caption: 0x88a59a => p.muted,
    standby_hint: 0x92a59e => p.muted,
    history_surface: 0x1a2025 => p.surface_variant,
    history_selected: 0x23302a => p.tonal,
    history_selected_border: 0x91c8a5 => p.primary,
    instruction_highlight: 0x30463b => p.tonal,
    on_instruction_highlight: 0xd5f7df => p.on_tonal,
    drawer_surface: 0x1b2227 => p.surface,
    drawer_border: 0x42504d => p.border,
    settings_group: 0x1c2529 => p.surface_variant,
    dialog_surface: 0x20292d => p.surface,
    dialog_border: 0x4c5d56 => p.border,
    input_surface: 0x11191d => p.surface,
    input_border: 0x5b6f65 => p.border,
    rewind_track: 0x35413c => p.border,
    switch_off: 0x46524f => p.border,
    switch_on: 0xaad9b9 => p.primary,
    switch_thumb_off: 0x17201b => p.text,
    switch_thumb_on: 0x17201b => p.on_primary,
}

// Non-Midnight themes use the original sidebar blend (45% surface over background).
const fn sidebar_blend(background: u32, surface: u32) -> u32 {
    const fn channel(background: u32, surface: u32, shift: u32) -> u32 {
        let from = (background >> shift) & 0xff;
        let to = (surface >> shift) & 0xff;
        ((55 * from + 45 * to + 50) / 100) << shift
    }
    channel(background, surface, 16)
        | channel(background, surface, 8)
        | channel(background, surface, 0)
}
