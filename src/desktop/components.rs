use gpui::{SharedString, div, prelude::*, px, rgb};

use eidoryn_psx::theme::Palette;

pub(super) fn button(
    palette: &Palette,
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let primary_hover = palette.primary_hover;
    let primary_pressed = palette.primary_pressed;
    let tonal_hover = palette.tonal_hover;
    let tonal_pressed = palette.tonal_pressed;
    div()
        .id(id)
        .px_4()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .bg(rgb(if primary {
            palette.primary
        } else {
            palette.tonal
        }))
        .text_color(rgb(if primary {
            palette.on_primary
        } else {
            palette.on_tonal
        }))
        .hover(move |style| style.bg(rgb(if primary { primary_hover } else { tonal_hover })))
        .active(move |style| {
            style.bg(rgb(if primary {
                primary_pressed
            } else {
                tonal_pressed
            }))
        })
        .child(label.into())
}

pub(super) fn card(palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded_2xl()
        .bg(rgb(palette.surface))
        .border_1()
        .border_color(rgb(palette.border))
}

pub(super) fn label(palette: &Palette, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_xs()
        .text_color(rgb(palette.muted))
        .child(text.into())
}

pub(super) fn heading(palette: &Palette, title: &'static str, detail: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(title))
        .child(label(palette, detail))
}

pub(super) fn pair(
    palette: &Palette,
    name: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex()
        .justify_between()
        .gap_2()
        .text_xs()
        .child(div().text_color(rgb(palette.muted)).child(name.into()))
        .child(div().font_family("monospace").child(value.into()))
}
