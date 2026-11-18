//! Single-line editor for snapshot names, including native IME composition.
use eidoryn_psx::theme::Palette;
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, ShapedLine, TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};
use std::ops::Range;

pub(super) struct TextInput {
    focus: FocusHandle,
    text: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    palette: Palette,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    dragging: bool,
}

impl TextInput {
    pub(super) fn new(text: String, palette: Palette, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            cursor: text.len(),
            text,
            anchor: 0,
            marked: None,
            palette,
            line: None,
            bounds: None,
            scroll: px(0.),
            dragging: false,
        }
    }

    pub(super) fn value(&self) -> &str {
        &self.text
    }

    pub(super) fn is_composing(&self) -> bool {
        self.marked.is_some()
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn utf8_offset(text: &str, offset: usize) -> usize {
        let mut units = 0;
        for (byte, ch) in text.char_indices() {
            if units + ch.len_utf16() > offset {
                return byte;
            }
            units += ch.len_utf16();
        }
        text.len()
    }

    fn utf16_range(&self, range: Range<usize>) -> Range<usize> {
        self.text[..range.start].encode_utf16().count()
            ..self.text[..range.end].encode_utf16().count()
    }

    fn utf8_range(&self, range: Range<usize>) -> Range<usize> {
        let start = Self::utf8_offset(&self.text, range.start);
        start..Self::utf8_offset(&self.text, range.end).max(start)
    }

    fn replace(&mut self, range: Option<Range<usize>>, text: &str) -> Range<usize> {
        let range = range
            .map(|range| self.utf8_range(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let text = text.replace(['\r', '\n'], " ");
        self.text.replace_range(range.clone(), &text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
        self.marked = None;
        range.start..self.cursor
    }

    fn changed(&self, window: &Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        let shortcut = modifiers.control || modifiers.platform;
        match key {
            "a" if shortcut => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            "c" | "x" if shortcut => {
                let range = self.selection();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(self.text[range].into()));
                    if key == "x" {
                        self.replace(None, "");
                    }
                }
            }
            "v" if shortcut => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.replace(None, &text);
                }
            }
            "left" | "right" | "home" | "end" | "backspace" | "delete" => {
                let selection = self.selection();
                let previous = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index);
                let next = self.text[self.cursor..]
                    .chars()
                    .next()
                    .map_or(self.cursor, |ch| self.cursor + ch.len_utf8());
                match key {
                    "backspace" | "delete" => {
                        if selection.is_empty() {
                            self.anchor = if key == "backspace" { previous } else { next };
                        }
                        self.replace(None, "");
                    }
                    _ => {
                        self.cursor = match key {
                            "home" => 0,
                            "end" => self.text.len(),
                            "left" if !modifiers.shift && !selection.is_empty() => selection.start,
                            "right" if !modifiers.shift && !selection.is_empty() => selection.end,
                            "left" => previous,
                            _ => next,
                        };
                        if !modifiers.shift {
                            self.anchor = self.cursor;
                        }
                        self.marked = None;
                    }
                }
            }
            _ => return,
        }
        self.changed(window, cx);
        cx.stop_propagation();
    }

    fn mouse_index(&self, position: Point<Pixels>) -> usize {
        self.bounds
            .zip(self.line.as_ref())
            .filter(|(_, line)| line.text.as_ref() == self.text)
            .map_or(self.cursor, |(bounds, line)| {
                line.closest_index_for_x(position.x - bounds.left() + self.scroll)
            })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.utf8_range(range);
        *actual = Some(self.utf16_range(range.clone()));
        Some(self.text[range].into())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(self.selection()),
            reversed: self.cursor < self.anchor,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.utf8_range(range);
        self.anchor = range.start;
        self.cursor = range.end;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|range| self.utf16_range(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.changed(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace(range, text);
        self.changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.replace(range, text);
        if !inserted.is_empty() {
            self.marked = Some(inserted.clone());
        }
        if let Some(selected) = selected {
            let inserted_text = &self.text[inserted.clone()];
            self.anchor = inserted.start + Self::utf8_offset(inserted_text, selected.start);
            self.cursor = inserted.start + Self::utf8_offset(inserted_text, selected.end);
        }
        self.changed(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.utf8_range(range);
        let line = self
            .line
            .as_ref()
            .filter(|line| line.text.as_ref() == self.text)?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(range.start) - self.scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(range.end) - self.scroll,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.bounds?;
        let index = self.mouse_index(position);
        Some(self.text[..index].encode_utf16().count())
    }
}

impl Render for TextInput {
    #[allow(clippy::too_many_lines)] // The canvas shares layout and paint state for native text input.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let prepaint_input = cx.entity();
        let paint_input = cx.entity();
        div()
            .id("snapshot-name-input")
            .debug_selector(|| "snapshot-name-input".into())
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.input_border()))
            .bg(rgb(palette.input_surface()))
            .text_color(rgb(palette.text))
            .text_sm()
            .cursor_text()
            .track_focus(&self.focus)
            .focus(move |style| style.border_color(rgb(palette.primary)))
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|input, event: &MouseDownEvent, window, cx| {
                    window.focus(&input.focus, cx);
                    let index = input.mouse_index(event.position);
                    input.cursor = index;
                    if !event.modifiers.shift {
                        input.anchor = index;
                    }
                    if event.click_count > 1 {
                        input.anchor = 0;
                        input.cursor = input.text.len();
                    }
                    input.dragging = true;
                    input.changed(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|input, event: &MouseMoveEvent, window, cx| {
                if input.dragging {
                    input.cursor = input.mouse_index(event.position);
                    input.changed(window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|input, _, _, _| input.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|input, _, _, _| input.dragging = false),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let input = prepaint_input.read(cx);
                        let style = window.text_style();
                        let run = TextRun {
                            len: input.text.len(),
                            font: style.font(),
                            color: rgb(palette.text).into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let runs = if let Some(marked) = &input.marked {
                            vec![
                                TextRun {
                                    len: marked.start,
                                    ..run.clone()
                                },
                                TextRun {
                                    len: marked.len(),
                                    underline: Some(UnderlineStyle {
                                        color: Some(run.color),
                                        thickness: px(1.),
                                        wavy: false,
                                    }),
                                    ..run.clone()
                                },
                                TextRun {
                                    len: input.text.len() - marked.end,
                                    ..run
                                },
                            ]
                            .into_iter()
                            .filter(|run| run.len > 0)
                            .collect()
                        } else {
                            vec![run]
                        };
                        let line = window.text_system().shape_line(
                            input.text.clone().into(),
                            style.font_size.to_pixels(window.rem_size()),
                            &runs,
                            None,
                        );
                        let cursor_x = line.x_for_index(input.cursor);
                        let scroll = input
                            .scroll
                            .min(cursor_x)
                            .max(cursor_x - (bounds.size.width - px(2.)))
                            .max(px(0.));
                        (line, scroll, input.selection())
                    },
                    move |bounds, (line, scroll, selection), window, cx| {
                        let focus = paint_input.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_input.clone()),
                            cx,
                        );
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            let origin = point(bounds.left() - scroll, bounds.top());
                            if !selection.is_empty() && focus.is_focused(window) {
                                window.paint_quad(fill(
                                    Bounds::from_corners(
                                        point(
                                            origin.x + line.x_for_index(selection.start),
                                            bounds.top(),
                                        ),
                                        point(
                                            origin.x + line.x_for_index(selection.end),
                                            bounds.bottom(),
                                        ),
                                    ),
                                    rgb(palette.tonal),
                                ));
                            }
                            let _ = line.paint(
                                origin,
                                bounds.size.height,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            if selection.is_empty() && focus.is_focused(window) {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            origin.x + line.x_for_index(selection.end),
                                            bounds.top(),
                                        ),
                                        size(px(1.), bounds.size.height),
                                    ),
                                    rgb(palette.text),
                                ));
                            }
                        });
                        paint_input.update(cx, |input, _| {
                            input.line = Some(line);
                            input.bounds = Some(bounds);
                            input.scroll = scroll;
                        });
                    },
                )
                .w_full()
                .h(px(22.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_keep_unicode_boundaries() {
        let text = "A🙂中文";
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 99].map(|index| TextInput::utf8_offset(text, index)),
            [0, 1, 1, 5, 8, 11, 11]
        );
    }

    #[cfg(feature = "ui-tests")]
    #[gpui::test]
    fn ime_replaces_marked_text_and_keeps_surrogate_selection(cx: &mut gpui::TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|_, cx| TextInput::new("A🙂Z".into(), Palette::MIDNIGHT, cx));
        view.update_in(cx, |input, window, cx| {
            input.replace_and_mark_text_in_range(Some(1..3), "中文", Some(0..2), window, cx);
            assert_eq!(input.value(), "A中文Z");
            assert_eq!(input.marked_text_range(window, cx), Some(1..3));
            input.replace_text_in_range(None, "🙂", window, cx);
            assert_eq!(input.value(), "A🙂Z");
            assert!(!input.is_composing());
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                3..3
            );
        });
    }

    #[cfg(feature = "ui-tests")]
    #[gpui::test]
    fn edits_unicode_selection_and_clipboard(cx: &mut gpui::TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|_, cx| TextInput::new("旧名称".into(), Palette::MIDNIGHT, cx));
        view.update_in(cx, |input, window, cx| window.focus(&input.focus, cx));
        cx.run_until_parked();
        cx.simulate_input("存档🙂");
        cx.simulate_keystrokes("left backspace shift-end ctrl-c ctrl-a ctrl-v");
        assert_eq!(
            view.read_with(cx, |input, _| input.value().to_owned()),
            "🙂"
        );
        cx.simulate_keystrokes("home delete");
        assert_eq!(view.read_with(cx, |input, _| input.value().to_owned()), "");
    }
}
