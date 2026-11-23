use super::{Desktop, GamePlatform, WorkspacePage};
use crate::desktop::components::{button, card, ghost_button};
use eidoryn_psx::{
    i18n::Text,
    library::Library,
    systems::gba::{import_firmware, needs_firmware_import},
};
use gpui::{Context, Div, PathPromptOptions, div, prelude::*, rgb};
use std::path::PathBuf;

type LibraryTarget = (u64, Option<i64>, Option<i64>);

/// The cartridge waiting for the first BIOS import.
pub(super) struct Setup {
    path: PathBuf,
    title: String,
    data_dir: PathBuf,
    target: Option<LibraryTarget>,
    error: Option<String>,
}

impl Desktop {
    fn gba_launch_target(&self) -> Option<LibraryTarget> {
        (self.page == WorkspacePage::Library).then(|| self.library.launch_target())
    }

    pub(in crate::desktop) fn cancel_gba_setup(&mut self) {
        self.bios_selection.take();
        self.gba.firmware_setup = None;
    }

    pub(in crate::desktop) fn open_gba(
        &mut self,
        path: PathBuf,
        title: String,
        cx: &mut Context<Self>,
    ) {
        self.switch_core(GamePlatform::Gba, cx);
        if !self.is_gba() || self.closing() {
            return;
        }
        self.cancel_gba_setup();
        self.notice = None;
        let data_dir = self
            .library
            .store
            .as_ref()
            .map(|s| s.base_dir().to_owned())
            .or_else(|| Library::default_path().and_then(|p| p.parent().map(ToOwned::to_owned)));
        let Some(data_dir) = data_dir else {
            self.notice = Some("Cannot locate the GBA data directory".into());
            cx.notify();
            return;
        };
        let legacy = self.preferences.gba_bios_path.clone();
        let target = self.gba_launch_target();
        self.bios_selection = Some(cx.spawn(async move |this, cx| {
            let (data_dir, needs_import) = cx
                .background_executor()
                .spawn(async move {
                    let needs_import = needs_firmware_import(&data_dir, legacy.as_deref());
                    (data_dir, needs_import)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if !v.is_gba() || v.closing() || target != v.gba_launch_target() {
                    return;
                }
                match needs_import {
                    Ok(false) => v.start_gba(path, title, data_dir, cx),
                    Ok(true) => {
                        v.gba.firmware_setup = Some(Setup {
                            path,
                            title,
                            data_dir,
                            target,
                            error: None,
                        });
                    }
                    Err(error) => v.notice = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn current_gba_setup(&self) -> Option<&Setup> {
        self.gba.firmware_setup.as_ref().filter(|setup| {
            self.is_gba() && !self.closing() && setup.target == self.gba_launch_target()
        })
    }

    pub(in crate::desktop) fn gba_setup_panel(&self, cx: &Context<Self>) -> Option<Div> {
        let setup = self.current_gba_setup()?;
        let p = &self.workspace_palette();
        Some(
            card(p)
                .debug_selector(|| "gba-firmware-setup".into())
                .flex_shrink_0()
                .p_4()
                .gap_2()
                .child(div().text_sm().child(self.tr(Text::GbaBiosSetup)))
                .when_some(setup.error.as_ref(), |panel, error| {
                    panel.child(
                        div()
                            .debug_selector(|| "gba-import-error".into())
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_sm()
                            .bg(rgb(p.error))
                            .text_color(rgb(p.on_error))
                            .child(error.clone()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            button(
                                &self.fonts,
                                p,
                                "import-gba-firmware",
                                self.tr(Text::GbaBiosImport),
                                true,
                            )
                            .role(gpui::Role::Button)
                            .aria_label(self.tr(Text::GbaBiosImport))
                            .debug_selector(|| "import-gba-firmware".into())
                            .on_click(cx.listener(|v, _, _, cx| v.pick_gba_firmware(cx))),
                        )
                        .child(
                            button(
                                &self.fonts,
                                p,
                                "cancel-gba-firmware",
                                self.tr(Text::LibraryCancel),
                                false,
                            )
                            .role(gpui::Role::Button)
                            .aria_label(self.tr(Text::LibraryCancel))
                            .debug_selector(|| "cancel-gba-firmware".into())
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.cancel_gba_setup();
                                cx.notify();
                            })),
                        )
                        .child(
                            ghost_button(
                                &self.fonts,
                                p,
                                "download-mgba",
                                self.tr(Text::DownloadMgba),
                                None,
                            )
                            .role(gpui::Role::Link)
                            .aria_label(self.tr(Text::DownloadMgba))
                            .debug_selector(|| "download-mgba".into())
                            .on_click(|_, _, cx| {
                                cx.open_url("https://mgba.io/downloads.html");
                            }),
                        ),
                ),
        )
    }

    fn pick_gba_firmware(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.current_gba_setup() else {
            return;
        };
        let data_dir = setup.data_dir.clone();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr(Text::GbaBiosImport).into()),
        });
        self.bios_selection = Some(cx.spawn(async move |this, cx| {
            let result = match picker.await {
                Ok(Ok(paths)) => {
                    let Some(source) = paths.and_then(|paths| paths.into_iter().next()) else {
                        return;
                    };
                    if !this
                        .read_with(cx, |v, _| v.current_gba_setup().is_some())
                        .unwrap_or(false)
                    {
                        return;
                    }
                    cx.background_executor()
                        .spawn(async move { import_firmware(&data_dir, &source) })
                        .await
                        .map_err(|e| e.to_string())
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |v, cx| {
                if v.current_gba_setup().is_none() {
                    return;
                }
                match result {
                    Ok(()) => {
                        if let Some(setup) = v.gba.firmware_setup.take() {
                            v.start_gba(setup.path, setup.title, setup.data_dir, cx);
                        }
                    }
                    Err(error) => {
                        let message = format!("{} {error}", v.tr(Text::GbaBiosImportFailed));
                        if let Some(setup) = &mut v.gba.firmware_setup {
                            setup.error = Some(message);
                        }
                    }
                }
                cx.notify();
            });
        }));
    }
}
