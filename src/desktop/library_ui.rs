//! Shared presentation and keyboard scope for library pages and their dialogs.
use super::{Desktop, WorkspacePage, components::ghost_icon_button};
use eidoryn_psx::{
    i18n::Text,
    theme::{DEFAULT_THEME_ID, Palette},
};
use gpui::{
    Context, Div, FocusHandle, Role, SharedString, Window, div, prelude::*, px, relative, rgb, rgba,
};

pub(super) struct DialogFocus {
    pub scope: FocusHandle,
    pub open: bool,
    previous: Option<FocusHandle>,
}

impl DialogFocus {
    pub fn new(scope: FocusHandle) -> Self {
        Self {
            scope,
            open: false,
            previous: None,
        }
    }
}

impl Desktop {
    pub(super) fn library_palette(&self) -> Palette {
        let theme = self.themes.active();
        if theme.id != DEFAULT_THEME_ID {
            return theme.palette;
        }
        Palette {
            background: 0x101114,
            surface: 0x1a1c21,
            surface_variant: 0x212329,
            border: 0x303238,
            text: 0xf4f4f2,
            muted: 0xaaaeb7,
            primary: 0xd3f572,
            on_primary: 0x182008,
            primary_hover: 0xdff99b,
            primary_pressed: 0xb9d95c,
            tonal: 0x2a3020,
            on_tonal: 0xd3f572,
            tonal_hover: 0x303238,
            tonal_pressed: 0x3b3d44,
            error: 0x382827,
            on_error: 0xf0a49e,
            success: 0x292f20,
            on_success: 0xd3f572,
            components: None,
            ..theme.palette
        }
    }

    pub(super) fn workspace_palette(&self) -> Palette {
        if self.page == WorkspacePage::Library && !self.presentation.shows_video() {
            self.library_palette()
        } else {
            self.themes.active().palette
        }
    }

    pub(super) fn close_library_dialog(&mut self, cx: &mut Context<Self>) -> bool {
        let closed =
            self.close_detail_dialog() || self.close_task_dialog() || self.close_overview_dialog();
        if closed {
            cx.notify();
        }
        closed
    }

    pub(super) fn library_dialog_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.page != WorkspacePage::Library || self.graphics.open {
            return false;
        }
        let open = self.library_dialog_focus.open;
        if key == "escape" && (self.close_library_dialog(cx) || self.close_library_source_menu()) {
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        open
    }

    pub(super) fn sync_library_dialog_focus(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page != WorkspacePage::Library && !self.library_dialog_focus.open {
            return;
        }
        if open && !self.library_dialog_focus.open {
            // An editor can already have requested focus for its first text field.
            if !self.library_editor_focused(window, cx) {
                self.library_dialog_focus.previous = window.focused(cx);
                window.focus(&self.library_dialog_focus.scope, cx);
            }
        } else if !open && self.library_dialog_focus.open {
            let previous = self.library_dialog_focus.previous.take();
            if self.page == WorkspacePage::Library && !self.graphics.open {
                window.focus(previous.as_ref().unwrap_or(&self.focus), cx);
            }
        } else if !open {
            self.library_dialog_focus.previous = window.focused(cx);
        }
        self.library_dialog_focus.open = open;
    }

    pub(super) fn library_modal(
        &self,
        title: impl Into<SharedString>,
        body: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.library_palette();
        let title = title.into();
        let close = cx.listener(|view, _, _, cx| {
            view.close_library_dialog(cx);
        });
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .child(
                div()
                    .id("library-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(rgba(0x000000aa))
                    .on_click(close),
            )
            .child(
                div()
                    .id("library-dialog")
                    .debug_selector(|| "library-dialog".into())
                    .track_focus(&self.library_dialog_focus.scope)
                    .tab_group()
                    .role(Role::Dialog)
                    .aria_label(title.clone())
                    .occlude()
                    .w_full()
                    .max_w(px(560.))
                    .max_h(relative(0.92))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.surface))
                    .shadow_lg()
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_5()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .font(self.fonts.bold())
                                    .text_size(px(19.))
                                    .child(title),
                            )
                            .child(
                                ghost_icon_button(
                                    &p,
                                    "lib-close-dialog",
                                    "close",
                                    self.tr(Text::Close),
                                )
                                .debug_selector(|| "lib-close-dialog".into())
                                .role(Role::Button)
                                .aria_label(self.tr(Text::Close))
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        view.close_library_dialog(cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .id("library-dialog-body")
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_5()
                            .child(body),
                    ),
            )
    }
}
