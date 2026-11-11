use super::{
    Desktop,
    components::{button, label},
};
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
            "CPU JIT · 实验性",
            "可用性取决于平台与构建",
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
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("CRT 显示效果"),
            )
            .child(label(p, "为游戏画面选择显示滤镜。"))
            .child(
                div().flex().gap_2().children(
                    [
                        (CrtFilter::Off, "关闭"),
                        (CrtFilter::Soft, "柔和"),
                        (CrtFilter::Classic, "经典"),
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
                    .child("渲染与性能"),
            )
            .child(label(p, "选项是否可用取决于平台、构建和当前模式。"))
            .child(
                switch(
                    p,
                    "vulkan-switch",
                    "Vulkan 硬件加速",
                    "需要受支持的硬件和构建",
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
                    "软件多线程",
                    "为软件渲染启用独立工作线程",
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
            .child(label(
                p,
                if !ready {
                    "正在应用设置…".into()
                } else if hardware {
                    format!("{} · 独立 GPU 提交线程", self.snapshot.renderer_device)
                } else if cfg!(feature = "vulkan") {
                    "当前使用软件渲染。".into()
                } else {
                    "此构建不包含 Vulkan。".into()
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
