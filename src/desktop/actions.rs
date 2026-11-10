use super::Desktop;
use eidoryn_psx::runner::Command;
use gpui::{Context, KeyDownEvent, KeyUpEvent, PathPromptOptions, Window};

impl Desktop {
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
        cx.notify();
    }

    pub(super) fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        let input_only = matches!(command, Command::SetButtons(_));
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
        if let Some(button) = pad_button(&event.keystroke.key)
            && self.pressed_buttons & button != 0
        {
            self.pressed_buttons &= !button;
            self.command(Command::SetButtons(self.pressed_buttons), cx);
        }
    }
    pub(super) fn open(bios: bool, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if bios {
                    "Select BIOS firmware (.bin/.rom, exactly 512 KiB)"
                } else {
                    "Load PS-X EXE"
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
                    Ok(Err(error)) => view.notice = Some(format!("File picker: {error}")),
                    Err(error) => view.notice = Some(format!("File picker: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn open_disc(&mut self, fast_boot: bool, cx: &mut Context<Self>) {
        if !fast_boot && self.bios.is_none() {
            self.notice = Some("Select a 512 KiB firmware file with Open BIOS first. Homebrew can use Fast boot without BIOS.".into());
            cx.notify();
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                if fast_boot {
                    "Homebrew CUE/BIN or ISO: extract EXE and skip BIOS initialization"
                } else {
                    "Open game disc (CUE for multiple BIN tracks, or ISO)"
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
                    Ok(Err(error)) => view.notice = Some(format!("File picker: {error}")),
                    Err(error) => view.notice = Some(format!("File picker: {error}")),
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
        let modifiers = &event.keystroke.modifiers;
        if !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
            && let Some(button) = pad_button(&event.keystroke.key)
        {
            self.pressed_buttons |= button;
            self.command(Command::SetButtons(self.pressed_buttons), cx);
            return;
        }
        match event.keystroke.key.as_str() {
            "space" => self.command(Command::Toggle, cx),
            "f10" => self.command(Command::Step, cx),
            "f9" => self.command(Command::Slice, cx),
            "f6" => self.toggle_view(window, cx),
            "f7" => self.toggle_crt(cx),
            "f8" => {
                self.graphics.open = !self.graphics.open;
                self.appearance_open = false;
                cx.notify();
            }
            "f11" => {
                window.toggle_fullscreen();
                cx.notify();
            }
            "escape" if window.is_fullscreen() => {
                window.toggle_fullscreen();
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
        cx.notify();
    }
}

fn pad_button(key: &str) -> Option<u16> {
    let bit = match key {
        "backspace" => 0, // Select
        "enter" => 3,     // Start
        "up" => 4,
        "right" => 5,
        "down" => 6,
        "left" => 7,
        "a" => 8,
        "f" => 9,
        "q" => 10,
        "e" => 11, // L2, R2, L1, R1
        "s" => 12,
        "d" => 13,
        "x" => 14,
        "z" => 15, // Triangle, Circle, Cross, Square
        _ => return None,
    };
    Some(1 << bit)
}
