use super::{
    Desktop,
    components::{button, label},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::{
    systems::ps1::{
        cpu_engine::CpuMode,
        gpu::RendererMode,
        runner::{Command, Snapshot},
    },
    theme::Palette,
    video_filter::CrtFilter,
};
use gpui::{Context, div, prelude::*, px, rgb};

pub(super) struct GraphicsControls {
    pub open: bool,
    software: RendererMode,
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
            pending: None,
            pending_revision: 0,
            pending_cpu: None,
        }
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
        if snapshot.renderer != RendererMode::Vulkan {
            self.software = snapshot.renderer;
        }
        confirmed
    }
}

impl Desktop {
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
            &self.themes.active().palette,
            "clean-fullscreen",
            self.tr(Text::CleanFullscreen),
            self.tr(Text::CleanFullscreenHint),
            self.clean_fullscreen,
            true,
        )
        .on_click(cx.listener(|view, _, _, cx| view.toggle_clean_fullscreen(cx)))
    }

    pub(super) fn graphics_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let hardware = self.snapshot.renderer == RendererMode::Vulkan;
        let ready = self.graphics.pending.is_none() && self.graphics.pending_cpu.is_none();
        let software_threads = self.graphics.software == RendererMode::Threaded;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.tr(Text::Crt)),
            )
            .child(label(p, self.tr(Text::CrtHint)))
            .child(
                div().flex().gap_2().children(
                    [
                        (CrtFilter::Off, self.tr(Text::Off)),
                        (CrtFilter::Soft, self.tr(Text::Soft)),
                        (CrtFilter::Classic, self.tr(Text::Classic)),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (filter, name))| {
                        button(p, ("crt-choice", index), name, self.crt_filter() == filter)
                            .on_click(
                                cx.listener(move |view, _, _, cx| view.set_crt_filter(filter, cx)),
                            )
                    }),
                ),
            )
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.tr(Text::Performance)),
            )
            .child(label(p, self.tr(Text::PerformanceHint)))
            .child(
                switch(
                    p,
                    "vulkan-switch",
                    self.tr(Text::Vulkan),
                    self.tr(Text::VulkanHint),
                    hardware,
                    ready && cfg!(feature = "vulkan"),
                )
                .when(ready && cfg!(feature = "vulkan"), |element| {
                    element.on_click(cx.listener(|view, _, _, cx| {
                        let mode = if view.snapshot.renderer == RendererMode::Vulkan {
                            view.graphics.software
                        } else {
                            RendererMode::Vulkan
                        };
                        view.select_renderer(mode, cx);
                    }))
                }),
            )
            .child(
                switch(
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
                } else if cfg!(feature = "vulkan") {
                    self.tr(Text::SoftwareHint).into()
                } else {
                    self.tr(Text::NoVulkan).into()
                },
            ))
    }
}

fn switch(
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
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(title),
                )
                .child(label(p, detail)),
        )
        .child(
            div()
                .w(px(36.))
                .h(px(20.))
                .px(px(3.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(if enabled { p.primary } else { p.border }))
                .when(enabled, gpui::Styled::justify_end)
                .child(div().size(px(14.)).rounded_full().bg(rgb(if enabled {
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
}
