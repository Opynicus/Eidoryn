use super::{
    Desktop,
    components::{button, label},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::{
    systems::ps1::{
        cpu_engine::CpuMode,
        gpu::{RendererAvailability, RendererMode, hardware_backends},
        runner::{Command, Snapshot},
    },
    theme::Palette,
    video_filter::CrtFilter,
};
use gpui::{Context, div, prelude::*, px, rgb};

pub(super) struct GraphicsControls {
    pub open: bool,
    software: RendererMode,
    hardware: RendererMode,
    backends: Option<Vec<RendererAvailability>>,
    detecting: bool,
    pending: Option<RendererMode>,
    pending_revision: u64,
    pending_cpu: Option<CpuMode>,
}

impl GraphicsControls {
    pub(super) fn new(mode: RendererMode, open: bool) -> Self {
        Self {
            open,
            software: if mode == RendererMode::Threaded {
                mode
            } else {
                RendererMode::Serial
            },
            hardware: if mode.is_hardware() {
                mode
            } else {
                RendererMode::Hardware
            },
            backends: None,
            detecting: false,
            pending: None,
            pending_revision: 0,
            pending_cpu: None,
        }
    }

    fn available(&self, mode: RendererMode) -> bool {
        self.backends.as_ref().is_some_and(|backends| {
            backends.iter().any(|backend| {
                (mode == RendererMode::Hardware || backend.mode == mode) && backend.device.is_some()
            })
        })
    }

    #[cfg(test)]
    pub(super) fn with_backends(mut self, backends: Vec<RendererAvailability>) -> Self {
        self.backends = Some(backends);
        self
    }

    pub(super) fn receive(&mut self, snapshot: &Snapshot) -> Option<RendererMode> {
        let completed = snapshot.renderer_revision != self.pending_revision;
        let confirmed = self
            .pending
            .filter(|mode| completed && *mode == snapshot.renderer && snapshot.message.is_none());
        if self.pending_cpu == Some(snapshot.cpu_mode) || snapshot.message.is_some() {
            self.pending_cpu = None;
        }
        if completed {
            self.pending = None;
        }
        if matches!(
            snapshot.renderer,
            RendererMode::Serial | RendererMode::Threaded
        ) {
            self.software = snapshot.renderer;
        } else if snapshot.message.is_none() {
            self.hardware = snapshot.renderer;
        }
        confirmed
    }
}

impl Desktop {
    pub(super) fn detect_hardware(&mut self, cx: &mut Context<Self>) {
        if self.graphics.backends.is_some() || self.graphics.detecting {
            return;
        }
        self.graphics.detecting = true;
        let detection = cx
            .background_executor()
            .spawn(async { hardware_backends() });
        cx.spawn(async move |this, cx| {
            let backends = detection.await;
            let _ = this.update(cx, |view, cx| {
                view.graphics.backends = Some(backends);
                view.graphics.detecting = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn hardware_control(&self, ready: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let active = self.snapshot.renderer.is_hardware();
        let available = ready && (active || self.graphics.available(self.graphics.hardware));
        switch(
            &self.fonts,
            &self.themes.active().palette,
            "hardware-switch",
            self.tr(Text::HardwareAcceleration),
            self.tr(Text::HardwareHint),
            active,
            available,
        )
        .when(available, |element| {
            element.on_click(cx.listener(|view, _, _, cx| {
                let next = if view.snapshot.renderer.is_hardware() {
                    view.graphics.software
                } else {
                    view.graphics.hardware
                };
                view.select_renderer(next, cx);
            }))
        })
    }

    fn backend_choices(&self, ready: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let modes = [
            RendererMode::Hardware,
            RendererMode::Dx12,
            RendererMode::Vulkan,
            RendererMode::OpenGl,
            RendererMode::Metal,
        ];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label(p, self.tr(Text::HardwareBackend)))
            .children(modes.into_iter().map(|mode| {
                let available = ready && self.graphics.available(mode);
                let selected = mode == self.graphics.hardware;
                let detail = if let Some(backends) = &self.graphics.backends {
                    if mode == RendererMode::Hardware {
                        if self.graphics.available(mode) {
                            self.tr(Text::AutomaticBackendHint).to_owned()
                        } else {
                            self.tr(Text::NoHardwareRenderer).to_owned()
                        }
                    } else {
                        backends
                            .iter()
                            .find(|backend| backend.mode == mode)
                            .and_then(|backend| backend.device.as_ref().or(backend.reason.as_ref()))
                            .cloned()
                            .unwrap_or_else(|| self.tr(Text::NoHardwareRenderer).to_owned())
                    }
                } else {
                    self.tr(Text::DetectingHardware).to_owned()
                };
                let title = if mode == RendererMode::Hardware {
                    self.tr(Text::AutomaticBackend)
                } else {
                    mode.label()
                };
                let palette = *p;
                div()
                    .id(gpui::SharedString::from(format!("backend-{}", mode.id())))
                    .debug_selector(move || format!("backend-{}", mode.id()))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(if selected { p.primary } else { p.border }))
                    .bg(rgb(if selected { p.tonal } else { p.surface }))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(!available, |row| row.opacity(0.45))
                    .when(available, |row| {
                        row.cursor_pointer()
                            .tab_index(0)
                            .focus(move |style| style.border_color(rgb(palette.primary)))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if view.snapshot.renderer.is_hardware() {
                                    view.select_renderer(mode, cx);
                                } else {
                                    view.graphics.hardware = mode;
                                    cx.notify();
                                }
                            }))
                    })
                    .child(div().text_sm().font(self.fonts.medium()).child(title))
                    .child(label(p, detail))
            }))
    }

    fn select_renderer(&mut self, mode: RendererMode, cx: &mut Context<Self>) {
        match self.runner.send(Command::SetRenderer(mode)) {
            Ok(()) => {
                self.graphics.pending = Some(mode);
                self.graphics.pending_revision = self.snapshot.renderer_revision;
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    fn jit_control(&self, ready: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let available = ready
            && cfg!(all(
                feature = "jit",
                target_arch = "x86_64",
                target_os = "linux"
            ));
        switch(
            &self.fonts,
            &self.themes.active().palette,
            "jit-switch",
            self.tr(Text::Jit),
            self.tr(Text::PlatformHint),
            self.snapshot.cpu_mode == CpuMode::Jit,
            available,
        )
        .when(available, |element| {
            element.on_click(cx.listener(|view, _, _, cx| {
                let mode = if view.snapshot.cpu_mode == CpuMode::Jit {
                    CpuMode::Interpreter
                } else {
                    CpuMode::Jit
                };
                match view.runner.send(Command::SetCpuMode(mode)) {
                    Ok(()) => {
                        view.graphics.pending_cpu = Some(mode);
                        view.notice = None;
                    }
                    Err(error) => view.notice = Some(error),
                }
                cx.notify();
            }))
        })
    }

    fn clean_fullscreen_control(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        switch(
            &self.fonts,
            &self.themes.active().palette,
            "clean-fullscreen",
            self.tr(Text::CleanFullscreen),
            self.tr(Text::CleanFullscreenHint),
            self.clean_fullscreen,
            true,
        )
        .on_click(cx.listener(|view, _, _, cx| view.toggle_clean_fullscreen(cx)))
    }

    fn crt_choices(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let fonts = &self.fonts;
        div().flex().gap_2().children(
            [
                (CrtFilter::Off, self.tr(Text::Off)),
                (CrtFilter::Soft, self.tr(Text::Soft)),
                (CrtFilter::Classic, self.tr(Text::Classic)),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (filter, name))| {
                button(fonts, p, ("crt-choice", index), name, false)
                    .when(self.crt_filter() == filter, |button| {
                        button
                            .bg(rgb(p.tonal))
                            .border_color(rgb(p.primary))
                            .text_color(rgb(p.on_tonal))
                    })
                    .on_click(cx.listener(move |view, _, _, cx| view.set_crt_filter(filter, cx)))
            }),
        )
    }

    pub(super) fn graphics_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let fonts = &self.fonts;
        let hardware = self.snapshot.renderer.is_hardware();
        let ready = self.graphics.pending.is_none() && self.graphics.pending_cpu.is_none();
        let software_threads = self.graphics.software == RendererMode::Threaded;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Crt)),
            )
            .child(label(p, self.tr(Text::CrtHint)))
            .child(self.crt_choices(cx))
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .font(self.fonts.semibold())
                    .child(self.tr(Text::Performance)),
            )
            .child(label(p, self.tr(Text::PerformanceHint)))
            .child(self.hardware_control(ready, cx))
            .child(self.backend_choices(ready, cx))
            .child(
                switch(
                    fonts,
                    p,
                    "threaded-switch",
                    self.tr(Text::Threaded),
                    self.tr(Text::ThreadedHint),
                    software_threads,
                    ready && !hardware,
                )
                .when(ready && !hardware, |element| {
                    element.on_click(cx.listener(|view, _, _, cx| {
                        let mode = if view.graphics.software == RendererMode::Threaded {
                            RendererMode::Serial
                        } else {
                            RendererMode::Threaded
                        };
                        view.select_renderer(mode, cx);
                    }))
                }),
            )
            .child(self.jit_control(ready, cx))
            .child(self.clean_fullscreen_control(cx))
            .child(label(
                p,
                if !ready {
                    self.tr(Text::Applying).into()
                } else if hardware {
                    format!(
                        "{} · {}",
                        self.snapshot.renderer_device,
                        self.tr(Text::GpuThread)
                    )
                } else if self.graphics.backends.is_none() {
                    self.tr(Text::DetectingHardware).into()
                } else if self.graphics.available(RendererMode::Hardware) {
                    self.tr(Text::SoftwareHint).into()
                } else {
                    self.tr(Text::NoHardwareRenderer).into()
                },
            ))
    }
}

fn switch(
    fonts: &super::fonts::Fonts,
    p: &Palette,
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    enabled: bool,
    available: bool,
) -> gpui::Stateful<gpui::Div> {
    let palette = *p;
    div()
        .id(id)
        .py_3()
        .border_t_1()
        .border_color(rgb(p.border))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .when(available, gpui::Styled::cursor_pointer)
        .when(available, |element| {
            element
                .tab_index(0)
                .focus(move |style| style.bg(rgb(palette.tonal)))
        })
        .tooltip(move |_, cx| super::components::tooltip(palette, detail.into(), cx))
        .when(!available, |element| element.opacity(0.45))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().font(fonts.medium()).child(title))
                .child(label(p, detail)),
        )
        .child(
            div()
                .w(px(36.))
                .h(px(20.))
                .px(px(2.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(if enabled { p.primary } else { p.border }))
                .when(enabled, gpui::Styled::justify_end)
                .child(div().size(px(16.)).rounded_full().bg(rgb(if enabled {
                    p.on_primary
                } else {
                    p.text
                }))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eidoryn_psx::systems::ps1::machine::Machine;

    #[test]
    fn only_confirmed_renderer_changes_are_persisted() {
        let mut controls = GraphicsControls::new(RendererMode::Serial, false);
        controls.pending = Some(RendererMode::Threaded);
        let mut snapshot = Snapshot::capture(&Machine::demo(), true, None, 0.0);
        assert_eq!(controls.receive(&snapshot), None);
        assert_eq!(controls.pending, Some(RendererMode::Threaded));
        snapshot.message = Some("An older notification".into());
        assert_eq!(controls.receive(&snapshot), None);
        assert_eq!(controls.pending, Some(RendererMode::Threaded));
        snapshot.message = None;
        snapshot.renderer = RendererMode::Threaded;
        snapshot.renderer_revision = 1;
        assert_eq!(controls.receive(&snapshot), Some(RendererMode::Threaded));
        assert_eq!(controls.receive(&snapshot), None);
        controls.pending = Some(RendererMode::Vulkan);
        controls.pending_revision = snapshot.renderer_revision;
        snapshot.message = Some("Renderer unavailable".into());
        snapshot.renderer_revision = 2;
        assert_eq!(controls.receive(&snapshot), None);
        assert!(controls.pending.is_none());
    }

    #[test]
    fn hardware_changes_preserve_the_selected_software_renderer() {
        let mut controls = GraphicsControls::new(RendererMode::Threaded, false);
        let mut snapshot = Snapshot::capture(&Machine::demo(), true, None, 0.0);
        for mode in [
            RendererMode::Hardware,
            RendererMode::Dx12,
            RendererMode::OpenGl,
            RendererMode::Vulkan,
            RendererMode::Metal,
        ] {
            controls.pending = Some(mode);
            controls.pending_revision = snapshot.renderer_revision;
            snapshot.renderer = mode;
            snapshot.renderer_revision += 1;
            assert_eq!(controls.receive(&snapshot), Some(mode));
            assert_eq!(controls.software, RendererMode::Threaded);
            assert_eq!(controls.hardware, mode);
        }
        controls.pending = Some(RendererMode::OpenGl);
        controls.pending_revision = snapshot.renderer_revision;
        snapshot.renderer = RendererMode::Threaded;
        snapshot.message = Some("OpenGL unavailable".into());
        snapshot.renderer_revision += 1;
        assert_eq!(controls.receive(&snapshot), None);
        assert!(controls.pending.is_none());
        assert_eq!(controls.software, RendererMode::Threaded);
        assert_eq!(controls.hardware, RendererMode::Metal);
    }

    fn detected_backends() -> Vec<RendererAvailability> {
        vec![
            RendererAvailability {
                mode: RendererMode::Dx12,
                device: Some("Test DirectX 12 GPU".into()),
                reason: None,
            },
            RendererAvailability {
                mode: RendererMode::Vulkan,
                device: None,
                reason: Some("Driver does not support Vulkan".into()),
            },
        ]
    }

    #[test]
    fn hardware_is_disabled_until_detection_and_only_supported_backends_are_available() {
        let mut controls = GraphicsControls::new(RendererMode::Serial, false);
        assert_eq!(controls.hardware, RendererMode::Hardware);
        assert!(!controls.available(RendererMode::Hardware));
        controls = controls.with_backends(detected_backends());
        assert!(controls.available(RendererMode::Hardware));
        assert!(controls.available(RendererMode::Dx12));
        assert!(!controls.available(RendererMode::Vulkan));
        assert!(!controls.available(RendererMode::OpenGl));
    }

    #[cfg(feature = "ui-tests")]
    #[gpui::test]
    fn backend_selection_uses_detected_support_without_starting_hardware(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(super::super::tests::desktop);
        view.update(cx, |view, cx| {
            view.preferences_path = None;
            view.graphics = GraphicsControls::new(RendererMode::Threaded, true)
                .with_backends(detected_backends());
            cx.notify();
        });
        cx.simulate_resize(gpui::size(px(1280.), px(1200.)));
        cx.run_until_parked();
        for (selector, expected) in [
            ("backend-vulkan", RendererMode::Hardware),
            ("backend-dx12", RendererMode::Dx12),
            ("backend-opengl", RendererMode::Dx12),
        ] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
            cx.run_until_parked();
            view.read_with(cx, |view, _| {
                assert_eq!(view.graphics.hardware, expected);
                assert!(view.graphics.pending.is_none());
                assert_eq!(view.snapshot.renderer, RendererMode::Serial);
            });
        }
    }
}
