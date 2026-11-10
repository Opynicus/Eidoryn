use super::{
    Desktop,
    components::{button, card, label},
};
use eidoryn_psx::{
    cpu_engine::CpuMode,
    gpu::RendererMode,
    runner::{Command, Snapshot},
    theme::Palette,
};
use gpui::{Context, div, prelude::*, px, rgb};

pub(super) struct GraphicsControls {
    pub open: bool,
    software: RendererMode,
    pending: Option<RendererMode>,
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
            pending_cpu: None,
        }
    }

    pub(super) fn receive(&mut self, snapshot: &Snapshot) {
        if self.pending_cpu == Some(snapshot.cpu_mode) || snapshot.message.is_some() {
            self.pending_cpu = None;
        }
        if self.pending == Some(snapshot.renderer) || snapshot.message.is_some() {
            self.pending = None;
        }
        if snapshot.renderer != RendererMode::Vulkan {
            self.software = snapshot.renderer;
        }
    }
}

impl Desktop {
    pub(super) fn graphics_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        button(
            &self.themes.active().palette,
            "graphics",
            "Graphics",
            self.graphics.open,
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.graphics.open = !view.graphics.open;
            view.appearance_open = false;
            cx.notify();
        }))
    }

    fn select_renderer(&mut self, mode: RendererMode, cx: &mut Context<Self>) {
        match self.runner.send(Command::SetRenderer(mode)) {
            Ok(()) => {
                self.graphics.pending = Some(mode);
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
            "CPU JIT · experimental",
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

    pub(super) fn graphics_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let hardware = self.snapshot.renderer == RendererMode::Vulkan;
        let ready = self.graphics.pending.is_none() && self.graphics.pending_cpu.is_none();
        let software_threads = self.graphics.software == RendererMode::Threaded;
        card(p)
            .p_4()
            .gap_3()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Graphics"))
                    .child(
                        button(p, "close-graphics", "Done", false).on_click(cx.listener(
                            |view, _, _, cx| {
                                view.graphics.open = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        switch(
                            p,
                            "vulkan-switch",
                            "Vulkan acceleration",
                            hardware,
                            ready && cfg!(feature = "vulkan"),
                        )
                        .when(
                            ready && cfg!(feature = "vulkan"),
                            |element| {
                                element.on_click(cx.listener(|view, _, _, cx| {
                                    let mode = if view.snapshot.renderer == RendererMode::Vulkan {
                                        view.graphics.software
                                    } else {
                                        RendererMode::Vulkan
                                    };
                                    view.select_renderer(mode, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        switch(
                            p,
                            "threaded-switch",
                            "Software multithreading",
                            software_threads,
                            ready && !hardware,
                        )
                        .when(ready && !hardware, |element| {
                            element.on_click(cx.listener(|view, _, _, cx| {
                                let mode = if view.snapshot.renderer == RendererMode::Threaded {
                                    RendererMode::Serial
                                } else {
                                    RendererMode::Threaded
                                };
                                view.select_renderer(mode, cx);
                            }))
                        }),
                    ),
            )
            .child(self.jit_control(ready, cx))
            .child(label(
                p,
                if !ready {
                    "Applying settings…".into()
                } else if hardware {
                    format!(
                        "{} · dedicated GPU submission thread",
                        self.snapshot.renderer_device
                    )
                } else if cfg!(feature = "vulkan") {
                    "Software rendering · multithreading moves drawing to a separate worker".into()
                } else {
                    "Vulkan is unavailable in this build · enable the vulkan Cargo feature".into()
                },
            ))
    }
}

fn switch(
    p: &Palette,
    id: &'static str,
    title: &'static str,
    enabled: bool,
    available: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .text_sm()
        .when(available, gpui::Styled::cursor_pointer)
        .when(!available, |element| element.opacity(0.45))
        .child(
            div()
                .w(px(44.))
                .h(px(24.))
                .px(px(3.))
                .flex()
                .items_center()
                .rounded_full()
                .bg(rgb(if enabled { p.primary } else { p.border }))
                .when(enabled, gpui::Styled::justify_end)
                .child(div().size(px(18.)).rounded_full().bg(rgb(if enabled {
                    p.on_primary
                } else {
                    p.surface
                }))),
        )
        .child(title)
        .child(label(p, if enabled { "On" } else { "Off" }))
}
