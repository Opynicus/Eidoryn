//! Desktop keyboard bindings for the PS1 digital controller.
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

/// Only overrides are stored; missing entries retain their original defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyBindings(BTreeMap<PadButton, String>);

impl KeyBindings {
    pub fn key(&self, button: PadButton) -> &str {
        self.0
            .get(&button)
            .map_or(button.default_key(), String::as_str)
    }

    pub fn button(&self, key: &str) -> Option<PadButton> {
        PadButton::ALL
            .into_iter()
            .find(|&button| self.key(button).eq_ignore_ascii_case(key))
    }

    pub fn conflict(&self, button: PadButton) -> Option<PadButton> {
        PadButton::ALL.into_iter().find(|&other| {
            other != button && self.key(other).eq_ignore_ascii_case(self.key(button))
        })
    }

    /// Editing may temporarily duplicate keys; validate the complete map before applying it.
    pub fn rebind(&mut self, button: PadButton, key: &str) -> Result<(), String> {
        if !supported_key(key) {
            return Err(format!("Unsupported controller key: {key}"));
        }
        self.0.insert(button, key.into());
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        for button in PadButton::ALL {
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

fn supported_key(key: &str) -> bool {
    if key.chars().count() == 1 {
        return key.chars().all(|c| !c.is_control() && !c.is_whitespace())
            && key == key.to_lowercase();
    }
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&number)
            && !(5..=12).contains(&number)
            && key == format!("f{number}");
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
    fn rebind_allows_swaps_and_validates_only_the_completed_map() {
        let mut bindings = KeyBindings::default();
        bindings.rebind(PadButton::Cross, "z").unwrap();
        assert_eq!(bindings.conflict(PadButton::Cross), Some(PadButton::Square));
        assert!(bindings.validate().is_err());
        bindings.rebind(PadButton::Square, "x").unwrap();
        assert_eq!(bindings.button("z"), Some(PadButton::Cross));
        assert_eq!(bindings.button("X"), Some(PadButton::Square));
        bindings.validate().unwrap();
        for key in [
            "f5", "tab", "escape", "f6", "f8", "f12", "ctrl-r", "shift", "", "J",
        ] {
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
        for json in [r#"{"cross":"z"}"#, r#"{"cross":"f5"}"#] {
            let bindings: KeyBindings = serde_json::from_str(json).unwrap();
            assert!(bindings.validate().is_err());
        }
    }
}
