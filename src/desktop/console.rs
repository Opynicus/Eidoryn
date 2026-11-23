use super::{
    Desktop,
    components::{
        action, button, card, ghost_button, ghost_icon_button, heading, icon_button, label,
        page_heading,
    },
};
use eidoryn_psx::{i18n::Text, shortcuts::ShortcutAction, systems::ps1::runner::Command};
use gpui::{Context, Window, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn console_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let width = f32::from(self.content_width(window));
        let stacked = width < 630.;
        let session_width = if stacked { width } else { (width - 16.) * 0.45 };
        let preview_height = ((session_width - 34.) * 0.75).clamp(100., 240.);
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(page_heading(
                &self.fonts,
                p,
                self.tr(Text::YourPlayspace),
                self.tr(Text::Welcome),
                self.tr(if self.is_gba() {
                    Text::GbaWelcome
                } else {
                    Text::WelcomeHint
                }),
            ))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(stacked, gpui::Styled::flex_col)
                    .when(!stacked, gpui::Styled::items_start)
                    .child(self.launch_card(cx).flex_1().min_w_0())
                    .child(
                        self.session_card(preview_height, window, cx)
                            .when(!stacked, |card| card.w(px(session_width)).flex_shrink_0()),
                    ),
            )
            .when(!self.is_gba(), |s| s.child(self.recent_games_card(cx)))
    }

    fn gba_launch_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "launch-card".into())
            .p_5()
            .gap_4()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::LaunchGame),
                "Game Boy Advance",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        action(
                            &self.fonts,
                            p,
                            "open-gba",
                            self.tr(Text::OpenCartridge),
                            Some("play"),
                            true,
                        )
                        .debug_selector(|| "open-gba".into())
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.pick_image(super::actions::ImagePicker::Gba, cx);
                        })),
                    )
                    .child(
                        button(
                            &self.fonts,
                            p,
                            "open-gba-library",
                            self.tr(Text::Library),
                            false,
                        )
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.show_page(super::WorkspacePage::Library, w, cx);
                        })),
                    ),
            )
    }

    fn launch_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        if self.is_gba() {
            return self.gba_launch_card(cx);
        }
        card(p)
            .debug_selector(|| "launch-card".into())
            .p_5()
            .gap_4()
            .child(heading(&self.fonts, p, self.tr(Text::LaunchGame), "BIOS"))
            .child(self.bios_controls(cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        action(
                            &self.fonts,
                            p,
                            "open-disc",
                            self.tr(Text::OpenDisc),
                            Some("disc"),
                            true,
                        )
                        .debug_selector(|| "open-disc".into())
                        .on_click(cx.listener(|view, _, _, cx| view.open_disc(false, cx))),
                    )
                    .child(
                        button(&self.fonts, p, "demo", self.tr(Text::Demo), false).on_click(
                            cx.listener(|view, _, _, cx| view.command(Command::Demo, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_1()
                    .child(
                        self.launch_link("open-exe", Text::OpenExe)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.pick_image(super::actions::ImagePicker::Exe, cx);
                            })),
                    )
                    .child(
                        self.launch_link("fast-boot", Text::FastBoot)
                            .on_click(cx.listener(|view, _, _, cx| view.open_disc(true, cx))),
                    )
                    .child(
                        self.launch_link("card-details", Text::MemoryCards)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.memory_details_open = !view.memory_details_open;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.memory_details_open, |group| {
                group.child(self.memory_card(cx))
            })
    }

    fn launch_link(&self, id: &'static str, text: Text) -> gpui::Stateful<gpui::Div> {
        let p = self.themes.active().palette;
        div()
            .id(id)
            .tab_index(0)
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(rgb(p.muted))
            .underline()
            .focus(move |s| s.text_color(rgb(p.primary)))
            .hover(move |s| s.text_color(rgb(p.text)))
            .child(self.tr(text))
    }

    fn session_card(&self, height: f32, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "session-card".into())
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::Session),
                self.playback_label(),
            ))
            .child(self.display_stage(height))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font(self.fonts.semibold())
                                    .truncate()
                                    .child(self.display_title().to_owned()),
                            )
                            .child(label(
                                p,
                                if self.is_gba() {
                                    "Game Boy Advance"
                                } else if self.snapshot.disc_mounted {
                                    self.tr(Text::DiscSession)
                                } else {
                                    self.tr(Text::LocalSession)
                                },
                            )),
                    )
                    .child(self.play_button(cx))
                    .when(!window.is_fullscreen(), |row| {
                        row.child(self.enlargement_button(window, cx))
                    })
                    .child(self.fullscreen_button(window, cx)),
            )
    }

    fn display_stage(&self, height: f32) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .debug_selector(|| "display-stage".into())
            .w_full()
            .h(px(height))
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(p.stage_border()))
            .bg(rgb(p.video_background))
            .child(if self.is_gba() && self.gba.snapshot.path.is_none() {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label(p, self.tr(Text::OpenCartridge)))
                    .into_any_element()
            } else if !self.is_gba()
                && self.snapshot.is_demo
                && self.snapshot.execution_state()
                    == eidoryn_psx::systems::ps1::runner::ExecutionState::Paused
                && self.snapshot.cpu.instructions == 0
            {
                div()
                    .debug_selector(|| "demo-standby".into())
                    .size_full()
                    .bg(rgb(p.standby_surface()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(
                        div()
                            .font(self.fonts.title())
                            .text_size(px(32.))
                            .text_color(rgb(p.standby_title()))
                            .child("EIDORYN"),
                    )
                    .child(
                        label(p, self.tr(Text::PlayStationEmulator))
                            .text_color(rgb(p.standby_caption())),
                    )
                    .child(
                        label(p, self.tr(Text::DemoStandby))
                            .text_color(rgb(p.standby_hint()))
                            .mt_3(),
                    )
                    .into_any_element()
            } else {
                self.video().into_any_element()
            })
    }

    pub(super) fn play_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let (text, symbol) = if !self.is_gba() && self.snapshot.rewinding {
            (Text::EndRewind, "pause")
        } else if self.running() {
            (Text::Pause, "pause")
        } else {
            (Text::Run, "play")
        };
        action(
            &self.fonts,
            &self.themes.active().palette,
            "transport-run",
            self.shortcut_label(self.tr(text), ShortcutAction::Toggle, None),
            Some(symbol),
            true,
        )
        .debug_selector(|| "transport-run".into())
        .when(self.snapshot.rewinding, |b| b.tab_index(-1))
        .on_click(cx.listener(|view, _, _, cx| view.toggle_playback(cx)))
    }

    pub(super) fn game_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let width = f32::from(self.content_width(window));
        let stacked = width < 720.;
        let side_width = 262.;
        let stage_width = if stacked {
            width
        } else {
            width - side_width - 16.
        };
        // Leave room for wrapped playback controls in a compact workspace.
        let chrome_height = if stage_width < 700. { 340. } else { 284. };
        let height = ((stage_width - 34.) / self.video_aspect())
            .min(f32::from(window.viewport_size().height) - chrome_height)
            .max(160.);
        div()
            .debug_selector(|| "game-controls".into())
            .flex()
            .flex_col()
            .gap_4()
            .child(if window.is_fullscreen() {
                label(p, self.playback_label())
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        page_heading(
                            &self.fonts,
                            p,
                            self.tr(Text::NowPlaying),
                            self.display_title().to_owned(),
                            self.playback_label(),
                        )
                        .flex_1(),
                    )
                    .child(self.enlargement_button(window, cx))
            })
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(stacked, gpui::Styled::flex_col)
                    .when(!stacked, gpui::Styled::items_start)
                    .child(
                        card(p)
                            .p_4()
                            .gap_3()
                            .min_w_0()
                            .flex_1()
                            .child(self.display_stage(height))
                            .child(self.game_toolbar(window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .when(!stacked, |sidebar| {
                                sidebar.w(px(side_width)).flex_shrink_0()
                            })
                            .when(self.is_gba(), |s| {
                                s.child(
                                    card(p)
                                        .p_4()
                                        .gap_3()
                                        .child(heading(
                                            &self.fonts,
                                            p,
                                            self.tr(Text::Controller),
                                            "GBA",
                                        ))
                                        .child(label(p, self.tr(Text::GbaKeys))),
                                )
                            })
                            .when(!self.is_gba(), |s| {
                                s.child(self.quick_slot_card())
                                    .child(
                                        card(p)
                                            .p_4()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font(self.fonts.semibold())
                                                    .child(self.tr(Text::Rewind)),
                                            )
                                            .child(self.rewind_controls(cx)),
                                    )
                                    .child(self.memory_card(cx))
                            }),
                    ),
            )
    }

    fn game_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .debug_selector(|| "transport".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(self.play_button(cx))
                    .when(!self.is_gba(), |s| {
                        s.child(self.quick_state_controls(cx))
                            .child(self.snapshot_menu_button(cx))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        ghost_icon_button(
                            p,
                            "transport-reset",
                            "reset",
                            self.shortcut_label(self.tr(Text::Reset), ShortcutAction::Reset, None),
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.reset_playback(cx))),
                    )
                    .child(self.crt_control(cx))
                    .child(self.fullscreen_button(window, cx))
                    .child(div().flex_1())
                    .when(!self.is_gba(), |s| s.child(self.audio_controls(cx))),
            )
    }

    pub(super) fn enlargement_button(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (id, icon, label, fallback) = if window.is_fullscreen() {
            ("restore-game", "back", Text::RestoreGame, Some("Esc"))
        } else {
            ("enlarge-game", "screen", Text::EnlargeGame, None)
        };
        ghost_button(
            &self.fonts,
            &self.themes.active().palette,
            id,
            self.shortcut_label(self.tr(label), ShortcutAction::Enlarge, fallback),
            Some(icon),
        )
        .debug_selector(move || id.into())
        .on_click(cx.listener(|view, _, window, cx| view.toggle_view(window, cx)))
    }

    pub(super) fn fullscreen_button(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (id, icon, label, fallback) =
            if window.is_fullscreen() && self.presentation.shows_video() {
                ("exit-fullscreen", "back", Text::ExitFullscreen, Some("Esc"))
            } else {
                ("enter-fullscreen", "expand", Text::Fullscreen, None)
            };
        icon_button(
            &self.themes.active().palette,
            id,
            icon,
            self.shortcut_label(self.tr(label), ShortcutAction::Fullscreen, fallback),
        )
        .debug_selector(move || id.into())
        .on_click(cx.listener(|view, _, window, cx| view.toggle_fullscreen(window, cx)))
    }

    fn memory_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "memory-card".into())
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::MemoryCards),
                self.tr(Text::MemoryCard),
            ))
            .children(self.snapshot.cards.iter().enumerate().map(|(slot, card)| {
                let state = card.as_ref().map_or(Text::Disconnected, |card| {
                    if card.pending {
                        Text::Saving
                    } else {
                        Text::Synced
                    }
                });
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .size(px(28.))
                            .rounded_md()
                            .bg(rgb(p.surface_variant))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(format!("{:02}", slot + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().child(format!(
                                "{} {}",
                                self.tr(Text::Slot),
                                slot + 1
                            )))
                            .when_some(card.as_ref(), |slot, card| {
                                let name = card
                                    .path
                                    .as_deref()
                                    .and_then(|p| p.file_name())
                                    .map_or_else(
                                        || self.tr(Text::TemporaryCard).to_owned(),
                                        |name| name.to_string_lossy().into_owned(),
                                    );
                                slot.child(label(p, name).truncate())
                            }),
                    )
                    .child(label(p, self.tr(state)))
            }))
            .child(
                button(
                    &self.fonts,
                    p,
                    "flush-cards",
                    self.tr(Text::SyncCards),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.command(Command::FlushCards, cx))),
            )
            .child(label(p, self.tr(Text::SaveHint)))
    }
}
