use super::{
    Desktop,
    components::{button, card, label},
};
use gpui::{Context, PathPromptOptions, SharedString, div, prelude::*};

impl Desktop {
    pub(super) fn appearance(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .p_4()
            .gap_3()
            .flex_shrink_0()
            .child(label(p, "Appearance"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .children(self.themes.themes().iter().map(|theme| {
                        let id = theme.id.clone();
                        button(
                            p,
                            SharedString::from(format!("theme-{id}")),
                            theme.name.clone(),
                            id == self.themes.active().id,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.notice = view.themes.select(&id).err();
                            cx.notify();
                        }))
                    }))
                    .child(
                        button(p, "load-theme", "Load theme…", false)
                            .on_click(cx.listener(|_, _, _, cx| Self::open_theme(cx))),
                    )
                    .child(
                        button(p, "close-appearance", "Done", false).on_click(cx.listener(
                            |view, _, _, cx| {
                                view.appearance_open = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
    }

    fn open_theme(cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Load a Eidoryn theme plugin (.json)".into()),
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
