use super::{Desktop, SettingsTab, WorkspacePage};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::systems::ps1::runner::Command;
use gpui::{Context, KeyDownEvent, KeyUpEvent, PathPromptOptions, Window};

impl Desktop {
    pub(super) fn stop_input(&mut self, cx: &mut Context<Self>) {
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
        let input_only = matches!(command, Command::SetButtons(_) | Command::SetTurbo(_));
        if matches!(
            command,
            Command::Reset
                | Command::Demo
                | Command::Load { .. }
                | Command::LoadDisc { .. }
                | Command::RequestClose
        ) || matches!(command, Command::Toggle) && self.snapshot.running
        {
            self.release_buttons(cx);
        }
        let notice = self.runner.send(command).err();
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
        if let Some(button) = self.preferences.keybindings.button(&event.keystroke.key)
            && self.pressed_buttons & button.mask() != 0
        {
            self.pressed_buttons &= !button.mask();
            self.command(Command::SetButtons(self.pressed_buttons), cx);
        }
    }
    pub(super) fn open(&mut self, bios: bool, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if bios {
                    self.tr(Text::BiosPicker)
                } else {
                    self.tr(Text::ExePicker)
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
                            view.command(Command::Load { path, bios }, cx);
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
    pub(super) fn open_disc(&mut self, fast_boot: bool, cx: &mut Context<Self>) {
        if !fast_boot && self.bios.is_none() {
            self.notice = Some(self.tr(Text::BiosRequired).into());
            cx.notify();
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
                            view.command(
                                Command::LoadDisc {
                                    path,
                                    bios: view.bios.clone(),
                                    fast_boot,
                                },
                                cx,
                            );
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
            match event.keystroke.key.as_str() {
                "escape" | "f8" => {
                    self.close_settings(window, cx);
                }
                "left" | "up" => self.move_focus(true, window, cx),
                "right" | "down" => self.move_focus(false, window, cx),
                _ => {}
            }
            return;
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
                "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" => {}
                "r" if event.keystroke.modifiers.control => {}
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
                self.open_settings(false, cx);
            }
            "f11" => {
                window.toggle_fullscreen();
                window.focus(&self.focus, cx);
                cx.notify();
            }
            "escape" if window.is_fullscreen() => {
                window.toggle_fullscreen();
                window.focus(&self.focus, cx);
                cx.notify();
            }
            "escape" if self.game_view => {
                self.game_view = false;
                cx.notify();
            }
            "r" if event.keystroke.modifiers.control => self.command(Command::Reset, cx),
            _ => {}
        }
    }
    pub(super) fn toggle_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            window.toggle_fullscreen();
            self.game_view = false;
        } else {
            self.game_view = !self.game_view;
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn show_page(
        &mut self,
        page: WorkspacePage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.game_view = false;
        self.page = page;
        self.rebinding = None;
        self.keybindings_draft = None;
        self.graphics.open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn open_settings(&mut self, appearance: bool, cx: &mut Context<Self>) {
        self.release_buttons(cx);
        self.rebinding = None;
        self.keybindings_draft = None;
        self.graphics.open = true;
        self.settings_tab = if appearance {
            SettingsTab::Appearance
        } else {
            SettingsTab::Graphics
        };
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebinding = None;
        self.keybindings_draft = None;
        self.graphics.open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn move_focus(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        // The drawer is modal: skip background tab stops, including on wraparound.
        for _ in 0..128 {
            if backwards {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            if !self.graphics.open || self.settings_focus.contains_focused(window, cx) {
                break;
            }
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
