use super::{
    Desktop, WorkspacePage,
    components::{button, card, ghost_button, label},
};
use eidoryn_psx::{
    i18n::Text,
    systems::ps1::runner::{Command, SelectedMedia, Snapshot},
};
use gpui::{Context, div, prelude::*, px, rgb};
use std::path::PathBuf;

impl Desktop {
    pub(super) fn bios_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let current = self.bios.as_deref();
        let name = current.map_or_else(
            || self.tr(Text::NotSelected).into(),
            |path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            },
        );
        div()
            .flex()
            .flex_col()
            .p_3()
            .gap_2()
            .rounded_md()
            .bg(rgb(p.inset_surface()))
            .border_1()
            .border_color(rgb(p.inset_border()))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(label(p, self.tr(Text::SelectedBios)))
                            .child(
                                div()
                                    .text_sm()
                                    .font(self.fonts.semibold())
                                    .truncate()
                                    .debug_selector(|| "selected-bios".into())
                                    .child(name.into_owned()),
                            ),
                    )
                    .child(
                        button(
                            &self.fonts,
                            p,
                            "open-bios",
                            self.tr(if current.is_some() {
                                Text::ChangeBios
                            } else {
                                Text::ChooseBios
                            }),
                            false,
                        )
                        .debug_selector(|| "open-bios".into())
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.pick_image(super::actions::ImagePicker::Bios, cx);
                        })),
                    ),
            )
            .child(label(p, self.tr(Text::BiosPurpose)))
            .child(
                div()
                    .id("bios-details")
                    .debug_selector(|| "bios-details".into())
                    .tab_index(0)
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(rgb(p.primary))
                    .focus(gpui::Styled::underline)
                    .hover(gpui::Styled::underline)
                    .child(format!(
                        "{} {}",
                        if self.bios_details_open { "▾" } else { "▸" },
                        self.tr(Text::BiosDetails)
                    ))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.bios_details_open = !view.bios_details_open;
                        cx.notify();
                    })),
            )
            .when(self.bios_details_open, |group| {
                group.child(self.bios_details(cx))
            })
    }

    fn bios_details(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let current = self.bios.as_deref();
        let default = self.preferences.bios_path.as_deref();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(
                [(Text::SelectedBios, current), (Text::DefaultBios, default)]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (title, path))| {
                        let full_path = path.map(|path| {
                            std::path::absolute(path)
                                .unwrap_or_else(|_| path.to_owned())
                                .display()
                                .to_string()
                        });
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(label(p, self.tr(title)))
                            .child(
                                div()
                                    .id(("bios-path", index))
                                    .w_full()
                                    .text_color(rgb(p.muted))
                                    .overflow_x_scroll()
                                    .child(div().whitespace_nowrap().text_xs().child(
                                        full_path.unwrap_or_else(|| {
                                            self.tr(Text::NotSelected).to_owned()
                                        }),
                                    )),
                            )
                    }),
            )
            .child(label(p, self.tr(Text::RememberBiosHint)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(current.is_some() && current != default, |row| {
                        row.child(
                            button(
                                &self.fonts,
                                p,
                                "set-default-bios",
                                self.tr(Text::SetDefaultBios),
                                false,
                            )
                            .debug_selector(|| "set-default-bios".into())
                            .on_click(cx.listener(|view, _, _, cx| {
                                if view.validate_current_bios(cx) {
                                    view.preferences.bios_path.clone_from(&view.bios);
                                    view.save_preferences();
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .when(default.is_some(), |row| {
                        row.child(
                            ghost_button(
                                &self.fonts,
                                p,
                                "clear-default-bios",
                                self.tr(Text::ClearDefaultBios),
                                None,
                            )
                            .debug_selector(|| "clear-default-bios".into())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.preferences.bios_path = None;
                                view.save_preferences();
                                cx.notify();
                            })),
                        )
                    }),
            )
    }

    pub(super) fn recent_games_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font(self.fonts.medium())
                            .child(self.tr(Text::RecentGames)),
                    )
                    .when(!self.preferences.recent_discs.is_empty(), |row| {
                        row.child(
                            button(
                                &self.fonts,
                                p,
                                "clear-recent-games",
                                self.tr(Text::ClearRecentGames),
                                false,
                            )
                            .debug_selector(|| "clear-recent-games".into())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.preferences.recent_discs.clear();
                                view.save_preferences();
                                cx.notify();
                            })),
                        )
                    }),
            )
            .when(self.preferences.recent_discs.is_empty(), |card| {
                card.child(label(p, self.tr(Text::NoRecentGames)))
            })
            .child(
                div()
                    .id("recent-game-list")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .children(self.preferences.recent_discs.iter().enumerate().map(
                        |(index, disc)| {
                            let path = disc.path.clone();
                            let fast_boot = disc.fast_boot;
                            let name = path
                                .file_name()
                                .unwrap_or(path.as_os_str())
                                .to_string_lossy()
                                .into_owned();
                            div()
                                .flex_shrink_0()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_3()
                                .child(
                                    button(&self.fonts, p, ("recent-game", index), name, false)
                                        .debug_selector(move || format!("recent-game-{index}"))
                                        .max_w_full()
                                        .truncate()
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.open_recent_game(path.clone(), fast_boot, cx);
                                        })),
                                )
                                .child(
                                    label(p, disc.path.display().to_string())
                                        .min_w_0()
                                        .truncate(),
                                )
                        },
                    )),
            )
    }

    pub(super) fn validate_current_bios(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.bios.clone() else {
            self.notice = Some(self.tr(Text::BiosRequired).into());
            cx.notify();
            return false;
        };
        // CLI BIOS overrides are session-only; validate without replacing the saved default.
        let saved = self.preferences.bios_path.clone();
        self.preferences.bios_path = Some(path.clone());
        let result = self.preferences.validate_saved_bios();
        let missing = self.preferences.bios_path.is_none();
        self.preferences.bios_path.clone_from(&saved);
        if missing {
            self.bios = None;
            self.notice = Some(self.tr(Text::MissingBios).into());
            if saved.as_ref() == Some(&path) {
                self.preferences.bios_path = None;
                self.save_preferences();
            }
            cx.notify();
            return false;
        }
        if let Err(error) = result {
            self.notice = Some(error);
            cx.notify();
            return false;
        }
        true
    }

    fn open_recent_game(&mut self, path: PathBuf, fast_boot: bool, cx: &mut Context<Self>) {
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                self.remove_missing_recent(&path, cx);
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.remove_missing_recent(&path, cx);
                return;
            }
            Err(error) => {
                self.notice = Some(format!("{}: {error}", path.display()));
                cx.notify();
                return;
            }
        }
        self.load_selected_disc(path, fast_boot, cx);
    }

    fn remove_missing_recent(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.preferences
            .recent_discs
            .retain(|disc| disc.path != path);
        self.notice = Some(self.tr(Text::MissingRecentGame).into());
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn load_selected_disc(
        &mut self,
        path: PathBuf,
        fast_boot: bool,
        cx: &mut Context<Self>,
    ) {
        if (!fast_boot || self.bios.is_some()) && !self.validate_current_bios(cx) {
            return;
        }
        self.command(
            Command::LoadDisc {
                path,
                bios: self.bios.clone(),
                fast_boot,
            },
            cx,
        );
    }

    pub(super) fn receive_media_selection(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        if snapshot.media_revision == self.snapshot.media_revision {
            return;
        }
        if self.page == WorkspacePage::Console
            && !self.graphics.open
            && matches!(
                snapshot.media_selection,
                Some(SelectedMedia::Demo | SelectedMedia::Disc | SelectedMedia::Exe)
            )
        {
            self.game_view = true;
        }
        match snapshot.media_selection {
            Some(SelectedMedia::Bios) => {
                if let Some(path) = &snapshot.bios_path {
                    match std::fs::canonicalize(path) {
                        Ok(path) => {
                            self.bios = Some(path);
                        }
                        Err(error) => self.notice = Some(format!("{}: {error}", path.display())),
                    }
                }
            }
            Some(SelectedMedia::Disc) => {
                if let Some(path) = &snapshot.disc_path {
                    match self
                        .preferences
                        .remember_disc(path, snapshot.disc_fast_boot)
                    {
                        Ok(()) => self.save_preferences(),
                        Err(error) => self.notice = Some(error),
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }
}
