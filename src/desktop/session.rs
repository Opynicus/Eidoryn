//! Active core selection and shared playback routing.
use super::{Desktop, GamePlatform, WorkspacePage, actions::ImagePicker, components::button};
use eidoryn_psx::{
    i18n::Text,
    keybindings::{GbaButton, NdsButton},
    library::Library,
    systems::{gba::Command as Gba, nds::Command as Nds},
};
use gpui::{Context, div, prelude::*};
use std::path::PathBuf;

#[derive(Default, PartialEq, Eq)]
pub(super) enum CloseState {
    #[default]
    Open,
    Saving,
    WaitingForWindow,
}

impl Desktop {
    pub(super) fn cartridge_data_dir(&self) -> Result<PathBuf, String> {
        self.library
            .store
            .as_ref()
            .map(|s| s.base_dir().to_owned())
            .or_else(|| Library::default_path().and_then(|p| p.parent().map(ToOwned::to_owned)))
            .ok_or_else(|| {
                format!(
                    "Cannot locate the {} data directory",
                    self.active_core.as_str()
                )
            })
    }

    pub(super) fn is_nds(&self) -> bool {
        self.active_core == GamePlatform::Nds
    }

    pub(super) fn is_ps1(&self) -> bool {
        self.active_core == GamePlatform::Ps1
    }

    pub(super) fn is_gba(&self) -> bool {
        self.active_core == GamePlatform::Gba
    }

    pub(super) fn running(&self) -> bool {
        match self.active_core {
            GamePlatform::Ps1 => self.snapshot.running,
            GamePlatform::Gba => self.gba.snapshot.running,
            GamePlatform::Nds => self.nds.snapshot.running,
        }
    }

    pub(super) fn core_selector(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let p = self.workspace_palette();
        div()
            .id("emulator-core")
            .flex()
            .items_center()
            .gap_1()
            .aria_label(self.tr(Text::EmulatorCore))
            .children(
                [
                    (GamePlatform::Ps1, "core-ps1", "PlayStation"),
                    (GamePlatform::Gba, "core-gba", "Game Boy Advance"),
                    (GamePlatform::Nds, "core-nds", "Nintendo DS"),
                ]
                .into_iter()
                .map(|(core, id, title)| {
                    let selected = core == self.active_core;
                    button(&self.fonts, &p, id, core.as_str(), selected)
                        .debug_selector(move || id.into())
                        .role(gpui::Role::Button)
                        .aria_label(title)
                        .aria_toggled(selected.into())
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.switch_core(core, cx);
                            window.focus(&v.focus, cx);
                        }))
                }),
            )
    }

    pub(super) fn switch_core(&mut self, core: GamePlatform, cx: &mut Context<Self>) {
        if self.closing() || self.active_core == core {
            return;
        }
        self.image_picker.take();
        self.stop_input(cx);
        self.discard_binding_drafts();
        let paused = match self.active_core {
            GamePlatform::Ps1 => self.runner.send(super::Command::Pause),
            GamePlatform::Gba => self
                .gba
                .runner
                .as_ref()
                .map_or(Ok(()), |r| r.send(Gba::Pause)),
            GamePlatform::Nds => self
                .nds
                .runner
                .as_ref()
                .map_or(Ok(()), |r| r.send(Nds::Pause)),
        };
        if let Err(error) = paused {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        self.notice = None;
        self.active_core = core;
        self.audio.select(core);
        self.focus_game = false;
        self.library.cancel_pending_launch();
        self.snapshot_menu = None;
        self.snapshot_confirmation = None;
        self.renaming_state = None;
        self.graphics.open = false;
        if self.page == WorkspacePage::SaveStates {
            self.command(super::Command::RefreshSaveStates, cx);
        }
        if let Some(image) = &mut self.image {
            image.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.is_nds() {
            if self.nds.snapshot.path.is_none() {
                self.pick_image(ImagePicker::Nds, cx);
            } else {
                self.release_buttons(cx);
                self.nds_command(Nds::Toggle, cx);
            }
        } else if self.is_gba() {
            if self.gba.snapshot.path.is_none() {
                self.pick_image(ImagePicker::Gba, cx);
            } else if self.rewind_sources != 0 || self.gba.snapshot.rewinding {
                self.stop_rewind(cx);
            } else {
                self.release_buttons(cx);
                self.gba_command(Gba::Toggle, cx);
            }
        } else {
            self.command(super::Command::Toggle, cx);
        }
    }

    pub(super) fn reset_playback(&mut self, cx: &mut Context<Self>) {
        if self.is_nds() {
            self.stop_input(cx);
            self.nds_command(Nds::Reset, cx);
        } else if self.is_gba() {
            self.stop_input(cx);
            self.gba_command(Gba::Reset, cx);
        } else {
            self.command(super::Command::Reset, cx);
        }
    }

    pub(super) fn handheld_button(&self, key: &str) -> Option<u16> {
        match self.active_core {
            GamePlatform::Gba => self
                .preferences
                .gba_keybindings
                .button(key)
                .map(GbaButton::mask),
            GamePlatform::Nds => self
                .preferences
                .nds_keybindings
                .button(key)
                .map(NdsButton::mask),
            GamePlatform::Ps1 => None,
        }
    }

    pub(super) fn handheld_key(&mut self, key: &str, down: bool, cx: &mut Context<Self>) {
        if let Some(mask) = self.handheld_button(key) {
            if down == (self.pressed_buttons & mask != 0) {
                return;
            }
            if down {
                self.pressed_buttons |= mask;
            } else {
                self.pressed_buttons &= !mask;
            }
            if self.is_nds() {
                self.nds_command(Nds::Buttons(self.pressed_buttons), cx);
            } else {
                self.gba_command(Gba::Buttons(self.pressed_buttons), cx);
            }
        }
    }

    pub(super) fn request_close(&mut self, cx: &mut Context<Self>) {
        if self.gba.runner.is_none() && self.nds.runner.is_none() {
            self.command(super::Command::RequestClose, cx);
            return;
        }
        if self.close != CloseState::Open {
            return;
        }
        let mut pending = Vec::new();
        if let Some(runner) = &self.gba.runner {
            let (done, result) = std::sync::mpsc::channel();
            pending.push(runner.send(Gba::PrepareClose(done)).map(|()| result));
        }
        if let Some(runner) = &self.nds.runner {
            let (done, result) = std::sync::mpsc::channel();
            pending.push(runner.send(Nds::PrepareClose(done)).map(|()| result));
        }
        self.close = CloseState::Saving;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    for result in pending {
                        result?.recv().map_err(|e| e.to_string())??;
                    }
                    Ok::<_, String>(())
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                match saved.and_then(|()| view.runner.send(super::Command::RequestClose)) {
                    Ok(()) => view.close = CloseState::WaitingForWindow,
                    Err(error) => {
                        view.close = CloseState::Open;
                        view.notice = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn closing(&self) -> bool {
        self.close != CloseState::Open
    }

    pub(super) fn receive_close_error(&mut self, snapshot: &super::Snapshot) {
        if self.close == CloseState::WaitingForWindow
            && !snapshot.close_ready
            && let Some(error) = &snapshot.message
        {
            self.notice = Some(error.clone());
            self.close = CloseState::Open;
        }
    }
}
