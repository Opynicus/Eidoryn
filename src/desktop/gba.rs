//! GBA presentation and input routing in the shared desktop workspace.
use super::{Desktop, GamePlatform, WorkspacePage, actions::ImagePicker, components::button};
use eidoryn_psx::{
    i18n::Text,
    library::Library,
    systems::gba::{Command, Runner, Snapshot},
};
use gpui::{Context, div, prelude::*};
use std::{path::PathBuf, sync::Arc};

mod firmware;

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

#[derive(Default)]
pub(super) struct Session {
    pub runner: Option<Runner>,
    pub snapshot: Snapshot,
    pub focus_game: bool,
    pub bios: Option<PathBuf>,
    close: CloseState,
}

impl Session {
    pub(super) fn with_bios(bios: Option<PathBuf>) -> Self {
        Self {
            bios,
            ..Self::default()
        }
    }
}

#[derive(Default, PartialEq, Eq)]
enum CloseState {
    #[default]
    Open,
    Saving,
    WaitingForWindow,
}

const KEYS: [&str; 10] = [
    "x",
    "z",
    "backspace",
    "enter",
    "right",
    "left",
    "up",
    "down",
    "e",
    "q",
];

impl Desktop {
    pub(super) fn is_gba(&self) -> bool {
        self.library.platform == GamePlatform::Gba
    }

    pub(super) fn running(&self) -> bool {
        if self.is_gba() {
            self.gba.snapshot.running
        } else {
            self.snapshot.running
        }
    }

    pub(super) fn supports_shortcut(&self, action: eidoryn_psx::shortcuts::ShortcutAction) -> bool {
        use eidoryn_psx::shortcuts::ShortcutAction::{
            CleanFullscreen, Crt, Enlarge, Fullscreen, Reset, Settings, Toggle,
        };
        !self.is_gba()
            || matches!(
                action,
                Toggle | Reset | Enlarge | Crt | Settings | Fullscreen | CleanFullscreen
            )
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
                ]
                .into_iter()
                .map(|(core, id, title)| {
                    let selected = core == self.library.platform;
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
        if self.gba.close != CloseState::Open || self.library.platform == core {
            return;
        }
        self.bios_selection.take();
        self.stop_input(cx);
        if self.is_gba() {
            self.gba_command(Command::Pause, cx);
        } else if let Err(error) = self.runner.send(super::Command::Pause) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        self.notice = None;
        self.library.change_platform(core);
        self.reorder_library();
        self.snapshot_menu = None;
        self.snapshot_confirmation = None;
        self.renaming_state = None;
        self.graphics.open = false;
        if matches!(self.page, WorkspacePage::SaveStates | WorkspacePage::Debug) {
            self.page = WorkspacePage::Console;
        }
        if let Some(image) = &mut self.image {
            image.dirty = true;
        }
        if self.page == WorkspacePage::Library {
            self.enter_library(cx);
        }
        cx.notify();
    }

    fn start_gba(&mut self, path: PathBuf, title: String, bios: PathBuf, cx: &mut Context<Self>) {
        if self.gba.runner.is_none() {
            let store = self.library.store.clone();
            let base = store.as_ref().map(|s| s.base_dir().to_owned()).or_else(|| {
                Library::default_path().and_then(|p| p.parent().map(ToOwned::to_owned))
            });
            let result = base
                .ok_or_else(|| "Cannot locate the GBA save directory".to_owned())
                .and_then(|dir| Runner::start(store, dir.join("saves/gba")));
            match result {
                Ok(runner) => self.gba.runner = Some(runner),
                Err(error) => {
                    self.notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        }
        self.release_buttons(cx);
        self.notice = None;
        self.gba_command(Command::Load { path, title, bios }, cx);
    }

    pub(super) fn receive_gba(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.gba.runner.as_ref().and_then(Runner::take_snapshot) else {
            return;
        };
        let active = self.is_gba();
        let loaded = next.media_revision != self.gba.snapshot.media_revision;
        let changed = !Arc::ptr_eq(&next.rgb, &self.gba.snapshot.rgb);
        let displays_video = matches!(self.page, WorkspacePage::Game | WorkspacePage::Console)
            || self.presentation.shows_video();
        let redraw = loaded
            || (changed && displays_video)
            || next.running != self.gba.snapshot.running
            || next.error != self.gba.snapshot.error;
        if active && loaded {
            self.page = WorkspacePage::Game;
            self.gba.focus_game = true;
            self.presentation.windowed_video = false;
        }
        if active
            && changed
            && let Some(image) = &mut self.image
        {
            image.dirty = true;
        }
        self.gba.snapshot = next;
        if active && redraw {
            cx.notify();
        }
    }

    pub(super) fn gba_command(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.gba.close != CloseState::Open {
            return;
        }
        if let Some(runner) = &self.gba.runner
            && let Err(error) = runner.send(command)
        {
            self.notice = Some(error);
            cx.notify();
        }
    }

    pub(super) fn request_close(&mut self, cx: &mut Context<Self>) {
        let Some(runner) = &self.gba.runner else {
            self.command(super::Command::RequestClose, cx);
            return;
        };
        if self.gba.close != CloseState::Open {
            return;
        }
        let (done, result) = std::sync::mpsc::channel();
        if let Err(error) = runner.send(Command::PrepareClose(done)) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        self.gba.close = CloseState::Saving;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { result.recv().map_err(|e| e.to_string()).and_then(|r| r) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match saved.and_then(|()| view.runner.send(super::Command::RequestClose)) {
                    Ok(()) => view.gba.close = CloseState::WaitingForWindow,
                    Err(error) => {
                        view.gba.close = CloseState::Open;
                        view.notice = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn closing(&self) -> bool {
        self.gba.close != CloseState::Open
    }

    pub(super) fn receive_close_error(&mut self, snapshot: &super::Snapshot) {
        if self.gba.close == CloseState::WaitingForWindow
            && !snapshot.close_ready
            && let Some(error) = &snapshot.message
        {
            self.notice = Some(error.clone());
            self.gba.close = CloseState::Open;
        }
    }

    pub(super) fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.is_gba() {
            if self.gba.snapshot.path.is_none() {
                self.pick_image(ImagePicker::Gba, cx);
            } else {
                self.release_buttons(cx);
                self.gba_command(Command::Toggle, cx);
            }
        } else {
            self.command(super::Command::Toggle, cx);
        }
    }

    pub(super) fn reset_playback(&mut self, cx: &mut Context<Self>) {
        if self.is_gba() {
            self.release_buttons(cx);
            self.gba_command(Command::Reset, cx);
        } else {
            self.command(super::Command::Reset, cx);
        }
    }

    pub(super) fn gba_key(&mut self, key: &str, down: bool, cx: &mut Context<Self>) {
        if let Some(i) = KEYS.iter().position(|&k| k == key) {
            if down {
                self.pressed_buttons |= 1 << i;
            } else {
                self.pressed_buttons &= !(1 << i);
            }
            self.gba_command(Command::Buttons(self.pressed_buttons), cx);
        }
    }
}
