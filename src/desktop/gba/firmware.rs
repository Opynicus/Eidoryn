use super::{Desktop, GamePlatform, WorkspacePage};
use eidoryn_psx::{i18n::Text, preferences::validate_gba_bios_choice};
use gpui::{Context, PathPromptOptions};
use std::path::PathBuf;

impl Desktop {
    fn gba_launch_target(&self) -> Option<(u64, Option<i64>, Option<i64>)> {
        (self.page == WorkspacePage::Library).then(|| self.library.launch_target())
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
        let Some(current) = self.gba.bios.clone() else {
            self.notice = Some(self.tr(Text::GbaBiosRequired).into());
            self.pick_gba_bios(Some((path, title)), cx);
            cx.notify();
            return;
        };
        let target = self.gba_launch_target();
        self.bios_selection = Some(cx.spawn(async move |this, cx| {
            let (choice, validation) = cx
                .background_executor()
                .spawn({
                    let mut choice = Some(current.clone());
                    async move {
                        let validation = validate_gba_bios_choice(&mut choice);
                        (choice, validation)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.is_gba() || view.closing() || view.gba_launch_target() != target {
                    return;
                }
                match validation {
                    Ok(Some(bios)) => view.start_gba(path, title, bios, cx),
                    Err(error) => {
                        view.notice = Some(error);
                        if choice.is_none() {
                            view.clear_missing_gba_bios(&current);
                        }
                        view.pick_gba_bios(Some((path, title)), cx);
                    }
                    Ok(None) => return,
                }
                cx.notify();
            });
        }));
    }

    pub(in crate::desktop) fn pick_gba_bios(
        &mut self,
        pending: Option<(PathBuf, String)>,
        cx: &mut Context<Self>,
    ) {
        let target = self.gba_launch_target();
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr(Text::GbaBiosPicker).into()),
        });
        self.bios_selection = Some(cx.spawn(async move |this, cx| {
            let result = match picker.await {
                Ok(Ok(paths)) => {
                    let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                        return;
                    };
                    cx.background_executor()
                        .spawn(async move { validate_gba_bios_choice(&mut Some(path)) })
                        .await
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |view, cx| {
                if !view.is_gba()
                    || view.closing()
                    || (pending.is_some() && view.gba_launch_target() != target)
                {
                    return;
                }
                match result {
                    Ok(Some(bios)) => {
                        view.gba.bios = Some(bios.clone());
                        view.notice = None;
                        if let Some((path, title)) = pending {
                            view.start_gba(path, title, bios, cx);
                        }
                    }
                    Ok(None) => return,
                    Err(error) => view.notice = Some(error),
                }
                cx.notify();
            });
        }));
    }

    pub(in crate::desktop) fn remember_gba_bios(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.gba.bios.clone() else {
            return;
        };
        let mut choice = Some(current.clone());
        match validate_gba_bios_choice(&mut choice) {
            Ok(Some(bios)) => {
                self.preferences.gba_bios_path = Some(bios);
                self.save_preferences();
            }
            Ok(None) => return,
            Err(error) => {
                self.notice = Some(error);
                if choice.is_none() {
                    self.clear_missing_gba_bios(&current);
                }
            }
        }
        cx.notify();
    }

    fn clear_missing_gba_bios(&mut self, original: &std::path::Path) {
        self.gba.bios = None;
        if self.preferences.gba_bios_path.as_deref() == Some(original) {
            self.preferences.gba_bios_path = None;
            self.save_preferences();
        }
    }
}
