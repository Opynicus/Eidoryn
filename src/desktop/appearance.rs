use super::{
    Desktop,
    components::{action, label},
};
use eidoryn_psx::i18n::{Language, Text};
use gpui::{Context, PathPromptOptions, SharedString, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.setting_group(Text::Language, cx, |cx| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(label(p, self.tr(Text::LanguageHint)))
                    .child(self.language_choices(cx))
            }))
            .child(self.setting_group(Text::Theme, cx, |cx| self.theme_choices(cx)))
    }

    fn theme_choices(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(label(p, self.tr(Text::ThemeHint)))
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_2()
                    .children(self.themes.themes().iter().map(|theme| {
                        let id = theme.id.clone();
                        let selector = format!("theme-{id}");
                        let selected = id == self.themes.active().id;
                        let palette = theme.palette;
                        let focus_color = p.primary;
                        let hover = p.tonal_hover;
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .role(gpui::Role::RadioButton)
                            .aria_label(theme.name.clone())
                            .aria_toggled(selected.into())
                            .tab_index(0)
                            .focus(move |style| style.border_color(rgb(focus_color)).border_2())
                            .min_w_0()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(rgb(if selected { p.primary } else { p.border }))
                            .bg(rgb(p.surface_variant))
                            .cursor_pointer()
                            .hover(move |style| style.bg(rgb(hover)))
                            .child(
                                div().flex().gap_1().children(
                                    [palette.background, palette.primary, palette.text]
                                        .into_iter()
                                        .map(|color| {
                                            div()
                                                .size(px(16.))
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(rgb(palette.border))
                                                .bg(rgb(color))
                                        }),
                                ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font(self.fonts.medium())
                                    .text_color(rgb(p.text))
                                    .child(theme.name.clone()),
                            )
                            .child(label(
                                p,
                                if selected {
                                    self.tr(Text::CurrentTheme)
                                } else {
                                    self.tr(Text::ApplyTheme)
                                },
                            ))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.select_theme(&id, cx);
                            }))
                    })),
            )
            .child(
                action(
                    &self.fonts,
                    p,
                    "load-theme",
                    self.tr(Text::LoadTheme),
                    Some("plus"),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.open_theme(cx))),
            )
            .child(label(p, self.tr(Text::ThemePluginHint)))
    }

    fn language_choices(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(Language::ALL.iter().map(|&language| {
                super::components::button(
                    &self.fonts,
                    p,
                    language.id(),
                    language.native_name(),
                    false,
                )
                .debug_selector(move || language.id().into())
                .when(self.language == language, |button| {
                    button
                        .bg(rgb(p.tonal))
                        .border_color(rgb(p.primary))
                        .text_color(rgb(p.on_tonal))
                })
                .on_click(cx.listener(move |view, _, _, cx| view.set_language(language, cx)))
            }))
    }

    fn open_theme(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr(Text::ThemePicker).into()),
        });
        cx.spawn(async move |this, cx| {
            let result = picker.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            match view.themes.load_plugin(path) {
                                Ok(id) => match std::path::absolute(path) {
                                    Ok(path) => {
                                        view.theme_paths.insert(id.clone(), path);
                                        view.select_theme(&id, cx);
                                    }
                                    Err(error) => view.notice = Some(error.to_string()),
                                },
                                Err(error) => view.notice = Some(error),
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        view.notice = Some(format!("{}: {error}", view.tr(Text::ThemePickerError)));
                    }
                    Err(error) => {
                        view.notice = Some(format!("{}: {error}", view.tr(Text::ThemePickerError)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
