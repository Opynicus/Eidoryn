//! Cache the static navigation independently of video-frame notifications.
use super::{Desktop, WorkspacePage};
use eidoryn_psx::{i18n::Language, theme::Palette};
use gpui::{
    AnyView, Context, Render, StyleRefinement, Subscription, ViewElement, WeakEntity, Window, div,
    prelude::*, px,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    palette: Palette,
    language: Language,
    page: WorkspacePage,
    settings_open: bool,
}

impl Key {
    fn new(desktop: &Desktop) -> Self {
        Self {
            palette: desktop.themes.active().palette,
            language: desktop.language,
            page: desktop.page,
            settings_open: desktop.graphics.open,
        }
    }
}

pub(super) struct Sidebar {
    desktop: WeakEntity<Desktop>,
    key: Key,
    compact: bool,
    _subscription: Subscription,
    #[cfg(all(test, feature = "ui-tests"))]
    pub(super) renders: usize,
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, feature = "ui-tests"))]
        {
            self.renders += 1;
        }
        // GPUI invokes child rendering after Desktop::render has returned. Keep
        // the existing navigation callbacks and font cache owned by Desktop;
        // the weak handle avoids a parent/child reference cycle.
        self.desktop
            .update(cx, |desktop, cx| desktop.sidebar_content(self.compact, cx))
            .unwrap_or_else(|_| div())
    }
}

impl Desktop {
    pub(super) fn cached_sidebar(
        &mut self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> ViewElement<AnyView> {
        let sidebar = if let Some(sidebar) = &self.sidebar {
            if sidebar.read(cx).compact != compact {
                // The cached wrapper changes width in this draw, which already
                // invalidates GPUI's bounds key. Its content must use the same mode.
                sidebar.update(cx, |sidebar, _| sidebar.compact = compact);
            }
            sidebar.clone()
        } else {
            let desktop = cx.entity();
            let key = Key::new(self);
            let sidebar = cx.new(|cx| Sidebar {
                desktop: desktop.downgrade(),
                key,
                compact,
                // Observe before the next draw; notifying during render would
                // leave the cached navigation stale for one frame.
                _subscription: cx.observe(&desktop, |sidebar: &mut Sidebar, desktop, cx| {
                    let key = Key::new(desktop.read(cx));
                    if sidebar.key != key {
                        sidebar.key = key;
                        cx.notify();
                    }
                }),
                #[cfg(all(test, feature = "ui-tests"))]
                renders: 0,
            });
            self.sidebar = Some(sidebar.clone());
            sidebar
        };
        // Bounds, inherited text style, focus/hover refreshes and DPI changes
        // are also validated by GPUI's view cache, not just the key above.
        AnyView::from(sidebar).cached(
            StyleRefinement::default()
                .w(if compact { px(72.) } else { px(224.) })
                .h_full()
                .flex_shrink_0(),
        )
    }
}
