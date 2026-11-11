use super::icons::icon;
use eidoryn_psx::theme::Palette;
use gpui::{SharedString, div, prelude::*, px, rgb};

pub(super) fn button(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    action(p, id, text, None, primary)
}

pub(super) fn action(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    symbol: Option<&'static str>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let (background, foreground, hover, pressed) = if primary {
        (p.primary, p.on_primary, p.primary_hover, p.primary_pressed)
    } else {
        (p.surface_variant, p.text, p.tonal_hover, p.tonal_pressed)
    };
    let text = text.into();
    let tooltip_text = text.clone();
    let palette = *p;
    div()
        .id(id)
        .tab_index(0)
        .px_4()
        .h(px(38.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded_lg()
        .cursor_pointer()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .bg(rgb(background))
        .border_1()
        .border_color(rgb(background))
        .focus(move |style| style.border_color(rgb(palette.primary)))
        .tooltip(move |_, cx| tooltip(palette, tooltip_text.clone(), cx))
        .text_color(rgb(foreground))
        .hover(move |style| style.bg(rgb(hover)))
        .active(move |style| style.bg(rgb(pressed)))
        .children(symbol.map(|symbol| icon(symbol, foreground)))
        .child(text)
}

pub(super) fn icon_button(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    symbol: &'static str,
    title: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.tonal_hover;
    let palette = *p;
    div()
        .id(id)
        .tab_index(0)
        .size(px(38.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .border_1()
        .border_color(rgb(p.border))
        .focus(move |style| style.border_color(rgb(palette.primary)))
        .tooltip(move |_, cx| tooltip(palette, title.into(), cx))
        .bg(rgb(p.surface_variant))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(hover)))
        .child(icon(symbol, p.muted))
}

pub(super) fn tooltip(palette: Palette, text: SharedString, cx: &mut gpui::App) -> gpui::AnyView {
    cx.new(|_| Tooltip { palette, text }).into()
}

struct Tooltip {
    palette: Palette,
    text: SharedString,
}

impl gpui::Render for Tooltip {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .max_w(px(320.))
            .bg(rgb(self.palette.surface_variant))
            .text_color(rgb(self.palette.text))
            .border_1()
            .border_color(rgb(self.palette.border))
            .text_sm()
            .child(self.text.clone())
    }
}

pub(super) fn card(p: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded_2xl()
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.border))
}

pub(super) fn label(p: &Palette, text: impl Into<SharedString>) -> gpui::Div {
    div().text_xs().text_color(rgb(p.muted)).child(text.into())
}

pub(super) fn heading(p: &Palette, title: &'static str, detail: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(label(p, detail))
}

pub(super) fn pair(
    p: &Palette,
    name: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex()
        .justify_between()
        .gap_3()
        .text_xs()
        .child(div().text_color(rgb(p.muted)).child(name.into()))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.text))
                .child(value.into()),
        )
}
