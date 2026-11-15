use super::{
    Desktop,
    components::{button, card, label, pair},
};
use eidoryn_psx::{
    i18n::Text,
    systems::ps1::runner::{Command, SelectedMedia, Snapshot},
};
use gpui::{Context, div, prelude::*, px};
use std::path::PathBuf;

impl Desktop {
    pub(super) fn bios_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let bios = self
            .bios
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || self.tr(Text::NotSelected).into(),
                |name| name.to_string_lossy().into_owned(),
            );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(pair(p, "BIOS", bios))
            .child(label(p, self.tr(Text::RememberBiosHint)))
            .when(self.preferences.bios_path.is_some(), |group| {
                group.child(
                    button(
                        &self.fonts,
                        p,
                        "clear-default-bios",
                        self.tr(Text::ClearDefaultBios),
                        false,
                    )
                    .debug_selector(|| "clear-default-bios".into())
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.preferences.bios_path = None;
                        view.save_preferences();
                        cx.notify();
                    })),
                )
            })
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
        match snapshot.media_selection {
            Some(SelectedMedia::Bios) => {
                if let Some(path) = &snapshot.bios_path {
                    match std::fs::canonicalize(path) {
                        Ok(path) => {
                            self.bios = Some(path.clone());
                            self.preferences.bios_path = Some(path);
                            self.save_preferences();
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
