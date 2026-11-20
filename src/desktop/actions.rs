use super::{Desktop, FullscreenView, SettingsTab, WorkspacePage};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::systems::ps1::runner::Command;
use gpui::{Context, KeyDownEvent, KeyUpEvent, PathPromptOptions, Window};

#[derive(Clone, Copy)]
pub(super) enum ImagePicker {
    Bios,
    Exe,
}

impl Desktop {
    pub(super) fn stop_input(&mut self, cx: &mut Context<Self>) {
        self.stop_rewind(cx);
        self.navigation.cancel_drag();
        if self.rebinding.take().is_some() {
            cx.notify();
        }
        if self.audio.dragging {
            self.persist_volume();
        }
        self.audio.dragging = false;
        self.release_buttons(cx);
    }

    pub(super) fn toggle_audio(&mut self, cx: &mut Context<Self>) {
        if self.audio.output.is_none() {
            match eidoryn_psx::audio::output::AudioOutput::open(self.audio.buffer.clone()) {
                Ok(output) => {
                    self.audio.output = Some(output);
                    self.notice = None;
                    self.audio.muted = false;
                }
                Err(error) => {
                    self.notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            self.audio.muted = !self.audio.muted;
        }
        self.audio.buffer.set_volume(if self.audio.muted {
            0
        } else {
            self.audio.volume
        });
        self.preferences.muted = self.audio.muted;
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        if matches!(command, Command::Toggle)
            && (self.rewind_sources != 0 || self.snapshot.rewinding)
        {
            self.stop_rewind(cx);
            return;
        }
        let input_only = matches!(command, Command::SetButtons(_) | Command::SetTurbo(_));
        if !input_only && !matches!(command, Command::SetRewinding(_)) {
            self.stop_rewind(cx);
        }
        if matches!(
            command,
            Command::Reset
                | Command::Demo
                | Command::Load { .. }
                | Command::LoadDisc { .. }
                | Command::ReopenDisc { .. }
                | Command::LaunchLibraryDisc { .. }
                | Command::RequestClose
        ) || matches!(command, Command::Toggle) && self.snapshot.running
        {
            self.release_buttons(cx);
        }
        let input = if matches!(command, Command::Toggle) && !self.snapshot.running {
            // A restored checkpoint may hold buttons that are no longer physically down.
            self.runner.send(Command::SetButtons(self.pressed_buttons))
        } else {
            Ok(())
        };
        let notice = input.and_then(|()| self.runner.send(command)).err();
        let notice_changed = notice != self.notice;
        self.notice = notice;
        // The pad display comes from the next worker snapshot. Invalidating
        // here redraws the previous video frame on every attack press/release.
        if !input_only || notice_changed {
            cx.notify();
        }
    }
    pub(super) fn release_buttons(&mut self, cx: &mut Context<Self>) {
        if self.pressed_buttons != 0 {
            self.pressed_buttons = 0;
            self.notice = self.runner.send(Command::SetButtons(0)).err();
            cx.notify();
        }
    }
    pub(super) fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "z"
            || !event.keystroke.modifiers.control
            || matches!(event.keystroke.key.as_str(), "control" | "ctrl")
        {
            self.hold_rewind(super::rewind::RewindSource::Keyboard, false, cx);
        }
        if let Some(button) = self.preferences.keybindings.button(&event.keystroke.key)
            && self.pressed_buttons & button.mask() != 0
        {
            self.pressed_buttons &= !button.mask();
            self.command(Command::SetButtons(self.pressed_buttons), cx);
        }
    }
    pub(super) fn pick_image(&mut self, kind: ImagePicker, cx: &mut Context<Self>) {
        let prompt = self.tr(match kind {
            ImagePicker::Bios => Text::BiosPicker,
            ImagePicker::Exe => Text::ExePicker,
        });
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        let error_title = self.tr(Text::FilePickerError);
        let task = cx.spawn(async move |this, cx| {
            let result = match picker.await {
                Ok(Ok(paths)) => Ok(paths.and_then(|paths| paths.into_iter().next())),
                Ok(Err(error)) => Err(format!("{error_title}: {error}")),
                Err(error) => Err(format!("{error_title}: {error}")),
            };
            let result = match (kind, result) {
                (ImagePicker::Bios, Ok(Some(path))) => {
                    cx.background_executor()
                        .spawn(async move {
                            eidoryn_psx::systems::ps1::media::validate_bios_path(&path).map(Some)
                        })
                        .await
                }
                (_, result) => result,
            };
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Some(path)) => match kind {
                        ImagePicker::Bios => {
                            view.bios = Some(path);
                            view.notice = None;
                        }
                        ImagePicker::Exe => view.command(Command::Load { path, bios: false }, cx),
                    },
                    Ok(None) => return,
                    Err(error) => view.notice = Some(error),
                }
                cx.notify();
            });
        });
        match kind {
            // A newer selection cancels the pending picker or validation before it can update the UI.
            ImagePicker::Bios => self.bios_selection = Some(task),
            ImagePicker::Exe => task.detach(),
        }
    }

    pub(super) fn open_disc(&mut self, fast_boot: bool, cx: &mut Context<Self>) {
        if !self.validate_disc_bios(fast_boot, cx) {
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if fast_boot {
                    self.tr(Text::FastPicker)
                } else {
                    self.tr(Text::DiscPicker)
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            let result = picker.await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            view.load_selected_disc(path, fast_boot, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        view.notice = Some(format!("{}: {error}", view.tr(Text::FilePickerError)));
                    }
                    Err(error) => {
                        view.notice = Some(format!("{}: {error}", view.tr(Text::FilePickerError)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held {
            return;
        }
        let key = event.keystroke.key.as_str();
        if self.renaming_state.is_some() {
            return self.rename_key(event, window, cx);
        }
        if self.snapshot_confirmation.is_some() {
            return self.snapshot_confirmation_key(key, window, cx);
        }
        if self.snapshot_menu.is_some() {
            return self.snapshot_menu_key(event, window, cx);
        }
        if key == "tab" {
            self.release_buttons(cx);
            self.move_focus(event.keystroke.modifiers.shift, window, cx);
            cx.stop_propagation();
            return;
        }
        if key == "f12" {
            self.toggle_clean_fullscreen(cx);
            if !self.graphics.open {
                window.focus(&self.focus, cx);
            }
            return;
        }
        if self.graphics.open {
            self.settings_key(key, window, cx);
            return;
        }
        if self.page == WorkspacePage::Library && self.library_input_focused(window, cx) {
            if key == "escape" {
                window.focus(&self.focus, cx);
            }
            return;
        }
        if key == "z" && event.keystroke.modifiers.control {
            self.hold_rewind(super::rewind::RewindSource::Keyboard, true, cx);
            cx.stop_propagation();
            return;
        }
        match key {
            "f11" => return self.toggle_fullscreen(window, cx),
            "escape" if window.is_fullscreen() => return self.toggle_view(window, cx),
            "escape" if self.presentation.windowed_video => {
                return self.show_page(self.page, window, cx);
            }
            _ => {}
        }
        if !self.focus.is_focused(window) {
            match key {
                "escape" => {
                    window.focus(&self.focus, cx);
                    cx.notify();
                    return;
                }
                "left" | "up" => {
                    self.move_focus(true, window, cx);
                    return;
                }
                "right" | "down" => {
                    self.move_focus(false, window, cx);
                    return;
                }
                "f5" | "f6" | "f7" | "f8" | "f9" | "f10" => {}
                "r" | "s" if event.keystroke.modifiers.control => {}
                "l" if event.keystroke.modifiers.control && event.keystroke.modifiers.shift => {}
                _ => return, // Enter/Space activate the focused control on key-up.
            }
        }
        let modifiers = &event.keystroke.modifiers;
        if !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
            && let Some(button) = self.preferences.keybindings.button(&event.keystroke.key)
        {
            self.pressed_buttons |= button.mask();
            self.command(Command::SetButtons(self.pressed_buttons), cx);
            return;
        }
        match event.keystroke.key.as_str() {
            "f5" => self.command(Command::Toggle, cx),
            "f10" => self.command(Command::Step, cx),
            "f9" => self.command(Command::Slice, cx),
            "f6" => self.toggle_view(window, cx),
            "f7" => self.toggle_crt(cx),
            "f8" => {
                self.open_settings(cx);
            }
            "r" if event.keystroke.modifiers.control => self.command(Command::Reset, cx),
            "s" if modifiers.control && modifiers.shift => self.command(Command::QuickSave, cx),
            "s" if modifiers.control => self.command(Command::SaveState, cx),
            "l" if modifiers.control && modifiers.shift => self.command(Command::QuickLoad, cx),
            _ => {}
        }
    }
    fn settings_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            "escape" if self.rewind_disable_origin.is_some() => {
                self.dismiss_rewind_confirmation(window, cx);
            }
            "escape" | "f8" => {
                self.close_settings(window, cx);
            }
            "left" | "up" => self.move_focus(true, window, cx),
            "right" | "down" => self.move_focus(false, window, cx),
            _ => {}
        }
    }
    pub(super) fn toggle_fullscreen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            match self.presentation.fullscreen {
                FullscreenView::Workspace { return_to } => {
                    self.presentation.fullscreen = FullscreenView::VideoOverWorkspace { return_to };
                }
                FullscreenView::VideoOverWorkspace { return_to } => {
                    self.presentation.fullscreen = FullscreenView::Workspace { return_to };
                }
                FullscreenView::Video => window.toggle_fullscreen(),
            }
        } else {
            self.presentation.fullscreen = FullscreenView::Video;
            window.toggle_fullscreen();
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn toggle_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.snapshot_confirmation = None;
        self.stop_rewind(cx);
        if !window.is_fullscreen() {
            self.presentation.fullscreen = FullscreenView::Workspace {
                return_to: self.page,
            };
        } else if matches!(
            self.presentation.fullscreen,
            FullscreenView::VideoOverWorkspace { .. }
        ) {
            self.toggle_fullscreen(window, cx);
            return;
        }
        window.toggle_fullscreen();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn show_page(
        &mut self,
        page: WorkspacePage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snapshot_confirmation = None;
        self.stop_rewind(cx);
        if window.is_fullscreen() && self.presentation.shows_video() {
            window.toggle_fullscreen();
        }
        self.presentation.windowed_video = false;
        self.page = page;
        if page == WorkspacePage::Library {
            self.release_buttons(cx);
            self.enter_library(cx);
        }
        if page == WorkspacePage::SaveStates {
            self.release_buttons(cx);
            self.command(Command::RefreshSaveStates, cx);
        }
        self.rebinding = None;
        self.keybindings_draft = None;
        self.rewind_disable_origin = None;
        self.graphics.open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.snapshot_confirmation = None;
        self.stop_rewind(cx);
        self.release_buttons(cx);
        self.rebinding = None;
        self.keybindings_draft = None;
        self.graphics.open = true;
        self.settings_tab = SettingsTab::Graphics;
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebinding = None;
        self.keybindings_draft = None;
        self.rewind_disable_origin = None;
        self.graphics.open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn move_focus(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self
            .renaming_state
            .as_ref()
            .map(|state| &state.focus)
            .or_else(|| self.graphics.open.then_some(&self.settings_focus));
        let mut first = None;
        loop {
            if backwards {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            let Some(scope) = scope else { break };
            if scope.contains_focused(window, cx) {
                break;
            }
            // A large snapshot history may put hundreds of tab stops behind the modal.
            let Some(focused) = window.focused(cx) else {
                break;
            };
            if first.as_ref() == Some(&focused) {
                window.focus(scope, cx);
                break;
            }
            first.get_or_insert(focused);
        }
        cx.notify();
    }

    pub(super) fn toggle_clean_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.clean_fullscreen = !self.clean_fullscreen;
        self.preferences.clean_fullscreen = self.clean_fullscreen;
        self.save_preferences();
        cx.notify();
    }
}
