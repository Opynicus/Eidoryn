//! Material volume control shared by the inspector and game views.
use super::{
    Desktop,
    components::{button, icon_button},
};
use eidoryn_psx::audio::{AudioBuffer, output::AudioOutput};
use eidoryn_psx::i18n::Text;
use gpui::{
    Bounds, Context, CursorStyle, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, canvas, div, fill, point, prelude::*, px, rgb, size,
};

pub(super) struct AudioControls {
    pub buffer: AudioBuffer,
    pub output: Option<AudioOutput>,
    pub volume: u8,
    pub muted: bool,
    pub dragging: bool,
}

impl Desktop {
    pub(super) fn audio_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = &self.themes.active().palette;
        let sound = if self.audio.output.is_none() {
            button(&self.fonts, p, "mute", self.tr(Text::EnableSound), false)
        } else {
            icon_button(
                p,
                "mute",
                if self.audio.muted { "mute" } else { "volume" },
                self.tr(Text::Mute),
            )
        };
        div()
            .flex()
            .flex_shrink_0()
            .gap_2()
            .items_center()
            .child(sound.on_click(cx.listener(|view, _, _, cx| view.toggle_audio(cx))))
            .child(
                div()
                    .id("volume-slider")
                    .debug_selector(|| "volume-slider".into())
                    .tab_index(0)
                    .border_1()
                    .border_color(rgb(p.surface))
                    .focus({
                        let color = p.primary;
                        move |style| style.border_color(rgb(color))
                    })
                    .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                        let volume = match event.keystroke.key.as_str() {
                            "left" | "down" => view.audio.volume.saturating_sub(5),
                            "right" | "up" => (view.audio.volume + 5).min(100),
                            "home" => 0,
                            "end" => 100,
                            _ => return,
                        };
                        view.audio.volume = volume;
                        view.audio.muted = false;
                        view.audio.buffer.set_volume(volume);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_key_up(cx.listener(|view, event: &gpui::KeyUpEvent, _, cx| {
                        if matches!(
                            event.keystroke.key.as_str(),
                            "left" | "right" | "up" | "down" | "home" | "end"
                        ) {
                            view.persist_volume();
                            cx.stop_propagation();
                        }
                    }))
                    .child(self.volume_slider(cx)),
            )
            .child(
                div()
                    .w(px(36.))
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(format!("{}%", self.audio.volume)),
            )
    }

    fn volume_slider(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = &self.themes.active().palette;
        let active = rgb(if self.audio.muted { p.muted } else { p.primary });
        let inactive = rgb(p.tonal);
        let fraction = f32::from(self.audio.volume) / 100.0;
        let dragging = self.audio.dragging;
        let down = cx.weak_entity();
        let movement = down.clone();
        let up = down.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                let track = Bounds::new(
                    point(bounds.left() + px(8.), bounds.top() + px(17.)),
                    size(bounds.size.width - px(16.), px(6.)),
                );
                let thumb = track.left() + track.size.width * fraction;
                window.paint_quad(fill(track, inactive).corner_radii(px(3.)));
                window.paint_quad(
                    fill(
                        Bounds::new(track.origin, size(thumb - track.left(), px(6.))),
                        active,
                    )
                    .corner_radii(px(3.)),
                );
                let radius = if dragging { px(9.) } else { px(7.) };
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(thumb - radius, track.top() + px(3.) - radius),
                            size(radius * 2., radius * 2.),
                        ),
                        active,
                    )
                    .corner_radii(radius),
                );
                window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && event.button == MouseButton::Left
                        && hitbox.is_hovered(window)
                    {
                        let _ = down.update(cx, |view, cx| {
                            view.audio.dragging = true;
                            view.set_volume_at(event.position.x, track, cx);
                        });
                        cx.stop_propagation();
                    }
                });
                // Capture movement across the whole window so dragging beyond either end clamps.
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase.capture() {
                        let _ = movement.update(cx, |view, cx| {
                            if view.audio.dragging {
                                view.audio.dragging = event.dragging();
                                if view.audio.dragging {
                                    view.set_volume_at(event.position.x, track, cx);
                                } else {
                                    view.persist_volume();
                                    cx.notify();
                                }
                            }
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase.capture() && event.button == MouseButton::Left {
                        let _ = up.update(cx, |view, cx| {
                            if view.audio.dragging {
                                view.set_volume_at(event.position.x, track, cx);
                                view.audio.dragging = false;
                                view.persist_volume();
                                cx.notify();
                            }
                        });
                    }
                });
            },
        )
        .w(px(104.))
        .h(px(40.))
    }

    fn set_volume_at(&mut self, x: Pixels, track: Bounds<Pixels>, cx: &mut Context<Self>) {
        let fraction = ((x - track.left()) / track.size.width).clamp(0., 1.);
        self.audio.volume = (fraction * 100.).round() as u8;
        self.audio.muted = false;
        self.audio.buffer.set_volume(self.audio.volume);
        cx.notify();
    }
}
