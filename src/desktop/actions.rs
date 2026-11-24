use super::shortcuts::{ShortcutEditor, shortcut_keystroke};
use super::{Desktop, FullscreenView, SettingsTab, WorkspacePage};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::shortcuts::ShortcutAction;
use eidoryn_psx::systems::ps1::runner::Command;
use gpui::{Context, KeyDownEvent, KeyUpEvent, PathPromptOptions, Window};

#[derive(Clone, Copy)]
pub(super) enum ImagePicker {
    Bios,
    Exe,
    Gba,
}

impl Desktop {
    pub(super) fn stop_input(&mut self, cx: &mut Context<Self>) {
        self.stop_rewind(cx);
        self.navigation.cancel_drag();
        let recording_shortcut = self.shortcut_editor.recording.take();
        if self.rebinding.take().is_some() || recording_shortcut.is_some() {
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
        if self.closing() {
            return;
        }
        if self.is_gba() && !matches!(command, Command::RequestClose) {
            use eidoryn_psx::systems::gba::Command as Gba;
            let action = match command {
                Command::SaveState => Gba::SaveState,
                Command::QuickSave => Gba::QuickSave,
                Command::QuickSaveIfUnchanged { revision, slot } => {
                    Gba::QuickSaveIfUnchanged { revision, slot }
                }
                Command::QuickLoad => Gba::QuickLoad,
                Command::LoadState { path } => Gba::LoadState { path },
                Command::RenameState { path, name } => Gba::RenameState { path, name },
                Command::DeleteState { path, modified } => Gba::DeleteState { path, modified },
                Command::RefreshSaveStates => Gba::RefreshSaveStates,
                Command::SetRewinding(held) => Gba::SetRewinding(held),
                _ => return,
            };
            if !matches!(action, Gba::SetRewinding(_)) {
                self.stop_rewind(cx);
            }
            self.notice = None;
            self.gba_command(action, cx);
            cx.notify();
            return;
        }
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
            if self.is_gba() {
                self.gba_command(eidoryn_psx::systems::gba::Command::Buttons(0), cx);
            } else {
                self.notice = self.runner.send(Command::SetButtons(0)).err();
            }
            cx.notify();
        }
    }
    pub(super) fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.rewind_sources != 0 {
            let stroke = shortcut_keystroke(&event.keystroke, cx);
            self.release_rewind_key(Some(&stroke.key), stroke.modifiers, cx);
        }
        if self.is_gba() {
            self.gba_key(&event.keystroke.key, false, cx);
            return;
        }
        if let Some(button) = self.preferences.keybindings.button(&event.keystroke.key)
            && self.pressed_buttons & button.mask() != 0
        {
            self.pressed_buttons &= !button.mask();
            self.command(Command::SetButtons(self.pressed_buttons), cx);
        }
    }
    pub(super) fn release_rewind_key(
        &mut self,
        released: Option<&str>,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        use super::rewind::RewindSource;
        if self.rewind_sources & RewindSource::Keyboard as u8 != 0
            && self
                .preferences
                .shortcuts
                .binding(ShortcutAction::Rewind)
                .is_none_or(|chord| {
                    released.is_some_and(|key| chord.matches_key(key))
                        || !chord.modifiers.is_subset_of(&modifiers)
                })
        {
            self.hold_rewind(RewindSource::Keyboard, false, cx);
        }
    }
    pub(super) fn pick_image(&mut self, kind: ImagePicker, cx: &mut Context<Self>) {
        let prompt = self.tr(match kind {
            ImagePicker::Bios => Text::BiosPicker,
            ImagePicker::Exe => Text::ExePicker,
            ImagePicker::Gba => Text::OpenCartridge,
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
                        ImagePicker::Gba if view.is_gba() => {
                            let title = path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            view.open_gba(path, title, cx);
                        }
                        ImagePicker::Gba => {}
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
            ImagePicker::Bios | ImagePicker::Gba => self.image_picker = Some(task),
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
        if self.library_dialog_key(key, cx) {
            return;
        }
        if !self.graphics.open
            && self.page == WorkspacePage::Library
            && self.library_input_focused(window, cx)
        {
            if key == "escape" {
                window.focus(&self.focus, cx);
            }
            return;
        }
        let modifiers = event.keystroke.modifiers;
        // A focused control owns its ordinary navigation and activation keys.
        let control_key = !self.focus.is_focused(window)
            && !modifiers.modified()
            && matches!(
                key,
                "left" | "right" | "up" | "down" | "enter" | "space" | "home" | "end"
            );
        let action = if control_key {
            None
        } else {
            self.preferences
                .shortcuts
                .action_for(&shortcut_keystroke(&event.keystroke, cx))
        };
        if self.graphics.open {
            match action {
                Some(ShortcutAction::Settings) => self.close_settings(window, cx),
                Some(ShortcutAction::CleanFullscreen) => self.toggle_clean_fullscreen(cx),
                _ => self.settings_key(key, window, cx),
            }
            return;
        }
        // Preserve the controller binding in profiles created when these shortcuts
        // only applied to PS1. The editor prevents new conflicts on either core.
        let gba_button = self.accepts_gba_button(key, modifiers);
        if let Some(action) = action.filter(|a| !(gba_button && a.is_checkpoint())) {
            self.run_shortcut(action, window, cx);
            cx.stop_propagation();
            return;
        }
        match key {
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
                _ => return, // Enter/Space activate the focused control on key-up.
            }
        }
        if self.is_gba() {
            if gba_button {
                self.gba_key(key, true, cx);
            }
            return;
        }
        if !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
            && let Some(button) = self.preferences.keybindings.button(&event.keystroke.key)
        {
            self.pressed_buttons |= button.mask();
            self.command(Command::SetButtons(self.pressed_buttons), cx);
        }
    }

    fn run_shortcut(
        &mut self,
        action: ShortcutAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ShortcutAction::Toggle => self.toggle_playback(cx),
            ShortcutAction::Step => {
                self.step_debugger(super::inspector::DebugStep::Instruction, cx);
            }
            ShortcutAction::Slice => self.step_debugger(super::inspector::DebugStep::Slice, cx),
            ShortcutAction::Enlarge => self.toggle_view(window, cx),
            ShortcutAction::Crt => self.toggle_crt(cx),
            ShortcutAction::Settings => self.open_settings(cx),
            ShortcutAction::Fullscreen => self.toggle_fullscreen(window, cx),
            ShortcutAction::CleanFullscreen => {
                self.toggle_clean_fullscreen(cx);
                window.focus(&self.focus, cx);
            }
            ShortcutAction::Reset => self.reset_playback(cx),
            ShortcutAction::Snapshot => self.command(Command::SaveState, cx),
            ShortcutAction::QuickSave => self.command(Command::QuickSave, cx),
            ShortcutAction::QuickLoad => self.command(Command::QuickLoad, cx),
            ShortcutAction::Rewind => {
                self.hold_rewind(super::rewind::RewindSource::Keyboard, true, cx);
            }
        }
    }
    fn settings_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            "escape" if self.rewind_disable_origin.is_some() => {
                self.dismiss_rewind_confirmation(window, cx);
            }
            "escape" => {
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
        self.dismiss_library_context_menu();
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
        if self.is_gba() && self.page != page {
            self.image_picker.take();
            self.release_buttons(cx);
        }
        self.snapshot_confirmation = None;
        self.dismiss_library_context_menu();
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
        self.discard_binding_drafts();
        self.rewind_disable_origin = None;
        self.graphics.open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn discard_binding_drafts(&mut self) {
        self.rebinding = None;
        self.keybindings_draft = None;
        self.gba.keybindings_draft = None;
        self.shortcut_editor = ShortcutEditor::default();
    }

    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.snapshot_confirmation = None;
        self.dismiss_library_context_menu();
        self.stop_rewind(cx);
        self.release_buttons(cx);
        self.discard_binding_drafts();
        self.graphics.open = true;
        self.settings_tab = SettingsTab::Graphics;
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_binding_drafts();
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
            .or_else(|| self.graphics.open.then_some(&self.settings_focus))
            .or_else(|| {
                self.library_dialog_focus
                    .open
                    .then_some(&self.library_dialog_focus.scope)
            });
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
