//! Interface translations embedded at build time from the TOML files in `locales/`.

use std::str::FromStr;

struct Catalog {
    id: &'static str,
    native_name: &'static str,
    aliases: &'static [&'static str],
    messages: &'static [&'static str],
}

// The build script orders every catalog's translations by the English message keys.
macro_rules! catalogs {
    (
        keys { $($key:ident,)* }
        languages { $($(#[$attr:meta])* $language:ident => ($id:literal, $name:literal,
            [$($alias:literal),* $(,)?], [$($message:literal),* $(,)?]),)* }
    ) => {
        /// A stable interface message key shared by every catalog.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Text { $($key,)* }

        impl Text {
            /// The untranslated key used in catalog files and widget identifiers.
            pub const fn id(self) -> &'static str {
                match self { $(Self::$key => stringify!($key),)* }
            }

            #[cfg(test)]
            const ALL: &'static [Self] = &[$(Self::$key,)*];
        }

        /// An interface language included in this build.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum Language { $($(#[$attr])* $language,)* }

        impl Language {
            /// All available languages in settings display order.
            pub const ALL: &'static [Self] = &[$(Self::$language,)*];

            const fn catalog(self) -> &'static Catalog {
                match self {
                    $(Self::$language => &Catalog {
                        id: $id,
                        native_name: $name,
                        aliases: &[$($alias,)*],
                        messages: &[$($message,)*],
                    },)*
                }
            }
        }
    };
}

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

impl FromStr for Language {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|language| language.id().eq_ignore_ascii_case(value))
            .ok_or_else(|| {
                format!(
                    "Unsupported language: {value}; choose from {}",
                    Self::supported_ids()
                )
            })
    }
}

impl Language {
    /// The locale tag used by the CLI and saved preferences.
    pub const fn id(self) -> &'static str {
        self.catalog().id
    }

    /// The language's name in its own writing system.
    pub const fn native_name(self) -> &'static str {
        self.catalog().native_name
    }

    /// The comma-separated locale tags accepted by the CLI.
    pub fn supported_ids() -> String {
        Self::ALL
            .iter()
            .map(|language| language.id())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Resolve a system locale, trying progressively less specific tags and their aliases.
    /// Unmatched locales use the default interface language.
    pub fn from_locale(value: &str) -> Self {
        let locale = value
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .replace('_', "-");
        let mut tag = locale.as_str();
        loop {
            if let Some(language) = Self::ALL.iter().copied().find(|language| {
                let catalog = language.catalog();
                catalog.id.eq_ignore_ascii_case(tag)
                    || catalog
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(tag))
            }) {
                return language;
            }
            let Some((parent, _)) = tag.rsplit_once('-') else {
                return Self::default();
            };
            tag = parent;
        }
    }

    /// Pick the UI language using the first configured POSIX messages locale.
    pub fn preferred() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
            .map_or_else(Self::default, |locale| Self::from_locale(&locale))
    }

    /// Look up a message, using the English text for untranslated keys.
    pub const fn text(self, key: Text) -> &'static str {
        self.catalog().messages[key as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_languages_round_trip_and_cover_the_interface() {
        for &language in Language::ALL {
            assert_eq!(language.id().parse(), Ok(language));
            assert_eq!(Language::from_locale(language.id()), language);
            assert!(!language.native_name().trim().is_empty());
            for &key in Text::ALL {
                assert!(
                    !language.text(key).trim().is_empty(),
                    "{} / {}",
                    language.id(),
                    key.id()
                );
            }
        }
        assert_eq!(Language::En.text(Text::SaveStates), "Save states");
        assert_eq!(Language::ZhCn.text(Text::SaveStates), "即时存档");
    }

    #[test]
    fn system_locales_support_region_script_and_posix_suffixes() {
        for tag in ["zh", "ZH_cn.UTF-8", "zh-Hans-x-eidoryn", "zh_CN@modifier"] {
            assert_eq!(Language::from_locale(tag), Language::ZhCn, "{tag}");
        }
        for tag in ["en.UTF-8", "C", "POSIX", "", "not-a-locale!"] {
            assert_eq!(Language::from_locale(tag), Language::En, "{tag}");
        }
        assert_eq!("ZH-cn".parse(), Ok(Language::ZhCn));
        // An explicit option must identify a shipped catalog.
        assert!("not-a-locale!".parse::<Language>().is_err());
    }
}
