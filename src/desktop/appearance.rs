use super::{
    Desktop,
    components::{action, label},
};
use eidoryn_psx::i18n::Text;
use gpui::{Context, PathPromptOptions, SharedString, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Language)),
            )
            .child(label(p, self.tr(Text::LanguageHint)))
            .child(self.language_choices(cx))
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Theme)),
            )
            .child(label(p, self.tr(Text::ThemeHint)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.themes.themes().iter().map(|theme| {
                        let id = theme.id.clone();
                        let selected = id == self.themes.active().id;
                        let palette = theme.palette;
                        let focus_color = p.primary;
                        div()
                            .id(SharedString::from(format!("theme-{id}")))
                            .tab_index(0)
                            .focus(move |style| style.border_color(rgb(focus_color)).border_2())
                            .w(gpui::rems(10.))
                            .max_w_full()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(if selected { p.primary } else { p.border }))
                            .bg(rgb(p.surface_variant))
                            .cursor_pointer()
                            .child(
                                div().flex().gap_1().children(
                                    [palette.background, palette.primary, palette.success]
                                        .into_iter()
                                        .map(|color| {
                                            div().size(px(19.)).rounded_md().bg(rgb(color))
                                        }),
                                ),
                            )
                            .child(
                                div()
                                    .text_sm()
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
        div().flex().gap_2().children(
            [
                (eidoryn_psx::i18n::Language::En, "English"),
                (eidoryn_psx::i18n::Language::ZhCn, "简体中文"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (language, name))| {
                super::components::button(
                    &self.fonts,
                    p,
                    ("language", index),
                    name,
                    self.language == language,
                )
                .on_click(cx.listener(move |view, _, _, cx| view.set_language(language, cx)))
            }),
        )
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
