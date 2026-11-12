use super::{
    Desktop, WorkspacePage,
    components::{action, button, card, heading, label, pair},
};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::systems::ps1::{
    cpu::REGISTER_NAMES,
    runner::{Command, SnapshotMode},
};
use gpui::{Context, Window, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn sync_snapshot_mode(&mut self, visible: bool) {
        let mode = if visible && self.debugger_enabled {
            SnapshotMode::Debug
        } else {
            SnapshotMode::Display
        };
        if mode == self.snapshot_mode {
            return;
        }
        self.snapshot_mode = mode;
        if mode == SnapshotMode::Display {
            // Release strings and vector capacity immediately, including when paused.
            self.snapshot.disassembly = Vec::new();
            self.snapshot.mode = mode;
        }
        if let Err(error) = self.runner.send(Command::SetSnapshotMode(mode)) {
            self.notice = Some(error);
        }
    }

    pub(super) fn debug_page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let page = div().flex().flex_col().gap_5().child(self.debug_header(cx));
        if !self.debugger_enabled {
            return page.child(
                card(p)
                    .p_6()
                    .gap_3()
                    .child(div().text_lg().child(self.tr(Text::DebuggerOff)))
                    .child(label(p, self.tr(Text::DebuggerOffHint))),
            );
        }
        if self.snapshot.mode != SnapshotMode::Debug {
            return page.child(
                card(p)
                    .p_6()
                    .child(label(p, self.tr(Text::DebuggerLoading))),
            );
        }
        let stacked = window.viewport_size().width < px(1120.);
        let instructions = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.disassembly(cx))
            .child(self.gpu_details());
        let registers = self
            .registers()
            .when(!stacked, |element| element.w(px(310.)).flex_shrink_0());
        page.child(if stacked {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(instructions)
                .child(registers)
        } else {
            div()
                .flex()
                .items_start()
                .gap_4()
                .child(instructions)
                .child(registers)
        })
    }

    fn debug_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font(self.fonts.bold())
                            .text_color(rgb(p.primary))
                            .child(self.tr(Text::DebugEyebrow)),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::DebugWorkspace)),
                    )
                    .child(label(p, self.tr(Text::DebugHint))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        action(
                            &self.fonts,
                            p,
                            "debug-back",
                            self.tr(Text::BackConsole),
                            Some("back"),
                            false,
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.show_page(WorkspacePage::Console, window, cx);
                        })),
                    )
                    .child(
                        button(
                            &self.fonts,
                            p,
                            "debug-toggle",
                            self.tr(if self.debugger_enabled {
                                Text::DisableDebugger
                            } else {
                                Text::EnableDebugger
                            }),
                            !self.debugger_enabled,
                        )
                        .debug_selector(|| "debug-toggle".into())
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.debugger_enabled = !view.debugger_enabled;
                            cx.notify();
                        })),
                    )
                    .child(
                        action(
                            &self.fonts,
                            p,
                            "debug-run",
                            if self.snapshot.running {
                                self.tr(Text::Pause)
                            } else {
                                self.tr(Text::Run)
                            },
                            Some(if self.snapshot.running {
                                "pause"
                            } else {
                                "play"
                            }),
                            true,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.command(Command::Toggle, cx))),
                    ),
            )
    }

    fn disassembly(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "debug-instructions".into())
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::Instructions),
                "MIPS I",
            ))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_3()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(div().w(px(16.)))
                    .child(div().w(px(88.)).child(self.tr(Text::Address)))
                    .child(div().w(px(88.)).child(self.tr(Text::Opcode)))
                    .child(self.tr(Text::Instruction)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .font(self.fonts.mono())
                    .children(self.snapshot.disassembly.iter().enumerate().map(
                        |(index, (address, word, instruction))| {
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .text_xs()
                                .bg(rgb(if index == 0 { p.tonal } else { p.surface }))
                                .text_color(rgb(if index == 0 { p.on_tonal } else { p.muted }))
                                .child(
                                    div()
                                        .w(px(16.))
                                        .text_color(rgb(p.primary))
                                        .child(if index == 0 { "›" } else { " " }),
                                )
                                .child(div().w(px(88.)).child(format!("{address:08X}")))
                                .child(div().w(px(88.)).child(word.map_or_else(
                                    || "--------".into(),
                                    |word| format!("{word:08X}"),
                                )))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(instruction.clone()),
                                )
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        action(
                            &self.fonts,
                            p,
                            "debug-step",
                            self.tr(Text::Step),
                            Some("skip"),
                            false,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.command(Command::Step, cx))),
                    )
                    .child(
                        button(&self.fonts, p, "debug-slice", self.tr(Text::Slice), false)
                            .on_click(
                                cx.listener(|view, _, _, cx| view.command(Command::Slice, cx)),
                            ),
                    )
                    .child(
                        button(&self.fonts, p, "debug-reset", self.tr(Text::Reset), false)
                            .on_click(
                                cx.listener(|view, _, _, cx| view.command(Command::Reset, cx)),
                            ),
                    ),
            )
    }

    fn registers(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        let cpu = &self.snapshot.cpu;
        card(p)
            .p_4()
            .gap_3()
            .child(heading(&self.fonts, p, self.tr(Text::Registers), "R3000A"))
            .child(
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(p.surface_variant))
                    .child(label(p, self.tr(Text::ProgramCounter)))
                    .child(
                        div()
                            .mt_1()
                            .text_xl()
                            .font(self.fonts.mono())
                            .text_color(rgb(p.primary))
                            .child(format!("{:08X}", cpu.pc)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font(self.fonts.mono())
                    .children((0..16).map(|row| {
                        div()
                            .flex()
                            .gap_3()
                            .children([row, row + 16].map(|register| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .justify_between()
                                    .gap_2()
                                    .py_1()
                                    .text_xs()
                                    .child(
                                        div()
                                            .text_color(rgb(p.muted))
                                            .child(REGISTER_NAMES[register]),
                                    )
                                    .child(format!("{:08X}", cpu.registers[register]))
                            }))
                    })),
            )
            .child(div().h(px(1.)).bg(rgb(p.border)))
            .child(pair(
                p,
                "HI / LO",
                format!("{:08X} / {:08X}", cpu.hi, cpu.lo),
            ))
            .child(pair(p, "COP0 status", format!("{:08X}", cpu.cop0[12])))
            .child(pair(
                p,
                "Cause / EPC",
                format!("{:08X} / {:08X}", cpu.cop0[13], cpu.cop0[14]),
            ))
    }

    fn gpu_details(&self) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .p_4()
            .gap_3()
            .child(heading(
                &self.fonts,
                p,
                self.tr(Text::GpuStatus),
                "PLAYSTATION",
            ))
            .child(pair(p, self.tr(Text::Renderer), self.renderer_label()))
            .child(pair(
                p,
                self.tr(Text::Resolution),
                format!(
                    "{} × {}",
                    self.snapshot.dimensions.0, self.snapshot.dimensions.1
                ),
            ))
            .child(pair(
                p,
                self.tr(Text::GpuStatus),
                format!("{:08X}", self.snapshot.gpu_status),
            ))
            .child(pair(p, self.tr(Text::Memory), "2 MiB / 1024 × 512"))
            .child(pair(
                p,
                self.tr(Text::InstructionsExecuted),
                self.snapshot.cpu.instructions.to_string(),
            ))
    }
}
