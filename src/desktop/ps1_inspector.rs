use super::{
    Desktop,
    components::{button, card, heading, label, pair},
};
use eidoryn_psx::systems::ps1::{cpu::REGISTER_NAMES, runner::Command};
use gpui::{Context, IntoElement, div, prelude::*, px, rgb};

impl Desktop {
    pub(super) fn registers(&self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.themes.active().palette;
        let cpu = &self.snapshot.cpu;
        card(p)
            .id("cpu-inspector")
            .w(px(318.))
            .min_h_0()
            .overflow_y_scroll()
            .flex_shrink_0()
            .p_5()
            .gap_3()
            .child(heading(
                p,
                "CPU inspector",
                if compact {
                    "R3000A · Scroll"
                } else {
                    "R3000A"
                },
            ))
            .child(
                div()
                    .p_4()
                    .rounded_xl()
                    .bg(rgb(p.tonal))
                    .child(label(p, "Program counter"))
                    .child(
                        div()
                            .mt_1()
                            .text_2xl()
                            .font_family("monospace")
                            .text_color(rgb(p.on_tonal))
                            .child(format!("{:08X}", cpu.pc)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .font_family("monospace")
                    .children((0..16).map(|row| {
                        div()
                            .flex()
                            .justify_between()
                            .children([row, row + 16].map(|r| {
                                div()
                                    .flex()
                                    .w(px(128.))
                                    .justify_between()
                                    .text_xs()
                                    .child(div().text_color(rgb(p.muted)).child(REGISTER_NAMES[r]))
                                    .child(format!("{:08X}", cpu.registers[r]))
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
            .child(div().flex_1())
            .child(self.memory_cards(cx))
            .child(
                div()
                    .p_3()
                    .rounded_xl()
                    .bg(rgb(p.surface_variant))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(pair(p, "Main RAM", "2 MiB"))
                    .child(pair(p, "Video RAM", "1024 × 512"))
                    .child(pair(
                        p,
                        "GPU status",
                        format!("{:08X}", self.snapshot.gpu_status),
                    )),
            )
    }

    fn memory_cards(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_xl()
            .bg(rgb(p.surface_variant))
            .child(heading(p, "Memory cards", "128 KiB"))
            .children(self.snapshot.cards.iter().enumerate().map(|(slot, card)| {
                let status = card.as_ref().map_or_else(
                    || "Disconnected".into(),
                    |card| {
                        let name = card
                            .path
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .map_or_else(|| "Temporary".into(), |name| name.to_string_lossy());
                        format!(
                            "{name} · {}",
                            if card.pending { "Saving…" } else { "Saved" }
                        )
                    },
                );
                div()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .truncate()
                    .child(format!("{}: {status}", slot + 1))
            }))
            .child(
                button(p, "flush-cards", "Flush saves", false)
                    .on_click(cx.listener(|view, _, _, cx| view.command(Command::FlushCards, cx))),
            )
    }
    pub(super) fn disassembly(&self, compact: bool) -> gpui::Div {
        let p = &self.themes.active().palette;
        card(p)
            .p_4()
            .gap_3()
            .flex_shrink_0()
            .child(heading(p, "Instruction stream", "MIPS I"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .font_family("monospace")
                    .children(
                        self.snapshot
                            .disassembly
                            .iter()
                            .take(if compact { 3 } else { 5 })
                            .enumerate()
                            .map(|(index, (address, word, instruction))| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .bg(rgb(if index == 0 { p.tonal } else { p.surface }))
                                    .text_color(rgb(if index == 0 { p.on_tonal } else { p.muted }))
                                    .child(div().w(px(10.)).child(if index == 0 {
                                        ">"
                                    } else {
                                        " "
                                    }))
                                    .child(format!("{address:08X}"))
                                    .child(word.map_or_else(
                                        || "--------".into(),
                                        |word| format!("{word:08X}"),
                                    ))
                                    .child(instruction.clone())
                            }),
                    ),
            )
    }
    pub(super) fn media(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let bios = self.bios.as_ref().map_or_else(
            || "No BIOS selected".into(),
            |path| {
                format!(
                    "BIOS: {}",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                )
            },
        );
        card(p)
            .p_4()
            .gap_2()
            .flex_shrink_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Optical drive"),
                            )
                            .child(label(
                                p,
                                format!(
                                    "{}  ·  Sectors read: {}",
                                    if self.snapshot.disc_mounted {
                                        "Disc mounted"
                                    } else {
                                        "No disc"
                                    },
                                    self.snapshot.cdrom_reads
                                ),
                            )),
                    )
                    .child(
                        button(p, "disc", "Open Disc", true)
                            .on_click(cx.listener(|view, _, _, cx| view.open_disc(false, cx))),
                    )
                    .child(
                        button(p, "fast-disc", "Fast boot", false)
                            .on_click(cx.listener(|view, _, _, cx| view.open_disc(true, cx))),
                    ),
            )
            .child(label(p, bios))
            .child(label(
                p,
                "CUE + BIN tracks / ISO  ·  Fast boot skips BIOS; homebrew only",
            ))
            .child(label(
                p,
                "Pad 1: arrows · X/Z/S/D · Enter/Backspace · Q/E: L1/R1 · A/F: L2/R2",
            ))
    }
}
