//! Cartridge save controls for the active GBA session.
use super::{Command, Desktop};
use crate::desktop::components::{button, card, heading, label, tooltip};
use eidoryn_psx::i18n::Text;
use gpui::{Context, PathPromptOptions, PromptButton, PromptLevel, Window, div, prelude::*};
use std::path::PathBuf;

impl Desktop {
    pub(in crate::desktop) fn gba_battery_card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.themes.active().palette;
        let path = self.gba.snapshot.battery_path.as_deref();
        card(&p)
            .debug_selector(|| "gba-battery".into())
            .p_4()
            .gap_2()
            .child(heading(
                &self.fonts,
                &p,
                self.tr(Text::CartridgeSave),
                ".sav",
            ))
            .when_some(path, |card, path| {
                let location = path.display().to_string();
                card.child(
                    label(&p, location.clone())
                        .truncate()
                        .id("gba-save-location")
                        .tooltip(move |_, cx| tooltip(p, location.clone().into(), cx)),
                )
            })
            .child(label(&p, self.tr(Text::GbaSaveHint)))
            .child(
                div().flex().flex_wrap().gap_2().children(
                    [
                        ("gba-sync-save", Text::SyncCartridgeSave),
                        ("gba-export-save", Text::ExportCartridgeSave),
                        ("gba-import-save", Text::ImportCartridgeSave),
                    ]
                    .map(|(id, text)| {
                        button(&self.fonts, &p, id, self.tr(text), false)
                            .debug_selector(move || id.into())
                            .when(path.is_none(), |b| {
                                b.opacity(0.45).tab_index(-1).cursor_default()
                            })
                            .when(path.is_some(), |b| {
                                b.on_click(cx.listener(move |v, _, window, cx| {
                                    v.stop_input(cx);
                                    if text == Text::SyncCartridgeSave {
                                        v.notice = None;
                                        v.gba_command(Command::SyncBattery, cx);
                                    } else {
                                        v.pick_battery_file(
                                            text == Text::ImportCartridgeSave,
                                            window,
                                            cx,
                                        );
                                    }
                                }))
                            })
                    }),
                ),
            )
    }

    fn pick_battery_file(&mut self, import: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rom) = &self.gba.snapshot.path else {
            return;
        };
        let revision = self.gba.snapshot.media_revision;
        let error_title = self.tr(Text::FilePickerError);
        let task = if import {
            let picker = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(self.tr(Text::ImportCartridgeSave).into()),
            });
            cx.spawn_in(window, async move |this, cx| {
                let path = match picker.await {
                    Ok(Ok(Some(paths))) => paths.into_iter().next(),
                    Ok(Ok(None)) => None,
                    Ok(Err(err)) => {
                        let _ = this.update(cx, |v, cx| {
                            v.notice = Some(format!("{error_title}: {err}"));
                            cx.notify();
                        });
                        return;
                    }
                    Err(_) => return,
                };
                let Some(path) = path else { return };
                let confirmation = this.update_in(cx, |v, window, cx| {
                    if !v.is_gba() || v.gba.snapshot.media_revision != revision || v.closing() {
                        return None;
                    }
                    Some(window.prompt(
                        PromptLevel::Warning,
                        v.tr(Text::ReplaceCartridgeSave),
                        Some(&format!(
                            "{}\n{}",
                            v.tr(Text::ImportCartridgeSaveHint),
                            path.display()
                        )),
                        &[
                            PromptButton::cancel(v.tr(Text::RenameCancel)),
                            PromptButton::ok(v.tr(Text::ImportCartridgeSave)),
                        ],
                        cx,
                    ))
                });
                if let Ok(Some(answer)) = confirmation
                    && answer.await == Ok(1)
                {
                    let _ = this.update(cx, |v, cx| {
                        v.transfer_battery_file(path, revision, true, cx);
                    });
                }
            })
        } else {
            let name = rom.with_extension("sav");
            let picker = cx.prompt_for_new_path(
                rom.parent().unwrap_or(rom),
                name.file_name().and_then(|s| s.to_str()),
            );
            cx.spawn(async move |this, cx| {
                let result = picker.await;
                let _ = this.update(cx, |v, cx| match result {
                    Ok(Ok(Some(path))) => v.transfer_battery_file(path, revision, false, cx),
                    Ok(Err(err)) => {
                        v.notice = Some(format!("{error_title}: {err}"));
                        cx.notify();
                    }
                    _ => {}
                });
            })
        };
        self.image_picker = Some(task);
    }

    fn transfer_battery_file(
        &mut self,
        path: PathBuf,
        media_revision: u64,
        import: bool,
        cx: &mut Context<Self>,
    ) {
        if self.is_gba() && self.gba.snapshot.media_revision == media_revision {
            self.notice = None;
            self.stop_input(cx);
            self.gba_command(
                if import {
                    Command::ImportBattery {
                        path,
                        media_revision,
                    }
                } else {
                    Command::ExportBattery {
                        path,
                        media_revision,
                    }
                },
                cx,
            );
            cx.notify();
        }
    }
}
