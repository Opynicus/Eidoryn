//! Virtualized collection layouts and shared artwork for the overview and details.
use super::*;
use gpui::{ObjectFit, Pixels, RetainAllImageCache, relative, rgba, uniform_list};

const TONES: [u32; 6] = [0x3c4244, 0x4e4540, 0x353e4b, 0x4b3e41, 0x444b3e, 0x494347];

#[derive(Clone, Copy)]
struct Layout {
    columns: usize,
    compact: bool,
    narrow: bool,
    gap: Pixels,
    width: Pixels,
    cover_width: Pixels,
    row_height: Pixels,
}

impl Layout {
    fn new(viewport: Pixels, content: Pixels, compact: bool) -> Self {
        let narrow = viewport <= px(768.);
        let gap = if narrow { px(12.) } else { px(16.) };
        let columns = if compact {
            1
        } else if viewport <= px(480.) {
            2
        } else if narrow {
            3
        } else if viewport <= px(1050.) {
            4
        } else if viewport <= px(1300.) {
            5
        } else {
            6
        };
        let cover_width = (content - gap * (columns - 1) as f32) / columns as f32;
        let row_height = if compact {
            px(58.)
        } else {
            cover_width + px(92.)
        };
        Self {
            columns,
            compact,
            narrow,
            gap,
            width: content,
            cover_width,
            row_height,
        }
    }
}

impl Desktop {
    pub(super) fn library_results(&self, window: &Window, cx: &Context<Self>) -> gpui::UniformList {
        let layout = Layout::new(
            window.viewport_size().width,
            self.content_width(window),
            self.preferences.library.list,
        );
        let rows = self.library.order.len().div_ceil(layout.columns);
        uniform_list(
            if layout.compact {
                "library-list-results"
            } else {
                "library-grid-results"
            },
            rows,
            cx.processor(move |v, range: std::ops::Range<usize>, _, cx| {
                #[cfg(all(test, feature = "ui-tests"))]
                {
                    v.library.rendered_cells = range.start * layout.columns
                        ..(range.end * layout.columns).min(v.library.order.len());
                }
                range
                    .map(|row| {
                        // UniformList also measures rows without a parent width.
                        let mut line = div()
                            .w(layout.width)
                            .h(layout.row_height)
                            .flex()
                            .items_stretch()
                            .gap(layout.gap);
                        for index in row * layout.columns..(row + 1) * layout.columns {
                            if let Some(&i) = v.library.order.get(index) {
                                let cache = if layout.compact {
                                    None
                                } else {
                                    v.library.thumbnail_cache(i, cx)
                                };
                                let entry = &v.library.entries[i];
                                let cell = v.collection_cell(entry, cx);
                                line = line.child(if layout.compact {
                                    v.collection_list_row(entry, cell, layout.narrow, cx)
                                } else {
                                    v.collection_grid_card(
                                        entry,
                                        cell,
                                        cache,
                                        layout.cover_width,
                                        cx,
                                    )
                                });
                            } else {
                                line = line.child(div().flex_1().min_w_0());
                            }
                        }
                        line
                    })
                    .collect()
            }),
        )
        .flex_1()
        .min_h_0()
        .w_full()
    }

    fn collection_grid_card(
        &self,
        entry: &Entry,
        cell: gpui::Stateful<Div>,
        cache: Option<Entity<RetainAllImageCache>>,
        cover_width: Pixels,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.library_palette();
        let id = entry.game.id;
        let selected = self.card_selected(id);
        cell.flex_col()
            .rounded_md()
            .overflow_hidden()
            .group("library-card")
            .child(
                self.library_artwork(entry, cache)
                    .debug_selector(move || format!("library-art-{id}"))
                    .size(cover_width)
                    .flex_shrink_0()
                    .border_2()
                    .border_color(rgb(if selected { p.primary } else { p.border }))
                    .group_hover("library-card", move |s| s.border_color(rgb(p.primary)))
                    .when(self.library.checked_games.is_some(), |art| {
                        art.child(self.game_checkbox(entry, cx).absolute().top_2().right_2())
                    }),
            )
            .child(
                div()
                    .px_1()
                    .pt_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .h(px(36.))
                            .line_clamp(2)
                            .font(self.fonts.semibold())
                            .child(entry.title.clone()),
                    )
                    .child(
                        self.library_badges(entry)
                            .debug_selector(move || format!("library-badges-{id}")),
                    ),
            )
    }

    pub(super) fn library_artwork(
        &self,
        entry: &Entry,
        cache: Option<Entity<RetainAllImageCache>>,
    ) -> Div {
        let p = self.library_palette();
        let cover = div()
            .rounded_md()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(p.border));
        if let Some(path) = &entry.thumbnail {
            return cover
                .bg(rgb(p.inset_surface()))
                .when_some(cache, Div::image_cache)
                .child(
                    gpui::img(path.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                );
        }
        let tone = TONES[entry.game.id.rem_euclid(TONES.len() as i64) as usize];
        let disc = div()
            .absolute()
            .w(relative(0.91))
            .aspect_square()
            .right(relative(-0.23))
            .bottom(relative(-0.20))
            .rounded_full()
            .border_1()
            .border_color(rgba(0xffffff25))
            .bg(rgba(0xffffff08))
            .flex()
            .justify_center()
            .items_center()
            .child(
                div()
                    .rounded_full()
                    .size(relative(0.85))
                    .border_1()
                    .border_color(rgba(0xffffff18))
                    .flex()
                    .justify_center()
                    .items_center()
                    .child(
                        div()
                            .rounded_full()
                            .size(relative(0.24))
                            .border_1()
                            .border_color(rgba(0xffffff25)),
                    ),
            );
        cover
            .bg(rgb(tone))
            .p_3()
            .flex()
            .flex_col()
            .justify_between()
            .child(disc)
            .child(
                div()
                    .relative()
                    .text_size(px(9.))
                    .text_color(rgb(0xe3e3dc))
                    .font(self.fonts.medium())
                    .child("PLAYSTATION · PS1"),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(21.))
                            .line_height(px(24.))
                            .line_clamp(2)
                            .font(self.fonts.title())
                            .text_color(rgb(0xffffff))
                            .child(entry.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(9.))
                            .text_color(rgb(0xe3e3dc))
                            .child(format!(
                                "{} · DISC {:02}",
                                self.tr(Text::LibraryMissingCover),
                                entry.disc_count().max(1)
                            )),
                    ),
            )
    }

    pub(super) fn library_badges(&self, entry: &Entry) -> Div {
        let p = self.library_palette();
        let badge = |text: String, color: u32, border: u32| {
            div()
                .flex_shrink_0()
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(border))
                .px(px(5.))
                .py(px(1.))
                .text_size(px(10.))
                .line_height(px(13.))
                .text_color(rgb(color))
                .child(text)
        };
        let discs = entry.disc_count();
        div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .items_center()
            .child(badge(
                entry
                    .region
                    .clone()
                    .unwrap_or_else(|| self.tr(Text::LibraryUnknown).into()),
                p.muted,
                p.border,
            ))
            .when(discs > 1, |row| {
                row.child(badge(
                    self.tr(Text::LibraryDiscCount)
                        .replace("{count}", &discs.to_string()),
                    p.muted,
                    p.border,
                ))
            })
            .when(entry.scrape_active, |row| {
                row.child(badge(
                    self.tr(Text::LibraryScrapePending).into(),
                    p.warning_text(),
                    p.warning_border(),
                ))
            })
            .when(entry.pending, |row| {
                row.child(badge(
                    self.tr(Text::LibraryPending).into(),
                    p.info_text(),
                    p.info_border(),
                ))
            })
            .when(
                !entry.game.locations.is_empty()
                    && entry.game.locations.iter().all(|loc| !loc.available),
                |row| {
                    row.child(badge(
                        self.tr(Text::LibraryMissing).into(),
                        p.on_error,
                        p.error_border(),
                    ))
                },
            )
    }

    pub(super) fn library_list_header(&self, window: &Window) -> Div {
        let p = self.library_palette();
        let narrow = window.viewport_size().width <= px(768.);
        let headings = [
            Text::LibraryGameColumn,
            Text::LibraryRegionStatus,
            Text::LibraryDate,
            Text::LibraryPlayed,
            Text::LibraryTime,
        ];
        div()
            .debug_selector(|| "library-list-header".into())
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .py_2()
            .px_3()
            .border_1()
            .rounded_t_md()
            .bg(rgb(p.surface))
            .border_color(rgb(p.border))
            .when(self.library.checked_games.is_some(), |row| {
                row.child(div().size(px(26.)).flex_shrink_0())
            })
            .children(
                headings
                    .into_iter()
                    .take(if narrow { 3 } else { 5 })
                    .enumerate()
                    .map(|(index, key)| {
                        label(&p, self.tr(key))
                            .flex_1()
                            .min_w_0()
                            .flex_grow(if index == 0 { 2. } else { 1. })
                    }),
            )
    }

    fn collection_list_row(
        &self,
        entry: &Entry,
        cell: gpui::Stateful<Div>,
        narrow: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.library_palette();
        let selected = self.card_selected(entry.game.id);
        let field = |value: String| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(value)
        };
        let (date_label, date) = entry.publication_date();
        let release_date = field(date.unwrap_or(self.tr(Text::LibraryUnknown)).to_owned())
            .when(date_label == Text::LibraryWorkDate, |cell| {
                cell.flex().flex_col().child(label(&p, self.tr(date_label)))
            });
        cell.items_center()
            .px_3()
            .gap_3()
            .border_b_1()
            .border_color(rgb(p.border))
            .when(selected, |cell| cell.bg(rgb(p.tonal)))
            .hover(move |s| s.bg(rgb(p.tonal_hover)))
            .when(self.library.checked_games.is_some(), |row| {
                row.child(self.game_checkbox(entry, cx))
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex_grow(2.)
                    .text_size(px(13.))
                    .font(self.fonts.semibold())
                    .line_clamp(2)
                    .child(entry.title.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .max_h(px(54.))
                    .overflow_hidden()
                    .child(self.library_badges(entry)),
            )
            .child(release_date)
            .when(!narrow, |row| {
                row.child(field(self.played_at(entry.game.last_played)))
                    .child(field(self.library_duration(entry)))
            })
    }
}
