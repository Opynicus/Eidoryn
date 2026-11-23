use super::super::{
    Desktop, WorkspacePage,
    components::{ACTION_HEIGHT, card, dropdown_button, label},
};
use eidoryn_psx::{
    i18n::Text,
    systems::{checkpoints::checkpoint_time, ps1::runner::Command},
};
use gpui::{
    Anchor, AnchoredPositionMode, Bounds, Context, FocusHandle, KeyDownEvent, Pixels, Window,
    anchored, deferred, div, point, prelude::*, px, rgb, size,
};
use std::{cell::Cell, path::PathBuf, rc::Rc};

const RECENT_SNAPSHOTS: usize = 3;

pub(in crate::desktop) struct SnapshotMenu {
    selected: usize,
    previous_focus: Option<FocusHandle>,
    trigger_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Desktop {
    pub(in crate::desktop) fn snapshot_menu_button(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let trigger = dropdown_button(
            &self.fonts,
            p,
            "manage-states",
            self.tr(Text::Snapshots),
            self.snapshot_menu.is_some(),
        )
        .debug_selector(|| "manage-states".into())
        .on_click(cx.listener(|view, _, window, cx| {
            if view.snapshot_menu.is_some() {
                view.dismiss_snapshot_menu(window, cx);
            } else {
                view.release_buttons(cx);
                view.snapshot_menu = Some(SnapshotMenu {
                    selected: 0,
                    previous_focus: window.focused(cx),
                    trigger_bounds: Rc::default(),
                });
                window.focus(&view.focus, cx);
                view.command(Command::RefreshSaveStates, cx);
            }
            cx.stop_propagation();
        }));
        let wrapper = div().relative().flex_shrink_0().child(trigger);
        let Some(menu) = &self.snapshot_menu else {
            return wrapper;
        };
        let trigger_bounds = Rc::clone(&menu.trigger_bounds);
        wrapper
            .on_children_prepainted(move |children, _, _| {
                if let Some(bounds) = children.first() {
                    trigger_bounds.set(*bounds);
                }
            })
            .child(
                div().absolute().top_0().left_0().child(
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .attach(
                                Bounds::new(point(px(0.), px(0.)), size(px(0.), ACTION_HEIGHT)),
                                Anchor::BottomLeft,
                            )
                            .offset(point(px(0.), px(4.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(self.snapshot_menu_content(menu, cx)),
                    )
                    .with_priority(1),
                ),
            )
    }

    fn snapshot_menu_content(&self, menu: &SnapshotMenu, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let entries = &self.state_history()[..self.state_history().len().min(RECENT_SNAPSHOTS)];
        let selected = menu.selected.min(entries.len());
        let trigger = Rc::clone(&menu.trigger_bounds);
        card(p)
            .debug_selector(|| "snapshot-menu".into())
            .occlude()
            .w(px(288.))
            .p_1()
            .shadow_md()
            .border_color(rgb(p.dialog_border()))
            .bg(rgb(p.dialog_surface()))
            .on_mouse_down_out(cx.listener(
                move |view, event: &gpui::MouseDownEvent, window, cx| {
                    // The trigger's click handler toggles the menu after mouse-up.
                    if !trigger.get().contains(&event.position) {
                        view.dismiss_snapshot_menu(window, cx);
                    }
                },
            ))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(entries.is_empty(), |menu| {
                menu.child(
                    label(p, self.tr(Text::NoRecentSnapshots))
                        .debug_selector(|| "snapshot-menu-empty".into())
                        .px_2()
                        .py_2(),
                )
            })
            .children(entries.iter().enumerate().map(|(index, state)| {
                let time = checkpoint_time(state.modified)
                    .unwrap_or_else(|| self.tr(Text::SnapshotTimeUnknown).to_owned());
                let path = state.path.clone();
                menu_row(p, ("recent-snapshot", index), selected == index)
                    .debug_selector(move || format!("recent-snapshot-{index}"))
                    .child(
                        div()
                            .truncate()
                            .font(self.fonts.medium())
                            .child(state.name.clone()),
                    )
                    .child(label(p, time))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.choose_snapshot_menu_item(Some(path.clone()), window, cx);
                        cx.stop_propagation();
                    }))
            }))
            .child(div().mx_1().my_1().h(px(1.)).bg(rgb(p.border)))
            .child(
                menu_row(p, "manage-all-snapshots", selected == entries.len())
                    .debug_selector(|| "manage-all-snapshots".into())
                    .child(self.tr(Text::ManageSnapshots))
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.choose_snapshot_menu_item(None, window, cx);
                        cx.stop_propagation();
                    })),
            )
    }

    fn dismiss_snapshot_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.snapshot_menu.take() {
            window.focus(menu.previous_focus.as_ref().unwrap_or(&self.focus), cx);
            cx.notify();
        }
    }

    fn choose_snapshot_menu_item(
        &mut self,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snapshot_menu = None;
        if let Some(path) = path {
            window.focus(&self.focus, cx);
            self.load_history_snapshot(path, cx);
        } else {
            self.show_page(WorkspacePage::SaveStates, window, cx);
        }
    }

    pub(in crate::desktop) fn snapshot_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = self.state_history().len().min(RECENT_SNAPSHOTS);
        let Some(menu) = &mut self.snapshot_menu else {
            return;
        };
        let selected = menu.selected.min(last);
        match (
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.shift,
        ) {
            ("up", _) | ("tab", true) => {
                menu.selected = selected.checked_sub(1).unwrap_or(last);
            }
            ("down", _) | ("tab", false) => menu.selected = (selected + 1) % (last + 1),
            ("home", _) => menu.selected = 0,
            ("end", _) => menu.selected = last,
            ("enter" | "space", _) => {
                let path = self.state_history()[..last]
                    .get(selected)
                    .map(|state| state.path.clone());
                self.choose_snapshot_menu_item(path, window, cx);
            }
            ("escape", _) => self.dismiss_snapshot_menu(window, cx),
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }
}

fn menu_row(
    p: &eidoryn_psx::theme::Palette,
    id: impl Into<gpui::ElementId>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.tonal_hover;
    div()
        .id(id)
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .py_2()
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(rgb(p.text))
        .when(selected, |row| row.bg(rgb(p.tonal)))
        .hover(move |row| row.bg(rgb(hover)))
}
