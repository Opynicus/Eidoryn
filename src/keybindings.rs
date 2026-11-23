//! Desktop keyboard bindings for emulated controllers.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadButton {
    Select = 0,
    Start = 3,
    Up = 4,
    Right = 5,
    Down = 6,
    Left = 7,
    L2 = 8,
    R2 = 9,
    L1 = 10,
    R1 = 11,
    Triangle = 12,
    Circle = 13,
    Cross = 14,
    Square = 15,
}

impl PadButton {
    pub const ALL: [Self; 14] = [
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Cross,
        Self::Square,
        Self::Triangle,
        Self::Circle,
        Self::Start,
        Self::Select,
        Self::L1,
        Self::R1,
        Self::L2,
        Self::R2,
    ];

    pub const fn mask(self) -> u16 {
        1 << self as u8
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Up => "↑",
            Self::Down => "↓",
            Self::Left => "←",
            Self::Right => "→",
            Self::Cross => "Cross",
            Self::Square => "Square",
            Self::Triangle => "Triangle",
            Self::Circle => "Circle",
            Self::Start => "Start",
            Self::Select => "Select",
            Self::L1 => "L1",
            Self::R1 => "R1",
            Self::L2 => "L2",
            Self::R2 => "R2",
        }
    }

    const fn default_key(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Cross => "x",
            Self::Square => "z",
            Self::Triangle => "s",
            Self::Circle => "d",
            Self::Start => "enter",
            Self::Select => "backspace",
            Self::L1 => "q",
            Self::R1 => "e",
            Self::L2 => "a",
            Self::R2 => "f",
        }
    }
}

/// Button positions correspond to bits in the GBA KEYINPUT register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GbaButton {
    A = 0,
    B = 1,
    Select = 2,
    Start = 3,
    Right = 4,
    Left = 5,
    Up = 6,
    Down = 7,
    R = 8,
    L = 9,
}

impl GbaButton {
    pub const fn mask(self) -> u16 {
        1 << self as u8
    }
}

/// Describes the buttons and factory keyboard layout of one controller.
pub trait ControllerButton: Copy + Ord + 'static {
    const BUTTONS: &'static [Self];
    fn name(self) -> &'static str;
    fn default_key(self) -> &'static str;
}

impl ControllerButton for PadButton {
    const BUTTONS: &'static [Self] = &Self::ALL;

    fn name(self) -> &'static str {
        self.name()
    }

    fn default_key(self) -> &'static str {
        self.default_key()
    }
}

impl ControllerButton for GbaButton {
    const BUTTONS: &'static [Self] = &[
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::A,
        Self::B,
        Self::Start,
        Self::Select,
        Self::L,
        Self::R,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Up => "↑",
            Self::Down => "↓",
            Self::Left => "←",
            Self::Right => "→",
            Self::A => "A",
            Self::B => "B",
            Self::Start => "Start",
            Self::Select => "Select",
            Self::L => "L",
            Self::R => "R",
        }
    }

    fn default_key(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::A => "x",
            Self::B => "z",
            Self::Start => "enter",
            Self::Select => "backspace",
            Self::L => "q",
            Self::R => "e",
        }
    }
}

pub type KeyBindings = ControllerBindings<PadButton>;
pub type GbaKeyBindings = ControllerBindings<GbaButton>;

/// Only overrides are stored; missing entries retain their original defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ControllerBindings<B: ControllerButton>(BTreeMap<B, String>);

impl<B: ControllerButton> Default for ControllerBindings<B> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<B: ControllerButton> ControllerBindings<B> {
    pub fn key(&self, button: B) -> &str {
        self.0
            .get(&button)
            .map_or(button.default_key(), String::as_str)
    }

    pub fn button(&self, key: &str) -> Option<B> {
        B::BUTTONS
            .iter()
            .copied()
            .find(|&button| self.key(button).eq_ignore_ascii_case(key))
    }

    pub fn conflict(&self, button: B) -> Option<B> {
        B::BUTTONS.iter().copied().find(|&other| {
            other != button && self.key(other).eq_ignore_ascii_case(self.key(button))
        })
    }

    /// Editing may temporarily duplicate keys; validate the complete map before applying it.
    pub fn rebind(&mut self, button: B, key: &str) -> Result<(), String> {
        if !supported_key(key) {
            return Err(format!("Unsupported controller key: {key}"));
        }
        self.0.insert(button, key.into());
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        for &button in B::BUTTONS {
            let key = self.key(button);
            if !supported_key(key) {
                return Err(format!("Unsupported controller key: {key}"));
            }
            if self.conflict(button).is_some() {
                return Err(format!("Controller key is assigned more than once: {key}"));
            }
        }
        Ok(())
    }
}

pub(crate) fn supported_key(key: &str) -> bool {
    if key.chars().count() == 1 {
        return key.chars().all(|c| !c.is_control() && !c.is_whitespace())
            && key == key.to_lowercase();
    }
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&number) && key == format!("f{number}");
    }
    matches!(
        key,
        "space"
            | "up"
            | "down"
            | "left"
            | "right"
            | "enter"
            | "backspace"
            | "delete"
            | "insert"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gba_keys_preserve_all_keyinput_bits_and_load_partial_overrides() {
        let defaults = GbaKeyBindings::default();
        for (bit, key) in [
            "x",
            "z",
            "backspace",
            "enter",
            "right",
            "left",
            "up",
            "down",
            "e",
            "q",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(defaults.button(key).unwrap().mask(), 1 << bit);
        }
        let remapped: GbaKeyBindings = serde_json::from_str(r#"{"a":"j","l":"w"}"#).unwrap();
        remapped.validate().unwrap();
        assert_eq!(remapped.button("J"), Some(GbaButton::A));
        assert_eq!(remapped.button("w"), Some(GbaButton::L));
        assert_eq!(remapped.button("x"), None);
        assert_eq!(remapped.button("up"), Some(GbaButton::Up));
    }

    #[test]
    fn rebind_allows_swaps_and_validates_only_the_completed_map() {
        let mut bindings = KeyBindings::default();
        bindings.rebind(PadButton::Cross, "z").unwrap();
        assert_eq!(bindings.conflict(PadButton::Cross), Some(PadButton::Square));
        assert!(bindings.validate().is_err());
        bindings.rebind(PadButton::Square, "x").unwrap();
        assert_eq!(bindings.button("z"), Some(PadButton::Cross));
        assert_eq!(bindings.button("X"), Some(PadButton::Square));
        bindings.validate().unwrap();
        for key in ["f25", "tab", "escape", "ctrl-r", "shift", "", "J"] {
            assert!(bindings.rebind(PadButton::Square, key).is_err());
        }
        assert_eq!(bindings.key(PadButton::Square), "x");
        bindings.rebind(PadButton::Square, "space").unwrap();
        assert_eq!(bindings.button("space"), Some(PadButton::Square));
        bindings.validate().unwrap();
    }

    #[test]
    fn partial_settings_keep_defaults_and_validate_conflicts() {
        let bindings: KeyBindings = serde_json::from_str(r#"{"cross":"j","up":"w"}"#).unwrap();
        bindings.validate().unwrap();
        assert_eq!(bindings.button("j").unwrap().mask(), 0x4000);
        assert_eq!(bindings.button("w").unwrap().mask(), 0x0010);
        assert_eq!(bindings.button("enter").unwrap().mask(), 0x0008);
        assert_eq!(bindings.button("x"), None);
        for json in [r#"{"cross":"z"}"#, r#"{"cross":"tab"}"#] {
            let bindings: KeyBindings = serde_json::from_str(json).unwrap();
            assert!(bindings.validate().is_err());
        }
    }
}
