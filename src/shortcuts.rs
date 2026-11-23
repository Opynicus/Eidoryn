//! Editable keyboard shortcuts for desktop emulator actions.
use crate::keybindings::{ControllerBindings, ControllerButton, GbaKeyBindings, supported_key};
use gpui::{Keystroke, Modifiers, OperatingSystem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    Toggle,
    Enlarge,
    Crt,
    Settings,
    Step,
    Slice,
    Fullscreen,
    CleanFullscreen,
    Reset,
    Snapshot,
    QuickSave,
    QuickLoad,
    Rewind,
}

impl ShortcutAction {
    pub const ALL: [Self; 13] = [
        Self::Toggle,
        Self::Enlarge,
        Self::Crt,
        Self::Settings,
        Self::Step,
        Self::Slice,
        Self::Fullscreen,
        Self::CleanFullscreen,
        Self::Reset,
        Self::Snapshot,
        Self::QuickSave,
        Self::QuickLoad,
        Self::Rewind,
    ];

    pub fn available_on_gba(self) -> bool {
        matches!(
            self,
            Self::Toggle
                | Self::Enlarge
                | Self::Crt
                | Self::Settings
                | Self::Step
                | Self::Slice
                | Self::Fullscreen
                | Self::CleanFullscreen
                | Self::Reset
        )
    }

    fn default_keys(self) -> &'static str {
        match self {
            Self::Toggle => "f5",
            Self::Enlarge => "f6",
            Self::Crt => "f7",
            Self::Settings => "f8",
            Self::Step => "f10",
            Self::Slice => "f9",
            Self::Fullscreen => "f11",
            Self::CleanFullscreen => "f12",
            Self::Reset => "ctrl-r",
            Self::Snapshot => "ctrl-s",
            Self::QuickSave => "ctrl-shift-s",
            Self::QuickLoad => "ctrl-shift-l",
            Self::Rewind => "ctrl-z",
        }
    }
}

/// Only the key and modifier state identify a shortcut. IME text can differ by input method.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ShortcutChord {
    pub modifiers: Modifiers,
    pub key: String,
}

impl ShortcutChord {
    pub fn from_keystroke(stroke: &Keystroke) -> Option<Self> {
        let chord = Self {
            modifiers: stroke.modifiers,
            key: stroke.key.to_lowercase(),
        };
        chord.supported().then_some(chord)
    }

    fn supported(&self) -> bool {
        supported_key(&self.key) && !(self.key == "f10" && self.modifiers == Modifiers::shift())
    }

    pub fn matches_key(&self, key: &str) -> bool {
        self.key.eq_ignore_ascii_case(key) || !key.is_ascii() && self.key == key.to_lowercase()
    }

    pub fn pad_conflict<B: ControllerButton>(&self, pad: &ControllerBindings<B>) -> Option<B> {
        if self.modifiers.modified() {
            None
        } else {
            pad.button(&self.key)
        }
    }

    pub fn label(&self) -> String {
        let platform = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(windows) {
            "Win"
        } else {
            "Super"
        };
        let mut label = String::new();
        for (active, name) in [
            (self.modifiers.control, "Ctrl"),
            (self.modifiers.alt, "Alt"),
            (self.modifiers.platform, platform),
            (self.modifiers.shift, "Shift"),
            (self.modifiers.function, "Fn"),
        ] {
            if active {
                label.push_str(name);
                label.push('+');
            }
        }
        let mut chars = self.key.chars();
        if let Some(first) = chars.next() {
            label.extend(first.to_uppercase());
            label.push_str(chars.as_str());
        }
        label
    }
}

impl TryFrom<String> for ShortcutChord {
    type Error = String;

    fn try_from(keys: String) -> Result<Self, Self::Error> {
        let stroke = Keystroke::parse_with_os(&keys, OperatingSystem::Unknown)
            .map_err(|error| error.to_string())?;
        Self::from_keystroke(&stroke).ok_or_else(|| format!("Unsupported shortcut: {keys}"))
    }
}

impl From<ShortcutChord> for String {
    fn from(chord: ShortcutChord) -> Self {
        Keystroke {
            key: chord.key,
            modifiers: chord.modifiers,
            key_char: None,
        }
        .unparse(OperatingSystem::Unknown)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "BTreeMap<ShortcutAction, Option<ShortcutChord>>")]
pub struct ShortcutBindings(BTreeMap<ShortcutAction, Option<ShortcutChord>>);

impl Default for ShortcutBindings {
    fn default() -> Self {
        Self(
            ShortcutAction::ALL
                .into_iter()
                .map(|action| {
                    let stroke =
                        Keystroke::parse(action.default_keys()).expect("valid default shortcut");
                    (action, ShortcutChord::from_keystroke(&stroke))
                })
                .collect(),
        )
    }
}

impl From<BTreeMap<ShortcutAction, Option<ShortcutChord>>> for ShortcutBindings {
    fn from(overrides: BTreeMap<ShortcutAction, Option<ShortcutChord>>) -> Self {
        // Missing entries inherit defaults; explicit null keeps an action unbound.
        let mut bindings = Self::default();
        bindings.0.extend(overrides);
        bindings
    }
}

impl ShortcutBindings {
    pub fn binding(&self, action: ShortcutAction) -> Option<&ShortcutChord> {
        self.0.get(&action)?.as_ref()
    }

    pub fn action_for(&self, stroke: &Keystroke) -> Option<ShortcutAction> {
        self.0.iter().find_map(|(&action, binding)| {
            let chord = binding.as_ref()?;
            (chord.modifiers == stroke.modifiers && chord.matches_key(&stroke.key))
                .then_some(action)
        })
    }

    pub fn set(&mut self, action: ShortcutAction, chord: Option<ShortcutChord>) {
        self.0.insert(action, chord);
    }

    pub fn conflict(&self, action: ShortcutAction) -> Option<ShortcutAction> {
        let chord = self.binding(action)?;
        self.0.iter().find_map(|(&other, binding)| {
            (other != action && binding.as_ref() == Some(chord)).then_some(other)
        })
    }

    pub fn gba_controller_conflict(&self, pad: &GbaKeyBindings) -> bool {
        self.0.iter().any(|(action, chord)| {
            action.available_on_gba()
                && chord
                    .as_ref()
                    .is_some_and(|chord| chord.pad_conflict(pad).is_some())
        })
    }

    pub fn validate<B: ControllerButton>(&self, pad: &ControllerBindings<B>) -> Result<(), String> {
        for (&action, binding) in &self.0 {
            let Some(chord) = binding else { continue };
            if !chord.supported() {
                return Err(format!("Unsupported shortcut: {}", chord.label()));
            }
            if self.conflict(action).is_some() || chord.pad_conflict(pad).is_some() {
                return Err(format!("Key is assigned more than once: {}", chord.label()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::KeyBindings;

    #[test]
    fn missing_bindings_use_defaults_and_disabled_actions_survive_restart() {
        let bindings: ShortcutBindings =
            serde_json::from_str(r#"{"quick_save":"shift-f3","reset":null}"#).unwrap();
        assert_eq!(
            bindings.binding(ShortcutAction::QuickSave).unwrap().label(),
            "Shift+F3"
        );
        assert_eq!(
            bindings.binding(ShortcutAction::Snapshot).unwrap().label(),
            "Ctrl+S"
        );
        assert_eq!(bindings.binding(ShortcutAction::Reset), None);
        let saved = serde_json::to_value(&bindings).unwrap();
        assert_eq!(saved["quick_save"], "shift-f3");
        assert!(saved["reset"].is_null());
        assert_eq!(
            serde_json::from_value::<ShortcutBindings>(saved).unwrap(),
            bindings
        );
    }

    #[test]
    fn input_matches_exact_modifiers_and_ignores_composed_text() {
        let mut bindings = ShortcutBindings::default();
        let stroke = Keystroke {
            modifiers: Modifiers::alt() | Modifiers::shift(),
            key: "É".into(),
            key_char: Some("Ẽ".into()),
        };
        bindings.set(
            ShortcutAction::Rewind,
            ShortcutChord::from_keystroke(&stroke),
        );
        assert_eq!(bindings.action_for(&stroke), Some(ShortcutAction::Rewind));
        let different = Keystroke {
            modifiers: Modifiers::control(),
            ..stroke
        };
        assert_eq!(bindings.action_for(&different), None);
        assert_eq!(
            bindings.action_for(&Keystroke::parse("ctrl-shift-s").unwrap()),
            Some(ShortcutAction::QuickSave)
        );
        assert_eq!(
            bindings.action_for(&Keystroke::parse("ctrl-s").unwrap()),
            Some(ShortcutAction::Snapshot)
        );
    }

    #[test]
    fn editing_supports_swaps_and_detects_shortcut_and_controller_conflicts() {
        let mut bindings = ShortcutBindings::default();
        let pad = KeyBindings::default();
        bindings.validate(&pad).unwrap();
        let rewind = bindings.binding(ShortcutAction::Rewind).cloned();
        bindings.set(ShortcutAction::Toggle, rewind);
        assert_eq!(
            bindings.conflict(ShortcutAction::Rewind),
            Some(ShortcutAction::Toggle)
        );
        assert!(bindings.validate(&pad).is_err());
        bindings.set(ShortcutAction::Rewind, None);
        bindings.validate(&pad).unwrap();
        let cross = Keystroke::parse("x").unwrap();
        bindings.set(
            ShortcutAction::Toggle,
            ShortcutChord::from_keystroke(&cross),
        );
        assert!(bindings.validate(&pad).is_err());
        let shifted = Keystroke {
            modifiers: Modifiers::shift(),
            ..cross
        };
        bindings.set(
            ShortcutAction::Toggle,
            ShortcutChord::from_keystroke(&shifted),
        );
        bindings.validate(&pad).unwrap();
        for invalid in ["escape", "tab", "alt-tab", "shift", "shift-f10", "invalid"] {
            assert!(
                ShortcutChord::from_keystroke(&Keystroke::parse(invalid).unwrap()).is_none(),
                "{invalid}"
            );
        }
    }
}
