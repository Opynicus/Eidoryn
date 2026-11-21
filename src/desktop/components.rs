use super::{fonts::Fonts, icons::icon};
use eidoryn_psx::theme::Palette;
use gpui::{SharedString, div, prelude::*, px, rgb};

pub(super) const ACTION_HEIGHT: gpui::Pixels = px(36.);

pub(super) struct ButtonLabel {
    text: SharedString,
    shortcut: Option<&'static str>,
}

impl<T: Into<SharedString>> From<T> for ButtonLabel {
    fn from(text: T) -> Self {
        Self {
            text: text.into(),
            shortcut: None,
        }
    }
}

pub(super) fn shortcut(text: impl Into<SharedString>, keys: &'static str) -> ButtonLabel {
    ButtonLabel {
        text: text.into(),
        shortcut: Some(keys),
    }
}

impl ButtonLabel {
    pub(super) fn tooltip(&self, palette: Palette, cx: &mut gpui::App) -> gpui::AnyView {
        let hint = match self.shortcut {
            Some(keys) => format!("{} · {keys}", self.text).into(),
            None => self.text.clone(),
        };
        tooltip(palette, hint, cx)
    }
}

pub(super) fn button(
    fonts: &Fonts,
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: impl Into<ButtonLabel>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    action(fonts, p, id, text, None, primary)
}

pub(super) fn action(
    fonts: &Fonts,
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: impl Into<ButtonLabel>,
    symbol: Option<&'static str>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let (background, hover) = if primary {
        (p.primary, p.primary_hover)
    } else {
        (p.button_surface(), p.tonal_hover)
    };
    with_label(action_content(fonts, p, id, symbol, primary), p, text)
        .bg(rgb(background))
        .hover(move |style| style.bg(rgb(hover)))
}

pub(super) fn ghost_button(
    fonts: &Fonts,
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: impl Into<ButtonLabel>,
    symbol: Option<&'static str>,
) -> gpui::Stateful<gpui::Div> {
    with_label(
        ghost(action_content(fonts, p, id, symbol, false), p),
        p,
        text,
    )
}

pub(super) fn dropdown_button(
    fonts: &Fonts,
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    text: &'static str,
    expanded: bool,
) -> gpui::Stateful<gpui::Div> {
    ghost(action_content(fonts, p, id, None, false), p)
        .child(text)
        .child(label(p, if expanded { "▴" } else { "▾" }))
}

fn with_label(
    button: gpui::Stateful<gpui::Div>,
    p: &Palette,
    text: impl Into<ButtonLabel>,
) -> gpui::Stateful<gpui::Div> {
    let hint = text.into();
    let text = hint.text.clone();
    let palette = *p;
    button
        .when(!text.is_empty(), |button| {
            button.tooltip(move |_, cx| hint.tooltip(palette, cx))
        })
        .child(text)
}

fn action_content(
    fonts: &Fonts,
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    symbol: Option<&'static str>,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let (foreground, pressed) = if primary {
        (p.on_primary, p.primary_pressed)
    } else {
        (p.text, p.tonal_pressed)
    };
    let palette = *p;
    div()
        .id(id)
        .tab_index(0)
        .px_3()
        .h(ACTION_HEIGHT)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(12.))
        .font(fonts.medium())
        .border_1()
        .border_color(rgb(if primary { p.primary } else { p.border }))
        .focus(move |style| {
            style.border_color(rgb(if primary {
                palette.on_primary
            } else {
                palette.primary
            }))
        })
        .text_color(rgb(foreground))
        .active(move |style| style.bg(rgb(pressed)))
        .children(symbol.map(|symbol| icon(symbol, foreground)))
}

pub(super) fn icon_button(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    symbol: &'static str,
    title: impl Into<ButtonLabel>,
) -> gpui::Stateful<gpui::Div> {
    let hover = p.tonal_hover;
    icon_button_content(p, id, symbol, title)
        .bg(rgb(p.button_surface()))
        .hover(move |style| style.bg(rgb(hover)))
}

pub(super) fn ghost_icon_button(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    symbol: &'static str,
    title: impl Into<ButtonLabel>,
) -> gpui::Stateful<gpui::Div> {
    ghost(icon_button_content(p, id, symbol, title), p)
}

fn icon_button_content(
    p: &Palette,
    id: impl Into<gpui::ElementId>,
    symbol: &'static str,
    title: impl Into<ButtonLabel>,
) -> gpui::Stateful<gpui::Div> {
    let palette = *p;
    let title = title.into();
    div()
        .id(id)
        .tab_index(0)
        .size(px(36.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(p.border))
        .focus(move |style| style.border_color(rgb(palette.primary)))
        .tooltip(move |_, cx| title.tooltip(palette, cx))
        .cursor_pointer()
        .active(move |style| style.bg(rgb(palette.tonal_pressed)))
        .child(icon(symbol, p.text))
}

fn ghost(button: gpui::Stateful<gpui::Div>, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let hover = p.ghost_hover();
    button
        .bg(gpui::transparent_black())
        .hover(move |style| style.bg(rgb(hover)))
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
            .debug_selector(|| format!("tooltip-{}", self.text))
            .px_3()
            .py_2()
            .rounded_md()
            .max_w(px(320.))
            .bg(rgb(self.palette.surface_variant))
            .text_color(rgb(self.palette.text))
            .border_1()
            .border_color(rgb(self.palette.border))
            .text_size(px(12.))
            .child(self.text.clone())
    }
}

pub(super) fn card(p: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.border))
}

pub(super) fn label(p: &Palette, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(11.))
        .text_color(rgb(p.muted))
        .child(text.into())
}

pub(super) fn heading(
    fonts: &Fonts,
    p: &Palette,
    title: &'static str,
    detail: &'static str,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().text_size(px(13.)).font(fonts.semibold()).child(title))
        .child(label(p, detail))
}

pub(super) fn page_heading(
    fonts: &Fonts,
    p: &Palette,
    eyebrow: &'static str,
    title: impl Into<SharedString>,
    description: &'static str,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            label(p, eyebrow)
                .text_color(rgb(p.soft_text()))
                .font(fonts.bold()),
        )
        .child(
            div()
                .text_size(px(30.))
                .font(fonts.title())
                .child(title.into()),
        )
        .child(label(p, description).text_size(px(14.)))
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
