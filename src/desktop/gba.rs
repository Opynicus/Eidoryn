//! GBA presentation and input routing in the shared desktop workspace.
use super::{Desktop, GamePlatform, WorkspacePage};
use eidoryn_psx::{
    preferences::GbaRenderer,
    systems::gba::{Command, Runner, Snapshot},
};
use gpui::Context;
use std::{path::PathBuf, sync::Arc};

mod battery;

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

#[derive(Default)]
pub(super) struct Session {
    pub runner: Option<Runner>,
    pub snapshot: Snapshot,
    pub graphics_request: Option<(u64, bool)>,
    pub capturing_debug: bool,
    pub keybindings_draft: Option<eidoryn_psx::keybindings::GbaKeyBindings>,
}

impl Desktop {
    pub(super) fn open_gba(&mut self, path: PathBuf, title: String, cx: &mut Context<Self>) {
        self.switch_core(GamePlatform::Gba, cx);
        if !self.is_gba() || self.closing() {
            return;
        }
        self.library.cancel_pending_launch();
        self.image_picker.take();
        if self.gba.runner.is_none() {
            let store = self.library.store.clone();
            let result = self.cartridge_data_dir().and_then(|dir| {
                Runner::start_with_audio(
                    store,
                    dir,
                    Some(self.audio.buffer(GamePlatform::Gba).clone()),
                )
            });
            match result {
                Ok(runner) => {
                    self.gba.runner = Some(runner);
                    self.gba_command(
                        Command::SetRewindSeconds(self.preferences.rewind_seconds),
                        cx,
                    );
                    self.gba_command(
                        Command::SetRewindEnabled(self.preferences.rewind_enabled),
                        cx,
                    );
                    if self.preferences.gba_renderer == GbaRenderer::Hardware {
                        self.gba_command(
                            Command::SetHardware {
                                enabled: true,
                                request: 0,
                            },
                            cx,
                        );
                    }
                }
                Err(error) => {
                    self.notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        }
        self.stop_input(cx);
        self.snapshot_menu = None;
        self.snapshot_confirmation = None;
        self.renaming_state = None;
        self.notice = None;
        self.gba_command(Command::Load { path, title }, cx);
    }

    pub(super) fn receive_gba(&mut self, cx: &mut Context<Self>) {
        let Some(mut next) = self.gba.runner.as_ref().and_then(Runner::take_snapshot) else {
            return;
        };
        if !self.gba.capturing_debug {
            next.debug = None;
        }
        let active = self.is_gba();
        if let Some((request, enabled)) = self.gba.graphics_request
            && request == next.graphics_request
        {
            self.gba.graphics_request = None;
            if next.graphics_error.is_none() {
                self.preferences.gba_renderer = GbaRenderer::from_hardware(enabled);
                self.save_preferences();
            }
        }
        let loaded = next.media_revision != self.gba.snapshot.media_revision;
        let changed = !Arc::ptr_eq(&next.rgb, &self.gba.snapshot.rgb);
        let displays_video = matches!(self.page, WorkspacePage::Game | WorkspacePage::Console)
            || self.presentation.shows_video();
        let redraw = loaded
            || (next.video_fps - self.gba.snapshot.video_fps).abs() > f64::EPSILON
            || next.graphics_request != self.gba.snapshot.graphics_request
            || next.graphics_error != self.gba.snapshot.graphics_error
            || next.graphics_device != self.gba.snapshot.graphics_device
            || next.debug.is_some()
            || (changed && displays_video)
            || next.running != self.gba.snapshot.running
            || next.error != self.gba.snapshot.error
            || next.success_message != self.gba.snapshot.success_message
            || next.rewinding != self.gba.snapshot.rewinding
            || next.rewind_enabled != self.gba.snapshot.rewind_enabled
            || next.rewind_states != self.gba.snapshot.rewind_states
            || next.quick_slot_revision != self.gba.snapshot.quick_slot_revision
            || next.quick_slot != self.gba.snapshot.quick_slot
            || !Arc::ptr_eq(&next.save_states, &self.gba.snapshot.save_states);
        if active
            && next.renamed_state != self.gba.snapshot.renamed_state
            && let Some((old, new)) = &next.renamed_state
            && self.selected_save_state.as_ref() == Some(old)
        {
            self.selected_save_state = Some(new.clone());
        }
        if loaded && let Some(path) = &next.path {
            self.remember_media(path, false);
        }
        if active && loaded {
            self.page = WorkspacePage::Game;
            self.focus_game = true;
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
        if self.closing() {
            return;
        }
        if let Some(runner) = &self.gba.runner
            && let Err(error) = runner.send(command)
        {
            self.notice = Some(error);
            cx.notify();
        }
    }
}
