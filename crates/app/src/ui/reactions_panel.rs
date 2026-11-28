//! The full reactions panel the strip unrolls into (tdesktop's
//! `Reactions::Selector::expand` with an `EmojiListWidget`): where only some
//! reactions are allowed (`RecentReactions` mode) a grid of them growing down
//! from the strip; where custom emoji are allowed too (`FullReactions`) the
//! categories row on top, the search field, the reactions, then the custom
//! emoji sets under their titles. The reactions play their animations on a
//! loop.

use gpui::{
    AnyElement, AppContext, Context, Focusable, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas,
    div, point, prelude::FluentBuilder,
};
use tdr_data::describe::ReactionKey;

use super::reactions::{
    EXPAND_MS, HOVER_SCALE, SCALE_MS, STRIP_EXTEND, STRIP_HEIGHT, STRIP_IMAGE, STRIP_SHADOW,
    STRIP_SIZE, STRIP_SKIP, StripCell, StripState,
};
use super::{MainView, c};
use crate::scale::{self, lg};

/// `emojiPanMinHeight`, `emojiPanMaxHeight`, `emojiPanRadius`, the
/// categories row (`footer`), the list's `header`, `headerLeft`, `headerTop`,
/// `reactPanelEmojiPan.searchMargin` and the search's height, the footer's
/// `iconArea` and `iconWidth`.
const PAN_MIN_HEIGHT: f32 = 278.;
const PAN_MAX_HEIGHT: f32 = 640.;
const PAN_RADIUS: f32 = 8.;
const FOOTER: f32 = 36.;
const HEADER: f32 = 33.;
const HEADER_LEFT: f32 = 14.;
const HEADER_TOP: f32 = 10.;
const SEARCH_MARGIN: [f32; 4] = [1., 10., 2., 6.];
const SEARCH_HEIGHT: f32 = 33.;
const ICON_AREA: f32 = 28.;
const ICON_WIDTH: f32 = 30.;
/// `kCustomSearchLimit`.
const SEARCH_LIMIT: usize = 256;

/// One cell of the list.
#[derive(Clone)]
enum Cell {
    Reaction(usize),
    Custom(i64),
}

/// A section of the list: its title (none for the reactions) and cells.
struct Section {
    title: Option<String>,
    icon: Option<i64>,
    cells: Vec<Cell>,
}

impl MainView {
    /// The strip's expand button (`Selector::expand`): all the reactions, the
    /// search field focused where custom emoji are allowed.
    pub(super) fn expand_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut strip) = self.text_ui.menu.as_mut().and_then(|m| m.strip.take()) else {
            return;
        };
        if strip.expanded.is_none() {
            strip.expanded = Some(std::time::Instant::now());
            strip.hovered = None;
            strip.unhovered = None;
            // The reactions the strip had no room for.
            let side = (lg(STRIP_IMAGE) * HOVER_SCALE * scale::get()).round() as u32;
            for k in strip.cells.len()..strip.keys.len() {
                let (appear, select) = match &strip.keys[k] {
                    ReactionKey::Emoji(e) => self.reactions_ui.strip_clips(e, side),
                    _ => (None, None),
                };
                strip.cells.push(StripCell::new(appear, select));
            }
            if strip.custom_allowed {
                self.ensure_emoji_sets();
                self.refresh_emoji_keywords();
                self.ensure_reaction_search(cx);
                if let Some(field) = &self.reactions_ui.search {
                    field.update(cx, |f, cx| f.set_text(String::new(), cx));
                    window.focus(&field.focus_handle(cx));
                }
            }
        }
        if let Some(open) = self.text_ui.menu.as_mut() {
            open.strip = Some(strip);
        }
    }

    fn ensure_reaction_search(&mut self, cx: &mut Context<Self>) {
        if self.reactions_ui.search.is_some() {
            return;
        }
        let palette = self.palette();
        let field = cx.new(|cx| {
            let mut field = crate::text_input::TextInput::new("Search", palette, cx);
            field.set_single_line(true);
            field
        });
        cx.observe(&field, |this, field, cx| {
            let query = field.read(cx).text();
            if query != this.reactions_ui.query {
                this.reactions_ui.query = query;
                if let Some(strip) = this.menu_strip_mut() {
                    strip
                        .panel_scroll
                        .set_offset(point(gpui::px(0.), gpui::px(0.)));
                }
            }
            cx.notify();
        })
        .detach();
        self.reactions_ui.search = Some(field);
    }

    /// The list's sections: the reactions, then (custom emoji allowed) the
    /// installed sets; while searching, what matches (`applyNextSearchQuery`:
    /// the reactions and custom emoji of the emoji found).
    fn panel_sections(&self, strip: &StripState) -> (Vec<Section>, bool) {
        let reactions: Vec<Cell> = (0..strip.keys.len()).map(Cell::Reaction).collect();
        if !strip.custom_allowed {
            return (
                vec![Section {
                    title: None,
                    icon: None,
                    cells: reactions,
                }],
                false,
            );
        }
        let sets: Vec<(String, Option<i64>, Vec<i64>)> = self
            .emoji_panel
            .sets
            .iter()
            .filter(|s| s.installed)
            .map(|s| (s.title().to_string(), s.thumb(), s.emoji.clone()))
            .collect();
        let query = self.reactions_ui.query.trim();
        if query.is_empty() {
            let mut sections = vec![Section {
                title: None,
                icon: None,
                cells: reactions,
            }];
            sections.extend(sets.into_iter().map(|(title, thumb, emoji)| Section {
                title: Some(title),
                icon: thumb.or(emoji.first().copied()),
                cells: emoji.into_iter().map(Cell::Custom).collect(),
            }));
            return (sections, false);
        }
        let (_, found) = crate::emoji_suggest::search_emoji(query);
        let alt_found = |alt: &str| tdr_emoji::find(alt).is_some_and(|(e, _)| found.contains(&e));
        let mut cells: Vec<Cell> = Vec::new();
        let mut custom_seen = std::collections::HashSet::new();
        for (k, key) in strip.keys.iter().enumerate() {
            let hit = match key {
                ReactionKey::Emoji(e) => alt_found(e),
                ReactionKey::Custom(id) => {
                    crate::custom_emoji::info(*id).is_some_and(|i| alt_found(&i.alt))
                }
                ReactionKey::Paid => false,
            };
            if hit {
                if let ReactionKey::Custom(id) = key {
                    custom_seen.insert(*id);
                }
                cells.push(Cell::Reaction(k));
            }
        }
        for (_, _, emoji) in &sets {
            for &id in emoji {
                if cells.len() >= SEARCH_LIMIT {
                    break;
                }
                let hit = crate::custom_emoji::info(id).is_some_and(|i| alt_found(&i.alt));
                if hit && custom_seen.insert(id) {
                    cells.push(Cell::Custom(id));
                }
            }
        }
        (
            vec![Section {
                title: None,
                icon: None,
                cells,
            }],
            true,
        )
    }

    /// The full panel the strip unrolled into (`Selector::paintExpanding`):
    /// it grows down over the menu from the strip (and up by the categories
    /// row), its corners less round with custom emoji, the about text and
    /// the expand button fading away; scrolled when it does not fit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_strip_panel(
        &self,
        strip: &StripState,
        start: std::time::Instant,
        (mx, my, mw, mh): (f32, f32, f32, f32),
        opacity: f32,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        use crate::icons::{Icon, icon};
        let p = self.palette();
        let [_, et, er, _] = STRIP_EXTEND.map(lg);
        let size = lg(STRIP_SIZE);
        let skip = strip.skip_x();
        let w = 2. * skip + strip.columns as f32 * size;
        let strip_h = lg(STRIP_HEIGHT);
        let about = self.strip_about(strip, w, window);
        let about_h = about.as_ref().map_or(0., |a| a.height);
        let x = mx + mw + er - w;
        let box_top = my - et - about_h;
        let box_bottom = my - et + strip_h;
        let skip_y = (strip_h - size) / 2.;
        let custom = strip.custom_allowed;
        let columns = (((w - 2. * skip) / size).floor() as usize).max(1);
        let (sections, searching) = self.panel_sections(strip);
        // The list's layout: the search, then each section's rows.
        let search_h = if custom {
            lg(SEARCH_MARGIN[1]) + lg(SEARCH_HEIGHT) + lg(SEARCH_MARGIN[3])
        } else {
            0.
        };
        let rows_top = if custom { search_h } else { skip_y };
        let mut tops = Vec::with_capacity(sections.len());
        let mut y = rows_top;
        for (i, section) in sections.iter().enumerate() {
            if i > 0 {
                y += lg(HEADER);
            }
            tops.push(y);
            y += section.cells.len().div_ceil(columns) as f32 * size;
        }
        let content_h = y + lg(STRIP_SKIP);
        // `minimalHeight`, or the menu's height up to `emojiPanMaxHeight`.
        let grid_h = sections
            .first()
            .map_or(0., |s| s.cells.len().div_ceil(columns) as f32 * size);
        let minimal = skip_y + grid_h.min(lg(PAN_MIN_HEIGHT)) + lg(PAN_RADIUS) + lg(STRIP_SKIP);
        // `_topAddOnExpand`: room for the categories row over the strip.
        let top_add = if custom {
            (lg(FOOTER) - about_h).max(0.)
        } else {
            0.
        };
        let final_top = box_top - top_add;
        let below = f32::from(window.viewport_size().height) - lg(STRIP_SHADOW[3]);
        let final_h = if custom {
            lg(PAN_MAX_HEIGHT).min((my + mh - final_top).max(minimal + lg(FOOTER)))
        } else {
            minimal
        }
        .min(below - final_top)
        .max(box_bottom - box_top);
        // `updateExpandingRects`.
        let ms = start.elapsed().as_secs_f32() * 1000.;
        let t = (ms / EXPAND_MS).min(1.);
        let e = crate::anim::ease_out_circ(t);
        let top = box_top - top_add * e;
        let bottom = box_bottom + (final_top + final_h - box_bottom) * e;
        let h = bottom - top;
        let radius = if custom {
            strip_h / 2. + (lg(PAN_RADIUS) - strip_h / 2.) * t
        } else {
            strip_h / 2.
        };
        let full = (ms / (EXPAND_MS + SCALE_MS)).min(1.);
        // The categories row (`rects.categories`) and the list under it.
        let categories_from = about_h * (1. - e);
        let list_top = if custom {
            about_h + (lg(FOOTER) - about_h) * e
        } else {
            about_h * (1. - e)
        };
        let list_h = final_h - if custom { lg(FOOTER) } else { 0. };
        let header_fg = c(p.emoji_pan_header_fg);
        let custom_side = crate::custom_emoji::size_large();
        let reaction_side = (lg(STRIP_IMAGE) * scale::get()).round() as u32;
        let mut content = div().relative().w(gpui::px(w)).h(gpui::px(content_h));
        let cell_div = |id: usize, left: f32, top: f32| {
            div()
                .id(("panel-cell", id))
                .absolute()
                .left(gpui::px(left))
                .top(gpui::px(top))
                .size(gpui::px(size))
                .cursor_pointer()
        };
        let mut index = 0;
        for (s, section) in sections.iter().enumerate() {
            let rows = tops[s];
            if let Some(title) = &section.title {
                content = content.child(
                    div()
                        .absolute()
                        .left(gpui::px(lg(HEADER_LEFT)))
                        .top(gpui::px(rows - lg(HEADER) + lg(HEADER_TOP)))
                        .w(gpui::px(w - 2. * lg(HEADER_LEFT)))
                        .text_size(scale::font(13.).size)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(header_fg)
                        .child(crate::one_line::one_line(title.clone())),
                );
            }
            for (i, cell) in section.cells.iter().enumerate() {
                let (col, row) = (i % columns, i / columns);
                let (left, top) = (skip + col as f32 * size, rows + row as f32 * size);
                index += 1;
                let click = view.clone();
                match cell.clone() {
                    Cell::Reaction(k) => {
                        let key = strip.keys[k].clone();
                        let select = match &key {
                            ReactionKey::Emoji(e) => self
                                .reactions_ui
                                .available
                                .iter()
                                .find(|r| &r.emoticon == e)
                                .and_then(|r| r.select.as_ref())
                                .map(|d| d.id),
                            _ => None,
                        };
                        let emoji = match &key {
                            ReactionKey::Emoji(e) => tdr_emoji::find(e).map(|(e, _)| e),
                            _ => None,
                        };
                        let bounds_out = strip.cell_bounds.clone();
                        let paint_key = key.clone();
                        content = content.child(
                            cell_div(index, left, top)
                                .child(
                                    canvas(
                                        move |bounds, _, _| {
                                            let mut all = bounds_out.borrow_mut();
                                            if all.len() <= k {
                                                all.resize(k + 1, gpui::Bounds::default());
                                            }
                                            all[k] = bounds;
                                        },
                                        move |bounds, _, window, _| {
                                            // On a loop (a custom emoji of its
                                            // select animation).
                                            let id = match (&paint_key, select) {
                                                (ReactionKey::Custom(id), _) => Some(*id),
                                                (_, Some(id)) => Some(id),
                                                _ => None,
                                            };
                                            let s = scale::get();
                                            match id {
                                                Some(id) => {
                                                    let side = reaction_side as f32 / s;
                                                    let at = point(
                                                        bounds.origin.x
                                                            + gpui::px(
                                                                ((size - side) / 2.).floor(),
                                                            ),
                                                        bounds.origin.y
                                                            + gpui::px(
                                                                ((size - side) / 2.).floor(),
                                                            ),
                                                    );
                                                    crate::custom_emoji::paint(
                                                        id,
                                                        at,
                                                        reaction_side,
                                                        gpui::Hsla::default(),
                                                        window,
                                                    );
                                                }
                                                None => {
                                                    if let Some(emoji) = emoji {
                                                        let side = crate::emoji::size_large();
                                                        let e = side as f32 / s;
                                                        let at = point(
                                                            bounds.origin.x
                                                                + gpui::px(
                                                                    ((size - e) / 2.).floor(),
                                                                ),
                                                            bounds.origin.y
                                                                + gpui::px(
                                                                    ((size - e) / 2.).floor(),
                                                                ),
                                                        );
                                                        crate::emoji::paint(
                                                            emoji, at, side, window,
                                                        );
                                                    }
                                                }
                                            }
                                        },
                                    )
                                    .size_full(),
                                )
                                .on_click(move |_, _, cx| {
                                    let key = key.clone();
                                    let _ = click.update(cx, |this, cx| {
                                        this.strip_chosen(Some(key), k);
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    Cell::Custom(id) => {
                        content = content.child(
                            cell_div(index, left, top)
                                .child(
                                    canvas(
                                        |_, _, _| {},
                                        move |bounds, _, window, _| {
                                            let side = custom_side as f32 / scale::get();
                                            let at = point(
                                                bounds.origin.x
                                                    + gpui::px(((size - side) / 2.).floor()),
                                                bounds.origin.y
                                                    + gpui::px(((size - side) / 2.).floor()),
                                            );
                                            crate::custom_emoji::paint(
                                                id,
                                                at,
                                                custom_side,
                                                gpui::Hsla::default(),
                                                window,
                                            );
                                        },
                                    )
                                    .size_full(),
                                )
                                .on_click(move |_, _, cx| {
                                    let _ = click.update(cx, |this, cx| {
                                        this.strip_chosen(
                                            Some(ReactionKey::Custom(id)),
                                            usize::MAX,
                                        );
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                }
            }
        }
        // The search field (`TabbedSearch`) at the list's top.
        let search = self
            .reactions_ui
            .search
            .clone()
            .filter(|_| custom)
            .map(|field| {
                let [ml, mt, mr, _] = SEARCH_MARGIN.map(lg);
                let left = lg(STRIP_SKIP) + ml;
                let pill_w = w - 2. * lg(STRIP_SKIP) - ml - mr;
                let focus = field.clone();
                div()
                    .id("reaction-search")
                    .absolute()
                    .left(gpui::px(left))
                    .top(gpui::px(mt))
                    .w(gpui::px(pill_w))
                    .h(gpui::px(lg(SEARCH_HEIGHT)))
                    .rounded(gpui::px(lg(SEARCH_HEIGHT) / 2.))
                    .bg(c(p.window_bg_over))
                    .flex()
                    .items_center()
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&focus.focus_handle(cx));
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(gpui::px(lg(SEARCH_HEIGHT)))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(Icon::EmojiSearch, p.emoji_icon_fg, window)),
                    )
                    .child(div().flex_1().h_full().flex().items_center().child(field))
            });
        // The categories row: the reactions, then each set (`fillIcons`); the
        // one at the list's top is marked.
        let footer = (custom && !searching).then(|| {
            let scroll = -f32::from(strip.panel_scroll.offset().y);
            let current = tops
                .iter()
                .rposition(|t| *t - if *t > rows_top { lg(HEADER) } else { 0. } <= scroll + 1.)
                .unwrap_or(0);
            let icon_w = lg(ICON_WIDTH);
            let area = lg(ICON_AREA);
            let mut row = div()
                .absolute()
                .left_0()
                .top(gpui::px(categories_from))
                .w(gpui::px(w))
                .h(gpui::px((list_top - categories_from).max(0.)))
                .overflow_hidden();
            let mut left = lg(STRIP_SKIP);
            for (s, section) in sections.iter().enumerate() {
                let section_top = if s == 0 { 0. } else { tops[s] - lg(HEADER) };
                let glyph: AnyElement = match section.icon {
                    None => icon(
                        Icon::EmojiRecent,
                        if s == current {
                            p.emoji_sub_icon_fg_active
                        } else {
                            p.emoji_icon_fg
                        },
                        window,
                    )
                    .into_any_element(),
                    Some(id) => canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let side = crate::emoji::size_large();
                            let e = side as f32 / scale::get();
                            let at = point(
                                bounds.origin.x + (bounds.size.width - gpui::px(e)) / 2.,
                                bounds.origin.y + (bounds.size.height - gpui::px(e)) / 2.,
                            );
                            crate::custom_emoji::paint(id, at, side, gpui::Hsla::default(), window);
                        },
                    )
                    .size_full()
                    .into_any_element(),
                };
                let scroll_view = view.clone();
                row = row.child(
                    div()
                        .id(("panel-footer", s))
                        .absolute()
                        .left(gpui::px(left))
                        .top(gpui::px((lg(FOOTER) - area) / 2.))
                        .w(gpui::px(icon_w))
                        .h(gpui::px(area))
                        .rounded(gpui::px(lg(PAN_RADIUS)))
                        .when(s == current, |d| d.bg(c(p.window_bg_over)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(glyph)
                        .on_click(move |_, _, cx| {
                            let _ = scroll_view.update(cx, |this, cx| {
                                if let Some(strip) = this.menu_strip_mut() {
                                    strip
                                        .panel_scroll
                                        .set_offset(point(gpui::px(0.), gpui::px(-section_top)));
                                }
                                cx.notify();
                            });
                        }),
                );
                left += icon_w;
                if left > w {
                    break;
                }
            }
            row
        });
        // `paintFadingExpandIcon`: where the strip's last cell was, shrinking.
        let fading = (full < 1.).then(|| {
            let sub = size / 3. * full;
            div()
                .absolute()
                .left(gpui::px(w - size - skip + sub))
                .top(gpui::px(box_top - top + about_h + skip_y + sub))
                .size(gpui::px(size - 2. * sub))
                .opacity(1. - full)
                .bg(c(p.emoji_pan_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    Icon::ReactionsExpandPanel,
                    p.window_sub_text_fg,
                    window,
                ))
        });
        // The about text goes (`(1 - progress)²`).
        let about_el = about.filter(|_| full < 1.).map(|a| {
            div()
                .absolute()
                .left_0()
                .top(gpui::px(box_top - top))
                .w(gpui::px(w))
                .opacity((1. - full) * (1. - full))
                .child(Self::render_strip_about(a, w, p, window, view))
        });
        let shadow_line = custom.then(|| {
            div()
                .absolute()
                .left_0()
                .top(gpui::px(list_top))
                .w(gpui::px(w))
                .h(gpui::px(lg(1.)))
                .bg(c(p.shadow_fg))
        });
        div()
            .id("reaction-panel")
            .absolute()
            .left(gpui::px(x))
            .top(gpui::px(top))
            .w(gpui::px(w))
            .h(gpui::px(h))
            .opacity(opacity)
            .occlude()
            .rounded(gpui::px(radius))
            .bg(c(p.emoji_pan_bg))
            .shadow(vec![gpui::BoxShadow {
                color: c(p.shadow_fg).into(),
                offset: point(gpui::px(0.), gpui::px(lg(1.))),
                blur_radius: gpui::px(lg(STRIP_SHADOW[0])),
                spread_radius: gpui::px(0.),
            }])
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .rounded(gpui::px(radius))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("reaction-panel-scroll")
                            .absolute()
                            .left_0()
                            .top(gpui::px(list_top))
                            .w(gpui::px(w))
                            .h(gpui::px(list_h))
                            .overflow_y_scroll()
                            .track_scroll(&strip.panel_scroll)
                            .child(content.children(search)),
                    )
                    .children(footer)
                    .children(shadow_line)
                    .children(about_el)
                    .children(fading),
            )
            .into_any_element()
    }
}
