use super::{
    Desktop,
    components::{action, label},
};
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
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("界面主题"),
            )
            .child(label(p, "主题只改变界面配色，游戏画面保持原样。"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.themes.themes().iter().map(|theme| {
                        let id = theme.id.clone();
                        let selected = id == self.themes.active().id;
                        let palette = theme.palette;
                        div()
                            .id(SharedString::from(format!("theme-{id}")))
                            .w(px(160.))
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
                                    "当前主题"
                                } else {
                                    "点击应用"
                                },
                            ))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.notice = view.themes.select(&id).err();
                                cx.notify();
                            }))
                    })),
            )
            .child(
                action(p, "load-theme", "加载自定义主题文件", Some("plus"), false)
                    .on_click(cx.listener(|_, _, _, cx| Self::open_theme(cx))),
            )
            .child(label(p, "支持 JSON 主题插件；应用后无需重启游戏。"))
    }

    fn open_theme(cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("加载 Eidoryn 主题插件（.json）".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = picker.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            view.notice = view
                                .themes
                                .load_plugin(path)
                                .and_then(|id| view.themes.select(&id))
                                .err();
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => view.notice = Some(format!("Theme picker: {error}")),
                    Err(error) => view.notice = Some(format!("Theme picker: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
