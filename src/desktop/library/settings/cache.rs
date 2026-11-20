use super::{Desktop, Text};
use crate::desktop::{
    components::{action, label},
    graphics::switch,
    icons::icon,
    library::OverviewMenu,
};
use eidoryn_psx::library::metadata::CacheScope;
use gpui::{Context, Div, Role, div, prelude::*, px, rgb};

#[derive(Debug, PartialEq, Eq)]
pub(in crate::desktop::library) enum CacheClearState {
    Idle,
    Running,
    Finished,
    Failed(String),
}

impl Desktop {
    pub(super) fn metadata_cache_settings(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let enabled = self.preferences.library.clear_metadata_on_remove;
        let running = self.library.cache_clear == CacheClearState::Running;
        let toggle = switch(
            &self.fonts,
            &p,
            "lib-clear-metadata-on-remove",
            self.tr(Text::LibraryClearMetadataOnRemove),
            self.tr(Text::LibraryClearMetadataOnRemoveHint),
            enabled,
            true,
        )
        .role(Role::Switch)
        .aria_label(self.tr(Text::LibraryClearMetadataOnRemove))
        .aria_toggled(enabled.into())
        .debug_selector(|| "lib-clear-metadata-on-remove".into())
        .on_click(cx.listener(|v, _, _, cx| {
            v.preferences.library.clear_metadata_on_remove =
                !v.preferences.library.clear_metadata_on_remove;
            v.save_preferences();
            cx.notify();
        }));
        let key = if running {
            Text::LibraryClearingMetadata
        } else {
            Text::LibraryClearMetadataCache
        };
        let open = action(
            &self.fonts,
            &p,
            "lib-clear-metadata",
            self.tr(key),
            None,
            false,
        )
        .role(Role::Button)
        .aria_label(self.tr(key))
        .debug_selector(|| "lib-clear-metadata".into())
        .when(running, |b| {
            b.opacity(0.45).tab_stop(false).cursor_default()
        })
        .when(!running, |b| {
            b.on_click(cx.listener(|v, _, _, cx| {
                v.library.menu = Some(OverviewMenu::ClearMetadata(CacheScope::Removed));
                cx.notify();
            }))
        });
        let status = match &self.library.cache_clear {
            CacheClearState::Finished => {
                Some(label(&p, self.tr(Text::LibraryMetadataCacheCleared)))
            }
            CacheClearState::Failed(error) => Some(
                label(
                    &p,
                    format!("{} {error}", self.tr(Text::LibraryMetadataCacheClearFailed)),
                )
                .text_color(rgb(p.danger_text())),
            ),
            _ => None,
        };
        self.settings_card(Text::LibraryMetadataCache, Text::LibraryMetadataCacheHint)
            .child(toggle)
            .child(
                div()
                    .py_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(open)
                    .when_some(status, |row, message| {
                        row.child(
                            message
                                .id("lib-cache-clear-status")
                                .role(Role::Status)
                                .debug_selector(|| "lib-cache-clear-status".into()),
                        )
                    }),
            )
    }

    pub(in crate::desktop::library) fn metadata_cache_dialog(
        &self,
        scope: CacheScope,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.library_palette();
        let scopes = [
            (
                CacheScope::Removed,
                Text::LibraryCacheScopeRemoved,
                "lib-cache-scope-removed",
            ),
            (
                CacheScope::All,
                Text::LibraryCacheScopeAll,
                "lib-cache-scope-all",
            ),
        ];
        let choices =
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(scopes.map(|(option, title, id)| {
                    let selected = option == scope;
                    action(&self.fonts, &p, id, self.tr(title), None, false)
                        .role(Role::RadioButton)
                        .aria_label(self.tr(title))
                        .aria_toggled(selected.into())
                        .debug_selector(move || id.into())
                        .justify_between()
                        .when(selected, |b| {
                            b.bg(rgb(p.tonal))
                                .border_color(rgb(p.primary))
                                .child(icon("check", p.primary))
                        })
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.library.menu = Some(OverviewMenu::ClearMetadata(option));
                            cx.notify();
                        }))
                }));
        let hint = match scope {
            CacheScope::Removed => Text::LibraryClearMetadataRemovedHint,
            CacheScope::All => Text::LibraryClearMetadataAllHint,
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .child(self.library_button(
                "lib-cache-cancel",
                Text::LibraryCancel,
                false,
                cx,
                |v, _, cx| {
                    v.library.menu = None;
                    cx.notify();
                },
            ))
            .child(
                self.library_button(
                    "lib-cache-confirm",
                    Text::LibraryClearMetadataCache,
                    false,
                    cx,
                    move |v, _, cx| {
                        v.library.menu = None;
                        v.clear_library_metadata_cache(scope, cx);
                    },
                )
                .bg(rgb(p.error))
                .text_color(rgb(p.danger_text())),
            );
        self.library_modal(
            self.tr(Text::LibraryClearMetadataCache),
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(choices)
                .child(label(&p, self.tr(hint)).text_sm())
                .child(label(&p, self.tr(Text::LibraryClearMetadataKeep)).text_size(px(12.)))
                .child(actions),
            cx,
        )
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;
