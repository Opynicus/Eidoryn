//! Cache the static navigation independently of video-frame notifications.
use super::{Desktop, WorkspacePage, components::label, icons::icon};
use eidoryn_psx::{
    app,
    i18n::{Language, Text},
    theme::Palette,
};
use gpui::{
    AnyView, Context, Render, StyleRefinement, Subscription, ViewElement, WeakEntity, Window, div,
    prelude::*, px, rgb,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    palette: Palette,
    language: Language,
    page: WorkspacePage,
    core: super::GamePlatform,
}

impl Key {
    fn new(desktop: &Desktop) -> Self {
        Self {
            palette: desktop.workspace_palette(),
            language: desktop.language,
            page: desktop.page,
            core: desktop.library.platform,
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
        width: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> ViewElement<AnyView> {
        let compact = width < SNAP_WIDTH;
        let sidebar = if let Some(sidebar) = &self.sidebar {
            if sidebar.read(cx).compact != compact {
                // The cached wrapper changes width in this draw, which already
                // invalidates GPUI's bounds key. Its content must use the same mode.
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.compact = compact;
                    cx.notify();
                });
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
        AnyView::from(sidebar).cached(StyleRefinement::default().w(width).h_full().flex_shrink_0())
    }
}

const ICON_WIDTH: gpui::Pixels = px(72.);
const FULL_WIDTH: gpui::Pixels = px(200.);
const SNAP_WIDTH: gpui::Pixels = px(136.);

#[derive(Default, Clone, Copy)]
enum SidebarWidth {
    #[default]
    Automatic,
    Icons,
    Expanded,
}

#[derive(Default)]
pub(super) struct Navigation {
    width: SidebarWidth,
    pub(super) hidden: bool,
    drag_width: Option<gpui::Pixels>,
}

impl Navigation {
    pub(super) fn width(&self, window: &Window) -> gpui::Pixels {
        if self.hidden {
            return px(0.);
        }
        self.drag_width.unwrap_or_else(|| match self.width {
            SidebarWidth::Automatic if window.viewport_size().width <= px(1250.) => ICON_WIDTH,
            SidebarWidth::Automatic | SidebarWidth::Expanded => FULL_WIDTH,
            SidebarWidth::Icons => ICON_WIDTH,
        })
    }

    pub(super) fn cancel_drag(&mut self) {
        self.drag_width = None;
    }

    pub(super) fn is_dragging(&self) -> bool {
        self.drag_width.is_some()
    }

    pub(super) fn track_drag(&mut self, x: gpui::Pixels) {
        self.drag_width = Some(x.clamp(ICON_WIDTH, FULL_WIDTH));
    }

    pub(super) fn finish_drag(&mut self, x: gpui::Pixels) {
        self.drag_width = None;
        self.width = if x < SNAP_WIDTH {
            SidebarWidth::Icons
        } else {
            SidebarWidth::Expanded
        };
    }
}

impl Desktop {
    pub(super) fn sidebar_handle(
        &self,
        width: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.workspace_palette();
        let hint = self.tr(eidoryn_psx::i18n::Text::ResizeSidebar);
        div()
            .id("sidebar-resize")
            .group("sidebar-resize")
            .debug_selector(|| "sidebar-resize".into())
            .tab_index(0)
            .absolute()
            .left(width - px(8.))
            .top_0()
            .bottom_0()
            .w(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .tooltip(move |_, cx| super::components::tooltip(p, hint.into(), cx))
            .focus(move |s| s.border_1().border_color(rgb(p.primary)))
            .child(
                div()
                    .w(px(2.))
                    .h(px(48.))
                    .rounded_full()
                    .bg(rgb(p.resize_handle()).opacity(0.6))
                    .group_hover("sidebar-resize", move |s| s.bg(rgb(p.primary))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |view, _, _, cx| {
                    view.navigation.track_drag(width);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                let width = match event.keystroke.key.as_str() {
                    "left" | "home" => ICON_WIDTH,
                    "right" | "end" => FULL_WIDTH,
                    _ => return,
                };
                view.navigation.finish_drag(width);
                cx.stop_propagation();
                cx.notify();
            }))
    }
}

const DESTINATIONS: [(&str, &str, Text, WorkspacePage); 5] = [
    ("nav-console", "grid", Text::Console, WorkspacePage::Console),
    ("nav-library", "disc", Text::Library, WorkspacePage::Library),
    ("nav-game", "screen", Text::GameView, WorkspacePage::Game),
    (
        "nav-save-states",
        "disc",
        Text::SaveStates,
        WorkspacePage::SaveStates,
    ),
    ("nav-debug", "code", Text::Debugger, WorkspacePage::Debug),
];

impl Desktop {
    fn sidebar_content(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.workspace_palette();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .px(px(12.))
            .py(px(24.))
            .bg(rgb(p.sidebar()))
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_1()
                    .mb_6()
                    .when(compact, gpui::Styled::justify_center)
                    .child(
                        div()
                            .size(px(34.))
                            .flex_shrink_0()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(rgb(p.brand_border()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(match super::icons::app_icon() {
                                Some(image) => gpui::img(image)
                                    .debug_selector(|| "sidebar-app-icon".into())
                                    .size_full()
                                    .rounded(px(7.))
                                    .into_any_element(),
                                None => div()
                                    .text_size(px(20.))
                                    .font(self.fonts.title())
                                    .text_color(rgb(p.primary))
                                    .child("E")
                                    .into_any_element(),
                            }),
                    )
                    .when(!compact, |brand| {
                        brand.child(
                            div()
                                .text_size(px(18.))
                                .font(self.fonts.bold())
                                .child(app::NAME),
                        )
                    }),
            )
            .when(!compact, |rail| {
                rail.child(
                    label(p, self.tr(Text::Workspace))
                        .text_color(rgb(p.soft_text()))
                        .font(self.fonts.bold())
                        .px_3()
                        .pb_2(),
                )
            })
            .children(
                DESTINATIONS
                    .into_iter()
                    .filter(|(_, _, _, page)| {
                        !self.is_gba()
                            || !matches!(page, WorkspacePage::Debug | WorkspacePage::SaveStates)
                    })
                    .map(|(id, symbol, title, page)| {
                        let selected = page == self.page;
                        nav_item(p, id, symbol, self.tr(title), compact, selected).on_click(
                            cx.listener(move |view, _, window, cx| {
                                view.show_page(page, window, cx);
                            }),
                        )
                    }),
            )
            .child(div().flex_1())
            .when(!compact, |rail| {
                rail.child(
                    div()
                        .mx_1()
                        .border_t_1()
                        .border_color(rgb(p.border))
                        .pt_3()
                        .child(
                            label(p, format!("EIDORYN / {}", self.library.platform.as_str()))
                                .font(self.fonts.bold()),
                        )
                        .child(
                            label(p, concat!("v", env!("CARGO_PKG_VERSION")))
                                .text_color(rgb(p.soft_text())),
                        ),
                )
            })
    }
}

fn nav_item(
    p: &Palette,
    id: &'static str,
    symbol: &'static str,
    text: &'static str,
    compact: bool,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.tonal_hover;
    let palette = *p;
    div()
        .id(id)
        .debug_selector(|| id.to_owned())
        .tab_index(0)
        .border_1()
        .border_color(gpui::transparent_black())
        .focus(move |style| style.border_color(rgb(palette.primary)))
        .tooltip(move |_, cx| super::components::tooltip(palette, text.into(), cx))
        .h(px(40.))
        .px(px(10.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_3()
        .rounded_md()
        .bg(if selected {
            rgb(p.tonal)
        } else {
            gpui::transparent_black().into()
        })
        .text_color(rgb(if selected { p.text } else { p.muted }))
        .cursor_pointer()
        .when(compact, gpui::Styled::justify_center)
        .hover(move |style| style.bg(rgb(hover)))
        .child(icon(symbol, if selected { p.primary } else { p.muted }))
        .when(!compact, |element| {
            element.child(div().text_size(px(14.)).child(text))
        })
}
