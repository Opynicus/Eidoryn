//! Presentation and input for the Nintendo DS worker.
use super::{Desktop, GamePlatform, WorkspacePage};
use eidoryn_nds::{SCREEN_HEIGHT, SCREEN_WIDTH};
use eidoryn_psx::{
    keybindings::NdsKeyBindings,
    systems::nds::{Command, Runner, Snapshot},
};
use gpui::{
    Bounds, Context, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    Window, div, prelude::*,
};
use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

#[derive(Default)]
pub(super) struct Session {
    pub runner: Option<Runner>,
    pub snapshot: Snapshot,
    pub keybindings_draft: Option<NdsKeyBindings>,
    pub screen: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub pen: Option<(u8, u8)>,
}

impl Desktop {
    pub(super) fn open_nds(&mut self, path: PathBuf, title: String, cx: &mut Context<Self>) {
        self.switch_core(GamePlatform::Nds, cx);
        if !self.is_nds() || self.closing() {
            return;
        }
        self.library.cancel_pending_launch();
        self.image_picker.take();
        if self.nds.runner.is_none() {
            let store = self.library.store.clone();
            match self.cartridge_data_dir().and_then(|dir| {
                Runner::start_with_audio(
                    store,
                    dir,
                    Some(self.audio.buffer(GamePlatform::Nds).clone()),
                )
            }) {
                Ok(runner) => self.nds.runner = Some(runner),
                Err(error) => {
                    self.notice = Some(error);
                    cx.notify();
                    return;
                }
            }
        }
        self.stop_input(cx);
        self.snapshot_menu = None;
        self.snapshot_confirmation = None;
        self.renaming_state = None;
        self.notice = None;
        self.nds_command(Command::Load { path, title }, cx);
    }

    pub(super) fn receive_nds(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.nds.runner.as_ref().and_then(Runner::take_snapshot) else {
            return;
        };
        let old = &self.nds.snapshot;
        let loaded = old.media_revision != next.media_revision;
        let changed = !Arc::ptr_eq(&old.rgb, &next.rgb);
        let visible = matches!(self.page, WorkspacePage::Console | WorkspacePage::Game)
            || self.presentation.shows_video();
        let redraw = loaded
            || changed && visible
            || self.page == WorkspacePage::Debug && self.debugger_enabled
            || (old.video_fps - next.video_fps).abs() > f64::EPSILON
            || old.running != next.running
            || old.error != next.error
            || old.quick_slot_revision != next.quick_slot_revision
            || old.quick_slot != next.quick_slot;
        if loaded && let Some(path) = &next.path {
            self.remember_media(path, false);
        }
        if self.is_nds() {
            if loaded {
                self.page = WorkspacePage::Game;
                self.focus_game = true;
                self.presentation.windowed_video = false;
            }
            if changed && let Some(image) = &mut self.image {
                image.dirty = true;
            }
            if redraw {
                cx.notify();
            }
        }
        self.nds.snapshot = next;
    }

    pub(super) fn nds_command(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.closing() {
            return;
        }
        if let Some(runner) = &self.nds.runner
            && let Err(error) = runner.send(command)
        {
            self.notice = Some(error);
            cx.notify();
        }
    }

    pub(super) fn nds_touchscreen(video: impl IntoElement, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id("nds-touchscreen")
            .debug_selector(|| "nds-touchscreen".into())
            .size_full()
            .child(video)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    window.focus(&view.focus, cx);
                    view.nds_pen(Some(event.position), cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.nds_pen(None, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.nds_pen(None, cx)),
            )
    }

    /// Track an active drag across the entire window, including the letterbox and sidebar.
    pub(super) fn move_nds_pen(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.nds.pen.is_some() {
            self.nds_pen(
                (event.pressed_button == Some(MouseButton::Left)).then_some(event.position),
                cx,
            );
        }
    }

    pub(super) fn nds_pen(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let point = self
            .nds
            .screen
            .get()
            .and_then(|bounds| position.and_then(|p| stylus_point(bounds, p)));
        if self.nds.pen != point {
            self.nds.pen = point;
            self.nds_command(Command::Touch(point), cx);
        }
    }
}

/// The fitted canvas contains the top LCD first. Its lower half is the digitizer.
fn stylus_point(screen: Bounds<Pixels>, position: Point<Pixels>) -> Option<(u8, u8)> {
    if screen.size.width <= gpui::px(0.) || screen.size.height <= gpui::px(0.) {
        return None;
    }
    let x = f32::from(position.x - screen.origin.x) / f32::from(screen.size.width);
    let y = f32::from(position.y - screen.origin.y) / f32::from(screen.size.height);
    ((0.0..1.0).contains(&x) && (0.5..1.0).contains(&y)).then_some({
        (
            (x * SCREEN_WIDTH as f32) as u8,
            ((y - 0.5) * (2 * SCREEN_HEIGHT) as f32) as u8,
        )
    })
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

#[cfg(test)]
mod coordinates {
    use super::stylus_point;
    use gpui::{Bounds, point, px, size};

    #[test]
    fn only_the_bottom_lcd_accepts_stylus_input() {
        let bounds = Bounds::new(point(px(40.), px(10.)), size(px(512.), px(768.)));
        for (x, y, expected) in [
            (40., 394., Some((0, 0))),
            (296., 586., Some((128, 96))),
            (551.9, 777.9, Some((255, 191))),
            (40., 393.9, None),
            (39.9, 394., None),
            (552., 400., None),
            (40., 778., None),
        ] {
            assert_eq!(stylus_point(bounds, point(px(x), px(y))), expected);
        }
    }
}
