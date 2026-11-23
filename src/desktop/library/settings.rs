mod cache;

pub(super) use cache::CacheClearState;

use super::{Desktop, Text, TextInput, button, card, div, label};
use crate::desktop::{
    components::{ghost_button, page_heading},
    graphics::switch,
    icons::icon,
};
use eidoryn_psx::library::{covers, scrape::WorkerCount};
use gpui::{Context, Div, Entity, Role, prelude::*, px, rgb};
use std::{cell::Cell, rc::Rc};

pub(super) struct TemplateField {
    pub(super) input: Entity<TextInput>,
    invalid: bool,
}

impl Desktop {
    fn toggle_import_scraping(&mut self, cx: &mut Context<Self>) {
        self.preferences.library.scrape_new_games = !self.preferences.library.scrape_new_games;
        self.configure_imports();
        if !self.preferences.library.scrape_new_games {
            self.library.scraping.stop_automatic();
        }
        self.poll_library_scrape(cx);
        self.save_preferences();
        cx.notify();
    }
    pub(super) fn configure_imports(&mut self) {
        let prefs = &self.preferences.library;
        if let Some(store) = &mut self.library.store {
            store.configure_import_scraping(prefs.scrape_new_games.then_some(prefs.cover_provider));
        }
    }

    pub(super) fn cover_preferences_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = &self.library.cover {
            task.cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.library.auto_attempted.clear();
        self.configure_imports();
        self.save_preferences();
        self.auto_covers(cx);
        cx.notify();
    }

    fn settings_text(&self, title: Text, hint: Text) -> Div {
        div()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font(self.fonts.semibold())
                    .text_sm()
                    .child(self.tr(title)),
            )
            .child(label(&self.library_palette(), self.tr(hint)).text_size(px(12.)))
    }

    fn settings_card(&self, title: Text, hint: Text) -> Div {
        let p = self.library_palette();
        card(&p).px_5().py_3().gap_1().child(
            div()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .font(self.fonts.semibold())
                        .text_size(px(17.))
                        .child(self.tr(title)),
                )
                .child(label(&p, self.tr(hint)).text_size(px(12.))),
        )
    }

    fn scrape_concurrency_control(&self, covers: bool, cx: &Context<Self>) -> Div {
        let (current, title, hint, decrease, increase) = if covers {
            (
                self.preferences.library.cover_workers,
                Text::LibraryCoverRequests,
                Text::LibraryCoverRequestsHint,
                "lib-cover-workers-down",
                "lib-cover-workers-up",
            )
        } else {
            (
                self.preferences.library.scrape_workers,
                Text::LibraryMetadataRequests,
                Text::LibraryMetadataRequestsHint,
                "lib-workers-down",
                "lib-workers-up",
            )
        };
        let p = self.library_palette();
        let step = |id: &'static str, sign: &'static str, delta: i64| {
            let next = WorkerCount::from(i64::from(current.get()) + delta);
            button(&self.fonts, &p, id, sign, false)
                .size(px(32.))
                .px_0()
                .role(Role::Button)
                .aria_label(format!("{} {sign}", self.tr(title)))
                .debug_selector(move || id.into())
                .when(next == current, |b| {
                    b.opacity(0.43).cursor_default().tab_index(-1)
                })
                .when(next != current, |b| {
                    b.on_click(cx.listener(move |v, _, _, cx| {
                        if covers {
                            v.preferences.library.cover_workers = next;
                        } else {
                            v.preferences.library.scrape_workers = next;
                        }
                        v.save_preferences();
                        cx.notify();
                    }))
                })
        };
        div()
            .py_3()
            .border_t_1()
            .border_color(rgb(p.border))
            .flex()
            .items_center()
            .gap_2()
            .child(self.settings_text(title, hint))
            .child(step(decrease, "−", -1))
            .child(
                div()
                    .w(px(24.))
                    .text_center()
                    .font(self.fonts.semibold())
                    .child(current.get().to_string()),
            )
            .child(step(increase, "+", 1))
    }

    fn cover_source_menu(&self, cx: &Context<Self>) -> Div {
        use covers::Provider;
        let p = self.library_palette();
        let selected = self.preferences.library.cover_provider;
        let options = [
            (Provider::Libretro, Text::LibraryLibretro),
            (Provider::ScreenScraper, Text::LibraryScreenScraper),
            (Provider::Igdb, Text::LibraryIgdb),
            (Provider::Custom, Text::LibraryCustomCoverSource),
        ];
        let selected_name = options
            .iter()
            .find(|(provider, _)| *provider == selected)
            .map_or("", |(_, key)| self.tr(*key));
        let open = self.library.source_menu_open;
        let trigger = button(&self.fonts, &p, "lib-source-menu", selected_name, false)
            .debug_selector(|| "lib-source-menu".into())
            .role(Role::Button)
            .aria_label(self.tr(Text::LibraryCoverSource))
            .aria_expanded(open)
            .child(icon("chevron-down", p.muted))
            .on_click(cx.listener(|v, _, _, cx| {
                v.library.source_menu_open = !v.library.source_menu_open;
                cx.notify();
            }));
        let wrapper = div().relative().flex_shrink_0().child(trigger);
        if !open {
            return wrapper;
        }
        let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
        let trigger_bounds = bounds.clone();
        let menu = card(&p)
            .occlude()
            .shadow_md()
            .w(px(182.))
            .p_1()
            .bg(rgb(p.surface_variant))
            .on_mouse_down_out(cx.listener(move |v, event: &gpui::MouseDownEvent, _, cx| {
                if !bounds.get().contains(&event.position) {
                    v.library.source_menu_open = false;
                    cx.notify();
                }
            }))
            .children(options.map(|(provider, key)| {
                let id = key.id();
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(Role::Button)
                    .aria_label(self.tr(key))
                    .tab_index(0)
                    .cursor_pointer()
                    .rounded_md()
                    .px_3()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_sm()
                    .hover(move |s| s.bg(rgb(p.tonal_hover)))
                    .focus(move |s| s.bg(rgb(p.tonal_hover)))
                    .child(self.tr(key))
                    .when(selected == provider, |s| s.child(icon("check", p.primary)))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.library.source_menu_open = false;
                        if v.preferences.library.cover_provider != provider {
                            v.library.template_field = None;
                            v.preferences.library.cover_provider = provider;
                            v.cover_preferences_changed(cx);
                        }
                        w.focus(&v.focus, cx);
                        cx.notify();
                    }))
            }));
        wrapper
            .on_children_prepainted(move |children, _, _| {
                if let Some(bounds) = children.first() {
                    trigger_bounds.set(*bounds);
                }
            })
            .child(
                div()
                    .absolute()
                    .top(px(40.))
                    .right_0()
                    .w(px(182.))
                    .child(gpui::deferred(menu).with_priority(1)),
            )
    }

    fn cover_source_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let prefs = &self.preferences.library;
        let source =
            self.settings_card(Text::LibraryCoverSource, Text::LibraryCoverSourceHint)
                .child(
                    div()
                        .border_t_1()
                        .border_color(rgb(p.border))
                        .py_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(self.settings_text(
                            Text::LibrarySelectSource,
                            Text::LibrarySelectSourceHint,
                        ))
                        .child(self.cover_source_menu(cx)),
                );
        match prefs.cover_provider {
            covers::Provider::Custom => source.child(self.custom_cover_template(cx)),
            covers::Provider::ScreenScraper => {
                source.child(label(&p, self.tr(Text::LibraryScreenScraperCredentials)).pb_2())
            }
            covers::Provider::Igdb => {
                source.child(label(&p, self.tr(Text::LibraryIgdbCredentials)).pb_2())
            }
            covers::Provider::Libretro => source,
        }
    }

    fn custom_cover_template(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        let field = self
            .library
            .template_field
            .get_or_insert_with(|| TemplateField {
                input: cx.new(|cx| {
                    TextInput::named(
                        "lib-template",
                        self.preferences.library.cover_template.clone(),
                        p,
                        cx,
                    )
                }),
                invalid: false,
            });
        let input = field.input.clone();
        let invalid = field.invalid;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label(&p, self.tr(Text::LibraryTemplate)))
            .child(input)
            .when(invalid, |field| {
                field.child(
                    label(&p, self.tr(Text::LibraryInvalidTemplate)).text_color(rgb(p.on_error)),
                )
            })
            .child(div().flex().justify_end().child(self.library_button(
                "lib-save-edit",
                Text::LibrarySave,
                false,
                cx,
                |v, _, cx| {
                    let Some(field) = &mut v.library.template_field else {
                        return;
                    };
                    let value = field.input.read(cx).value().trim();
                    field.invalid = !covers::valid_template(value);
                    if !field.invalid && value != v.preferences.library.cover_template {
                        v.preferences.library.cover_template = value.to_owned();
                        v.cover_preferences_changed(cx);
                    }
                    cx.notify();
                },
            )))
    }

    fn automatic_settings(&self, cx: &Context<Self>) -> Div {
        let p = self.library_palette();
        let prefs = &self.preferences.library;
        let toggle = |id: &'static str, name, hint, value: bool| {
            switch(
                &self.fonts,
                &p,
                id,
                self.tr(name),
                self.tr(hint),
                value,
                true,
            )
            .role(Role::Switch)
            .aria_label(self.tr(name))
            .aria_toggled(value.into())
            .debug_selector(move || id.into())
        };
        self.settings_card(Text::LibraryAutoFetch, Text::LibraryAutoFetchHint)
            .child(
                toggle(
                    "lib-scrape-new",
                    Text::LibraryAutoScrapeTitle,
                    Text::LibraryScrapeNewGames,
                    prefs.scrape_new_games,
                )
                .on_click(cx.listener(|v, _, _, cx| v.toggle_import_scraping(cx))),
            )
            .child(
                toggle(
                    "lib-auto",
                    Text::LibraryAutoCoverTitle,
                    Text::LibraryAutoCovers,
                    prefs.auto_covers,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.preferences.library.auto_covers = !v.preferences.library.auto_covers;
                    v.cover_preferences_changed(cx);
                })),
            )
    }

    pub(in crate::desktop) fn close_library_source_menu(&mut self) -> bool {
        std::mem::take(&mut self.library.source_menu_open)
    }

    pub(super) fn cover_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.library_palette();
        div().flex_1().min_h_0().flex().child(
            div()
                .id("lib-settings-scroll")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(800.))
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .pb_4()
                        .child(
                            div().flex().child(
                                ghost_button(
                                    &self.fonts,
                                    &p,
                                    "lib-settings-back",
                                    self.tr(Text::LibraryBack),
                                    Some("back"),
                                )
                                .debug_selector(|| "lib-settings-back".into())
                                .role(Role::Button)
                                .aria_label(self.tr(Text::LibraryBack))
                                .on_click(cx.listener(
                                    |v, _, w, cx| {
                                        v.library.settings = false;
                                        v.library.source_menu_open = false;
                                        v.library.template_field = None;
                                        w.focus(&v.focus, cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                        )
                        .child(page_heading(
                            &self.fonts,
                            &p,
                            "LIBRARY / SETTINGS",
                            self.tr(Text::LibrarySettings),
                            self.tr(if self.is_gba() {
                                Text::LibraryMetadataCacheHint
                            } else {
                                Text::LibrarySettingsHint
                            }),
                        ))
                        .when(!self.is_gba(), |s| {
                            s.child(self.cover_source_settings(cx))
                                .child(
                                    self.settings_card(
                                        Text::LibraryConcurrentTasks,
                                        Text::LibraryConcurrentTasksHint,
                                    )
                                    .child(self.scrape_concurrency_control(false, cx))
                                    .child(self.scrape_concurrency_control(true, cx)),
                                )
                                .child(self.automatic_settings(cx))
                        })
                        .child(self.metadata_cache_settings(cx))
                        .when(!self.is_gba() && self.library.cover.is_some(), |s| {
                            s.child(self.cover_progress(cx))
                        }),
                ),
        )
    }
}
