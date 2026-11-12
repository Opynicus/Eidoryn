//! Reuse successful font requests, including GPUI's platform fallback choice.
use gpui::{Font, FontWeight, TextSystem, font};
use std::{cell::OnceCell, sync::Arc};

pub(super) struct Fonts {
    system: Arc<TextSystem>,
    resolved: [OnceCell<Font>; 5],
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::*;

    #[gpui::test]
    fn resolved_requests_keep_the_original_faces(cx: &mut gpui::TestAppContext) {
        let system = cx.text_system();
        let fonts = Fonts::new(Arc::clone(system));
        for (weight, resolved) in [
            (FontWeight::NORMAL, fonts.normal()),
            (FontWeight::MEDIUM, fonts.medium()),
            (FontWeight::SEMIBOLD, fonts.semibold()),
            (FontWeight::BOLD, fonts.bold()),
        ] {
            let mut requested = font("sans-serif");
            requested.weight = weight;
            assert_eq!(
                system.resolve_font(&requested),
                system.resolve_font(&resolved)
            );
        }
        assert!(fonts.resolved[4].get().is_none());
        assert_eq!(
            system.resolve_font(&font("monospace")),
            system.resolve_font(&fonts.mono())
        );
    }
}

impl Fonts {
    pub(super) fn new(system: Arc<TextSystem>) -> Self {
        Self {
            system,
            resolved: std::array::from_fn(|_| OnceCell::new()),
        }
    }

    pub(super) fn normal(&self) -> Font {
        self.get(0, "sans-serif", FontWeight::NORMAL)
    }

    pub(super) fn medium(&self) -> Font {
        self.get(1, "sans-serif", FontWeight::MEDIUM)
    }

    pub(super) fn semibold(&self) -> Font {
        self.get(2, "sans-serif", FontWeight::SEMIBOLD)
    }

    pub(super) fn bold(&self) -> Font {
        self.get(3, "sans-serif", FontWeight::BOLD)
    }

    pub(super) fn mono(&self) -> Font {
        self.get(4, "monospace", FontWeight::NORMAL)
    }

    fn get(&self, slot: usize, family: &'static str, weight: FontWeight) -> Font {
        self.resolved[slot]
            .get_or_init(|| {
                let mut requested = font(family);
                requested.weight = weight;
                let id = self.system.resolve_font(&requested);
                // An unavailable family can fall back to a different weight, too.
                // Keep the entire successful request so glyphs and metrics stay
                // identical, without reconstructing cached lookup errors per label.
                self.system.get_font_for_id(id).unwrap_or(requested)
            })
            .clone()
    }
}
