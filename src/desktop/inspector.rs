use super::{
    Desktop, WorkspacePage,
    components::{action, button, card, heading, label, page_heading, pair},
};
use eidoryn_gba::{InstructionSet, debug::DebugSnapshot};
use eidoryn_psx::i18n::Text;
use eidoryn_psx::shortcuts::ShortcutAction;
use eidoryn_psx::systems::gba::Command as GbaCommand;
use eidoryn_psx::systems::nds::Command as NdsCommand;
use eidoryn_psx::systems::ps1::{
    cpu::REGISTER_NAMES,
    runner::{Command, Snapshot, SnapshotMode},
};
use gpui::{Context, Window, div, prelude::*, px, rgb};

#[derive(Clone, Copy)]
enum Capture<'a> {
    Ps1(&'a Snapshot),
    Gba(&'a DebugSnapshot),
}

impl Capture<'_> {
    fn pc(self) -> u32 {
        match self {
            Self::Ps1(snapshot) => snapshot.cpu.pc,
            Self::Gba(debug) => debug.registers[15],
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DebugStep {
    Instruction,
    Slice,
}

impl Desktop {
    fn capture(&self) -> Option<Capture<'_>> {
        if self.is_nds() {
            None
        } else if self.is_gba() {
            self.gba.snapshot.debug.as_deref().map(Capture::Gba)
        } else {
            (self.snapshot.mode == SnapshotMode::Debug).then_some(Capture::Ps1(&self.snapshot))
        }
    }

    pub(super) fn step_debugger(&mut self, step: DebugStep, cx: &mut Context<Self>) {
        if self.is_nds() {
            self.release_buttons(cx);
            self.nds_command(
                match step {
                    DebugStep::Instruction => NdsCommand::Step,
                    DebugStep::Slice => NdsCommand::Frame,
                },
                cx,
            );
        } else if self.is_gba() {
            self.release_buttons(cx);
            self.gba_command(
                match step {
                    DebugStep::Instruction => GbaCommand::Step,
                    DebugStep::Slice => GbaCommand::Slice,
                },
                cx,
            );
        } else {
            self.command(
                match step {
                    DebugStep::Instruction => Command::Step,
                    DebugStep::Slice => Command::Slice,
                },
                cx,
            );
        }
    }

    pub(super) fn sync_snapshot_mode(&mut self, visible: bool) {
        let enabled = visible && self.debugger_enabled;
        let capture_gba = enabled && self.is_gba() && self.gba.runner.is_some();
        if capture_gba != self.gba.capturing_debug {
            if let Some(runner) = &self.gba.runner
                && let Err(error) = runner.send(GbaCommand::SetDebug(capture_gba))
            {
                self.notice = Some(error);
            }
            self.gba.capturing_debug = capture_gba;
            if !capture_gba {
                self.gba.snapshot.debug = None;
            }
        }
        let mode = if enabled && self.is_ps1() {
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
        let page = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.debug_header(window, cx))
            .child(self.capture_controls(cx));
        if !self.debugger_enabled {
            return page.child(
                card(p)
                    .px_6()
                    .py_12()
                    .items_center()
                    .text_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::DebuggerOff)),
                    )
                    .child(label(p, self.tr(Text::DebuggerOffHint)).max_w(px(420.))),
            );
        }
        if self.is_nds() {
            let panel = card(p).p_4().gap_3();
            return page.child(if self.nds.snapshot.path.is_some() {
                panel
                    .debug_selector(|| "debug-nds".into())
                    .child(heading(
                        &self.fonts,
                        p,
                        self.tr(Text::ProgramCounter),
                        "Nintendo DS",
                    ))
                    .child(pair(
                        p,
                        "ARM9 PC",
                        format!("{:08X}", self.nds.snapshot.arm9_pc),
                    ))
                    .child(pair(
                        p,
                        "ARM7 PC",
                        format!("{:08X}", self.nds.snapshot.arm7_pc),
                    ))
                    .child(self.debug_step_controls(cx))
            } else {
                panel.child(label(p, self.tr(Text::OpenNds)))
            });
        }
        let Some(capture) = self.capture() else {
            return page.child(card(p).px_6().py_12().items_center().child(label(
                p,
                self.tr(if self.is_gba() && self.gba.snapshot.path.is_none() {
                    Text::DebuggerOpenCartridge
                } else {
                    Text::DebuggerLoading
                }),
            )));
        };
        let stacked = self.content_width(window) < px(760.);
        let instructions = self.disassembly(capture, cx).flex_1().min_w_0();
        let details = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.registers(capture))
            .child(self.gpu_details(capture))
            .when(!stacked, |element| {
                element
                    .w(gpui::relative(0.40))
                    .min_w(px(270.))
                    .flex_shrink_0()
            });
        page.child(if stacked {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(instructions)
                .child(details)
        } else {
            div()
                .flex()
                .items_start()
                .gap_5()
                .child(instructions)
                .child(details)
        })
    }

    fn debug_header(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let back = self.tr(Text::ReturnToGame);
        let back = if window.is_fullscreen() {
            back.into()
        } else {
            self.shortcut_label(back, ShortcutAction::Enlarge, None)
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(page_heading(
                &self.fonts,
                p,
                self.tr(Text::DeveloperTools),
                self.tr(Text::Debugger),
                self.tr(Text::DebugHint),
            ))
            .child(
                action(&self.fonts, p, "debug-back", back, Some("back"), false)
                    .debug_selector(|| "debug-back".into())
                    .on_click(cx.listener(|view, _, window, cx| {
                        if window.is_fullscreen() {
                            view.show_page(WorkspacePage::Game, window, cx);
                        } else {
                            view.toggle_view(window, cx);
                        }
                    })),
            )
    }

    fn capture_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let capture = if self.capture().is_some()
            || self.is_nds() && self.debugger_enabled && self.nds.snapshot.path.is_some()
        {
            self.tr(Text::CaptureActive)
        } else if self.debugger_enabled
            && !self.is_nds()
            && !(self.is_gba() && self.gba.snapshot.path.is_none())
        {
            self.tr(Text::DebuggerLoading)
        } else {
            self.tr(Text::Paused)
        };
        let statuses = [
            (self.tr(Text::GameExecution), self.playback_label()),
            (self.tr(Text::CaptureStatus), capture),
        ];
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .children(statuses.map(|(title, status)| {
                label(p, format!("{title}  ·  {status}"))
                    .text_xs()
                    .rounded_full()
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(p.border))
            }))
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
                    false,
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
                    self.shortcut_label(
                        self.tr(if self.rewinding() {
                            Text::EndRewind
                        } else if self.running() {
                            Text::Pause
                        } else {
                            Text::Run
                        }),
                        ShortcutAction::Toggle,
                        None,
                    ),
                    Some(if self.running() || self.rewinding() {
                        "pause"
                    } else {
                        "play"
                    }),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.toggle_playback(cx))),
            )
    }

    fn disassembly(&self, capture: Capture<'_>, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .debug_selector(|| "debug-instructions".into())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font(self.fonts.semibold())
                            .child(self.tr(Text::Instructions)),
                    )
                    .child(
                        label(p, format!("PC {:08X}", capture.pc()))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .font(self.fonts.mono())
                            .bg(rgb(p.tonal))
                            .text_color(rgb(p.on_tonal)),
                    )
                    .p_4()
                    .border_b_1()
                    .border_color(rgb(p.border)),
            )
            .child(self.instruction_table(capture))
            .child(self.debug_step_controls(cx))
    }

    fn debug_step_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .p_4()
            .child(
                action(
                    &self.fonts,
                    p,
                    "debug-step",
                    self.shortcut_label(self.tr(Text::Step), ShortcutAction::Step, None)
                        .hint(),
                    Some("skip"),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.step_debugger(DebugStep::Instruction, cx);
                })),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    "debug-slice",
                    self.shortcut_label(
                        self.tr(if self.is_nds() {
                            Text::StepFrame
                        } else {
                            Text::Slice
                        }),
                        ShortcutAction::Slice,
                        None,
                    )
                    .hint(),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.step_debugger(DebugStep::Slice, cx))),
            )
            .child(
                button(
                    &self.fonts,
                    p,
                    "debug-reset",
                    self.shortcut_label(self.tr(Text::Reset), ShortcutAction::Reset, None),
                    false,
                )
                .on_click(cx.listener(|view, _, _, cx| view.reset_playback(cx))),
            )
    }

    fn instruction_table(&self, capture: Capture<'_>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(div().w(px(16.)))
                    .child(div().w(px(88.)).child(self.tr(Text::Address)))
                    .child(div().w(px(88.)).child(self.tr(Text::Opcode)))
                    .child(self.tr(Text::Instruction)),
            )
            .child({
                let row = |address: u32, opcode: String, instruction: &str| {
                    let current = address == capture.pc();
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .text_xs()
                        .bg(rgb(if current {
                            p.instruction_highlight()
                        } else {
                            p.surface
                        }))
                        .text_color(rgb(if current {
                            p.on_instruction_highlight()
                        } else {
                            p.text
                        }))
                        .child(div().w(px(16.)).child(if current { "›" } else { " " }))
                        .child(div().w(px(88.)).child(format!("{address:08X}")))
                        .child(div().w(px(88.)).child(opcode))
                        .child(div().flex_1().min_w_0().child(instruction.to_owned()))
                };
                let rows = div().flex().flex_col().font(self.fonts.mono());
                match capture {
                    Capture::Ps1(snapshot) => rows.children(snapshot.disassembly.iter().map(
                        |(address, word, instruction)| {
                            row(
                                *address,
                                word.map_or_else(
                                    || "--------".into(),
                                    |word| format!("{word:08X}"),
                                ),
                                instruction,
                            )
                        },
                    )),
                    Capture::Gba(debug) => rows.children(debug.instructions.iter().map(|ins| {
                        let opcode = match (debug.instruction_set, ins.bytes) {
                            (InstructionSet::Thumb, 4) => {
                                format!("{:04X} {:04X}", ins.opcode as u16, ins.opcode >> 16)
                            }
                            (InstructionSet::Thumb, _) => format!("{:04X}", ins.opcode),
                            (InstructionSet::Arm, _) => format!("{:08X}", ins.opcode),
                        };
                        row(ins.address, opcode, &ins.assembly)
                    })),
                }
            })
    }

    fn registers(&self, capture: Capture<'_>) -> gpui::Div {
        const ARM_REGISTERS: [&str; 16] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "SP",
            "LR", "PC",
        ];
        let p = &self.themes.active().palette;
        let (cpu_name, names, registers): (_, &[&str], &[u32]) = match capture {
            Capture::Ps1(snapshot) => ("R3000A", &REGISTER_NAMES, &snapshot.cpu.registers),
            Capture::Gba(debug) => ("ARM7TDMI", &ARM_REGISTERS, &debug.registers),
        };
        let half = registers.len() / 2;
        let card = card(p)
            .debug_selector(|| "debug-registers".into())
            .p_4()
            .gap_3()
            .child(heading(&self.fonts, p, self.tr(Text::Registers), cpu_name))
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.surface_variant))
                    .child(label(p, self.tr(Text::ProgramCounter)))
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(18.))
                            .font(self.fonts.mono())
                            .text_color(rgb(p.primary))
                            .child(format!("{:08X}", capture.pc())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font(self.fonts.mono())
                    .children((0..half).map(|row| {
                        div()
                            .flex()
                            .gap_1()
                            .children([row, row + half].map(|register| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .p_2()
                                    .border_b_1()
                                    .border_color(rgb(p.border))
                                    .text_xs()
                                    .child(div().text_color(rgb(p.muted)).child(names[register]))
                                    .child(
                                        div()
                                            .text_color(rgb(p.primary))
                                            .child(format!("{:08X}", registers[register])),
                                    )
                            }))
                    })),
            )
            .child(div().h(px(1.)).bg(rgb(p.border)));
        match capture {
            Capture::Ps1(snapshot) => {
                let cpu = &snapshot.cpu;
                card.child(pair(
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
            Capture::Gba(debug) => card
                .child(pair(p, "CPSR", format!("{:08X}", debug.cpsr)))
                .child(pair(
                    p,
                    self.tr(Text::InstructionSet),
                    match debug.instruction_set {
                        InstructionSet::Arm => "ARM",
                        InstructionSet::Thumb => "Thumb",
                    },
                ))
                .child(label(p, self.tr(Text::StackMemory)))
                .children(debug.stack.iter().enumerate().map(|(i, word)| {
                    let addr = (debug.registers[13] & !3).wrapping_add(i as u32 * 4);
                    pair(p, format!("{addr:08X}"), format!("{word:08X}"))
                })),
        }
    }

    fn gpu_details(&self, capture: Capture<'_>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let card = card(p).debug_selector(|| "debug-gpu".into()).p_4().gap_3();
        if let Capture::Gba(debug) = capture {
            return card
                .child(heading(
                    &self.fonts,
                    p,
                    self.tr(Text::GpuStatus),
                    "GAME BOY ADVANCE",
                ))
                .child(pair(
                    p,
                    self.tr(Text::Renderer),
                    self.gba
                        .snapshot
                        .graphics_device
                        .clone()
                        .unwrap_or_else(|| self.tr(Text::SerialRenderer).into()),
                ))
                .child(pair(
                    p,
                    self.tr(Text::Resolution),
                    format!("{} × {}", eidoryn_gba::WIDTH, eidoryn_gba::HEIGHT),
                ))
                .child(pair(
                    p,
                    "DISPCNT / DISPSTAT",
                    format!(
                        "{:04X} / {:04X}",
                        debug.display_control, debug.display_status
                    ),
                ))
                .child(pair(p, "VCOUNT", debug.scanline.to_string()));
        }
        card.child(heading(
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
                self.snapshot.native_dimensions.0, self.snapshot.native_dimensions.1
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
