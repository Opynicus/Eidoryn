//! Pointer and keyboard interactions shared by the collection grid and list.
use super::*;
use eidoryn_psx::{library::Placement, theme::Palette};
use gpui::{FocusHandle, MouseButton, Pixels, Point, SharedString, anchored, deferred};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy)]
enum CardAction {
    Scrape,
    Details,
    Remove,
}

impl CardAction {
    const ALL: [Self; 3] = [Self::Scrape, Self::Details, Self::Remove];

    fn item(self) -> (Text, &'static str, &'static str) {
        match self {
            Self::Scrape => (Text::LibraryScrapeMetadata, "reset", "lib-card-scrape"),
            Self::Details => (Text::LibraryDetails, "info", "lib-card-details"),
            Self::Remove => (Text::LibraryRemove, "close", "lib-card-remove"),
        }
    }
}

pub(super) struct CardMenu {
    pub(super) id: i64,
    position: Point<Pixels>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    active: usize,
}

#[derive(Clone)]
struct GameDrag {
    id: i64,
    title: SharedString,
    palette: Palette,
    offset: Point<Pixels>,
}

impl Render for GameDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        div()
            .pl(self.offset.x + px(10.))
            .pt(self.offset.y + px(10.))
            .child(
                card(p)
                    .max_w(px(210.))
                    .truncate()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(p.text))
                    .border_color(rgb(p.primary))
                    .bg(rgb(p.dialog_surface()))
                    .shadow_md()
                    .child(self.title.clone()),
            )
    }
}

fn placement(list: bool, pointer: Point<Pixels>, center: Point<Pixels>) -> Placement {
    let before = if list {
        pointer.y < center.y
    } else {
        pointer.x < center.x
    };
    if before {
        Placement::Before
    } else {
        Placement::After
    }
}

impl Desktop {
    pub(super) fn card_selected(&self, id: i64) -> bool {
        if self.library.checked_games.is_empty() {
            self.library.selected == Some(id)
        } else {
            self.library.checked_games.contains(&id)
        }
    }

    pub(super) fn collection_cell(&self, entry: &Entry, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let p = self.library_palette();
        let id = entry.game.id;
        let center = Rc::new(Cell::new(Point::default()));
        let content_center = Rc::clone(&center);
        let cell = div()
            // The first child is centered horizontally in a card and vertically in a row.
            .on_children_prepainted(move |children, _, _| {
                if let Some(first) = children.first() {
                    content_center.set(first.center());
                }
            })
            .id(("library-entry", id as u64))
            .debug_selector(move || format!("library-entry-{id}"))
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(entry.title.clone())
            .aria_selected(self.card_selected(id))
            .when(self.library.card_menu.is_none(), |cell| {
                let title = self.tr(Text::LibraryGameMenu);
                cell.tooltip(move |_, cx| shortcut(title, "Shift+F10").tooltip(p, cx))
            })
            .flex_1()
            .min_w_0()
            .flex()
            .cursor_pointer()
            .focus(move |s| s.bg(rgb(p.tonal)))
            .on_click(cx.listener(move |v, event: &gpui::ClickEvent, _, cx| {
                if !v.library.checked_games.is_empty() {
                    if event.is_keyboard() || event.click_count() == 1 {
                        v.library.toggle_checked_game(id);
                        if v.library.checked_games.is_empty() && v.library.selected == Some(id) {
                            v.library.selected = None;
                        }
                    }
                } else if event.is_keyboard()
                    || (event.click_count() >= 2 && v.library.selected == Some(id))
                {
                    v.library.select(id);
                    v.launch_library(id, cx);
                } else if event.click_count() == 1 {
                    v.library.select(id);
                }
                cx.stop_propagation();
                cx.notify();
            }));
        let keyboard_center = Rc::clone(&center);
        let drop_center = Rc::clone(&center);
        let list = self.preferences.library.list;
        let cell = cell
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v, event: &gpui::MouseDownEvent, window, cx| {
                    v.open_card_menu(id, event.position, window, cx);
                }),
            )
            .on_key_down(
                cx.listener(move |v, event: &gpui::KeyDownEvent, window, cx| {
                    let key = &event.keystroke;
                    if key.key == "menu"
                        || (key.key == "f10" && key.modifiers == gpui::Modifiers::shift())
                    {
                        v.open_card_menu(id, keyboard_center.get(), window, cx);
                        cx.stop_propagation();
                    }
                }),
            );
        if !self.library.checked_games.is_empty() {
            return cell;
        }
        cell.on_drag(
            GameDrag {
                id,
                title: entry.title.clone().into(),
                palette: p,
                offset: Point::default(),
            },
            |drag, offset, _, cx| {
                cx.new(|_| GameDrag {
                    offset,
                    ..drag.clone()
                })
            },
        )
        .drag_over::<GameDrag>(move |style, drag, window, _| {
            if drag.id == id {
                return style;
            }
            let style = style.border_color(rgb(p.primary));
            match (list, placement(list, window.mouse_position(), center.get())) {
                (true, Placement::Before) => style.border_t_2(),
                (true, Placement::After) => style.border_b_2(),
                (false, Placement::Before) => style.border_l_2(),
                (false, Placement::After) => style.border_r_2(),
            }
        })
        .on_drop(cx.listener(move |v, drag: &GameDrag, window, cx| {
            if drag.id != id {
                let side = placement(list, window.mouse_position(), drop_center.get());
                v.move_library_game(drag.id, id, side, cx);
            }
        }))
    }

    fn open_card_menu(
        &mut self,
        id: i64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.select(id);
        self.library.menu = None;
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.library.card_menu = Some(CardMenu {
            id,
            position,
            focus,
            previous_focus,
            active: 0,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn card_menu(&self, cx: &Context<Self>) -> Option<gpui::Deferred> {
        let menu = self.library.card_menu.as_ref()?;
        let p = self.library_palette();
        let content = card(&p)
            .id("lib-card-menu")
            .debug_selector(|| "lib-card-menu".into())
            .role(gpui::Role::Menu)
            .track_focus(&menu.focus)
            .occlude()
            .w(px(188.))
            .p_1()
            .shadow_md()
            .border_color(rgb(p.dialog_border()))
            .bg(rgb(p.dialog_surface()))
            .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                v.library.card_menu = None;
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::card_menu_key))
            .children(
                CardAction::ALL
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(index, action)| {
                        let (label, glyph, selector) = action.item();
                        let color = if matches!(action, CardAction::Remove) {
                            p.danger_text()
                        } else {
                            p.text
                        };
                        div()
                            .id(selector)
                            .debug_selector(move || selector.into())
                            .role(gpui::Role::MenuItem)
                            .aria_label(self.tr(label))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .h(px(34.))
                            .text_sm()
                            .text_color(rgb(color))
                            .cursor_pointer()
                            .when(menu.active == index, |row| {
                                row.aria_active_descendant().bg(rgb(p.tonal))
                            })
                            .hover(move |row| row.bg(rgb(p.tonal_hover)))
                            .child(icon(glyph, color).size(px(15.)))
                            .child(self.tr(label))
                            .on_click(cx.listener(move |v, _, window, cx| {
                                v.choose_card_action(action, window, cx);
                            }))
                    }),
            );
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(content),
            )
            .with_priority(1),
        )
    }

    fn card_menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = &mut self.library.card_menu else {
            return;
        };
        let count = CardAction::ALL.len();
        match (
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.shift,
        ) {
            ("up", _) | ("tab", true) => menu.active = (menu.active + count - 1) % count,
            ("down", _) | ("tab", false) => menu.active = (menu.active + 1) % count,
            ("enter" | "space", _) => {
                let action = CardAction::ALL[menu.active];
                self.choose_card_action(action, window, cx);
            }
            ("escape", _) => {
                window.focus(menu.previous_focus.as_ref().unwrap_or(&self.focus), cx);
                self.library.card_menu = None;
            }
            _ => {
                cx.stop_propagation();
                return;
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn choose_card_action(
        &mut self,
        action: CardAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.library.card_menu.take() else {
            return;
        };
        window.focus(&self.focus, cx);
        match action {
            CardAction::Scrape => self.fetch_metadata(menu.id, window, cx),
            CardAction::Details => {
                self.library.select(menu.id);
                self.library.details = true;
            }
            CardAction::Remove => self.confirm_game_removal(vec![menu.id], cx),
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn card_metadata_status(&self, id: i64) -> Option<gpui::Stateful<Div>> {
        let p = self.library_palette();
        let (text, color) = match self.library.metadata_lookups.get(&id)? {
            MetadataLookup::Fetching(_) => {
                (self.tr(Text::LibraryFetchingMetadata).to_owned(), p.muted)
            }
            MetadataLookup::Finished(key) => (self.tr(*key).to_owned(), p.muted),
            MetadataLookup::Failed(err) => (
                format!("{} {err}", self.tr(Text::LibraryMetadataError)),
                p.danger_text(),
            ),
        };
        let tooltip_text = SharedString::from(text);
        Some(
            label(&p, tooltip_text.clone())
                .id("lib-card-metadata-status")
                .role(gpui::Role::Status)
                .min_w_0()
                .truncate()
                .tooltip(move |_, cx| {
                    crate::desktop::components::tooltip(p, tooltip_text.clone(), cx)
                })
                .text_color(rgb(color))
                .debug_selector(|| "lib-card-metadata-status".into()),
        )
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;
