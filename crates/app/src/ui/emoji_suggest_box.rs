//! The emoji suggestions over the message field (tdesktop's `SuggestionsWidget`
//! in an `InnerDropdown`, `SuggestionsController::updateGeometry`): a row of
//! emoji at the query, scrolled when there are more than seven, chosen by mouse
//! or keys (the field forwards them, `crate::emoji_suggest`).

use std::time::Instant;

use gpui::{
    AnyElement, Context, Focusable, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, prelude::FluentBuilder, px,
};

use super::{MainView, c};
use crate::emoji_suggest::{self, Query, Row};
use crate::scale::{self, lg, px as gpx};

/// `emojiSuggestionsDropdown` (`defaultInnerDropdown`): the shadow's margin,
/// `emojiColorsPadding` above and below, the side paddings.
const DROPDOWN_PADDING: f32 = 10.;
const SCROLL_MARGIN: f32 = 8.;
const SIDE_PADDING: f32 = 8.;
/// `InnerDropdown`'s `showDuration` / `duration`.
const SHOW_MS: f32 = 200.;
const HIDE_MS: f32 = 150.;
/// `emojiSuggestionsFadeAfter`, and the fade's width.
const FADE_AFTER: f32 = 20.;
const FADE_WIDTH: f32 = 20.;

impl MainView {
    /// Premium emoji are locked here: no Premium and not Saved Messages.
    pub(super) fn custom_emoji_locked(&self, cx: &Context<Self>) -> bool {
        let premium = self
            .accounts
            .read(cx)
            .active()
            .and_then(|s| s.me.as_ref())
            .is_some_and(|m| m.premium);
        let saved = self
            .selected
            .and_then(|peer| self.chat_item(peer))
            .is_some_and(|c| c.is_saved);
        !premium && !saved
    }

    /// Each frame: the recent emoji for the field, the focus, a delayed word,
    /// and the rows of a new query.
    pub(super) fn update_emoji_suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::emoji::sync_recent(&self.emoji_panel.recent);
        let focused = self.input.focus_handle(cx).is_focused(window);
        let suggest = self.emoji_suggest.clone();
        let mut s = suggest.borrow_mut();
        s.focused = focused;
        if let Some(due) = s.exact_due {
            if s.take_due() {
                // Shown below.
            } else {
                Self::wake_after(due.saturating_duration_since(Instant::now()), cx);
            }
        }
        if s.dirty || s.refreshed {
            let query = s.query.clone();
            drop(s);
            let rows = self.suggestion_rows(&query, cx);
            suggest.borrow_mut().set_rows(rows);
            s = suggest.borrow_mut();
        }
        if s.animating()
            || s.toggled
                .is_some_and(|t| t.elapsed().as_secs_f32() * 1000. < SHOW_MS)
        {
            window.request_animation_frame();
        }
    }

    /// `getRowsByQuery` with `queryMine` and `appendCustom`.
    fn suggestion_rows(&mut self, query: &Query, cx: &mut Context<Self>) -> Vec<Row> {
        let recent = crate::emoji::recent_list();
        let rows: Vec<Row> = match query {
            Query::None => return Vec::new(),
            Query::Emoji { emoji, .. } => {
                let row = Row {
                    emoji: *emoji,
                    custom: None,
                    replacement: tdr_emoji::get(*emoji).map_or(String::new(), |e| e.text()),
                };
                return self.custom_rows(&[row], cx);
            }
            Query::Text(text) => {
                let middle = text.starts_with(':');
                let real = if middle { &text[1..] } else { text.as_str() };
                // `:D` and `:-P` only as exact matches.
                let simple = middle
                    && text.encode_utf16().count() <= 2
                    && !text.chars().any(char::is_lowercase);
                let exact = !middle || simple;
                emoji_suggest::query(real, exact, &recent)
                    .into_iter()
                    .map(|f| Row {
                        emoji: crate::emoji::variant_of(f.emoji),
                        custom: None,
                        replacement: f.label,
                    })
                    .collect()
            }
        };
        let custom = self.custom_rows(&rows, cx);
        let mut rows = rows;
        rows.extend(custom);
        rows
    }

    /// `lookupCustom`: custom emoji of the installed sets for the rows' emoji, in
    /// the rows' order; without Premium a set stops at its first premium emoji.
    fn custom_rows(&mut self, rows: &[Row], cx: &mut Context<Self>) -> Vec<Row> {
        if rows.is_empty() {
            return Vec::new();
        }
        self.ensure_emoji_sets();
        let locked = self.custom_emoji_locked(cx);
        let mut found: Vec<(usize, Row)> = Vec::new();
        for set in self.emoji_panel.sets.iter().filter(|s| s.installed) {
            for &id in &set.emoji {
                let Some(info) = crate::custom_emoji::info(id) else {
                    continue;
                };
                if locked && !info.free {
                    break;
                }
                let Some((emoji, _)) = tdr_emoji::find(&info.alt) else {
                    continue;
                };
                let original = tdr_emoji::get(emoji).map_or(emoji, |e| e.original());
                let position = rows.iter().position(|r| {
                    tdr_emoji::get(r.emoji).map_or(r.emoji, |e| e.original()) == original
                });
                if let Some(at) = position {
                    found.push((
                        at,
                        Row {
                            emoji,
                            custom: Some(id),
                            replacement: rows[at].replacement.clone(),
                        },
                    ));
                }
            }
        }
        found.sort_by_key(|f| f.0);
        found.into_iter().map(|f| f.1).collect()
    }

    /// The row over the field, at the query.
    pub(super) fn render_emoji_suggestions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.palette();
        let s = self.emoji_suggest.borrow();
        if !s.focused || s.rows.is_empty() {
            return None;
        }
        let opacity = match s.toggled {
            Some(t) => {
                let ms = t.elapsed().as_secs_f32() * 1000.;
                if s.shown {
                    (ms / SHOW_MS).min(1.)
                } else {
                    1. - (ms / HIDE_MS).min(1.)
                }
            }
            None => f32::from(s.shown),
        };
        if opacity <= 0. {
            return None;
        }
        let one = lg(emoji_suggest::SIZE);
        let count = s.rows.len();
        let scrolled = count > emoji_suggest::MAX_NON_SCROLLED;
        let inner_w = if scrolled {
            lg(emoji_suggest::SCROLLED_WIDTH)
        } else {
            count as f32 * one
        };
        let pad = lg(DROPDOWN_PADDING);
        let w = inner_w + 2. * lg(SIDE_PADDING) + 2. * pad;
        let h = one + 2. * lg(SCROLL_MARGIN) + 2. * pad;
        let caret = self.input.read(cx).caret_bounds(s.anchor)?;
        let (cx0, cy0) = (f32::from(caret.left()), f32::from(caret.top()));
        let shift = (w - 2. * pad).min(one) / 2.;
        let mut x = cx0 - (pad + lg(SIDE_PADDING) + shift);
        let mut y = cy0 - h;
        let window_w = f32::from(window.viewport_size().width);
        x = x.min(window_w - w).max(0.);
        if y < lg(super::TITLE_HEIGHT) {
            y = cy0 + f32::from(caret.size.height);
        }
        let scroll = s.scroll_now() * one;
        let scroll_max = s.scroll_max * one;
        let selected = s.selected_now();
        let rows = s.rows.clone();
        drop(s);

        let over_bg = c(p.window_bg_over);
        let menu_bg = c(p.menu_bg);
        let text_fg = c(p.window_fg);
        let esize = crate::emoji::size_large() as f32 / scale::get();
        let custom_side = crate::custom_emoji::size_large();
        let cells_x = pad + lg(SIDE_PADDING);
        let cells_y = pad + lg(SCROLL_MARGIN);
        let painted_rows = rows.clone();
        let cells = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let left = bounds.origin.x - gpx(scroll);
                let top = bounds.origin.y;
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    if selected > -1. {
                        window.paint_quad(
                            gpui::fill(
                                gpui::Bounds::new(
                                    point(left + gpx(selected * one), top),
                                    gpui::size(gpx(one), gpx(one)),
                                ),
                                over_bg,
                            )
                            .corner_radii(gpui::Corners::all(px(6.))),
                        );
                    }
                    for (i, row) in painted_rows.iter().enumerate() {
                        let x = i as f32 * one - scroll;
                        if x + one < 0. || x > f32::from(bounds.size.width) {
                            continue;
                        }
                        let cell = point(left + gpx(i as f32 * one), top);
                        match row.custom {
                            Some(id) => {
                                let side = custom_side as f32 / scale::get();
                                let at = point(
                                    cell.x + gpx(((one - side) / 2.).floor()),
                                    cell.y + gpx(((one - side) / 2.).floor()),
                                );
                                crate::custom_emoji::paint(
                                    id,
                                    at,
                                    custom_side,
                                    text_fg.into(),
                                    window,
                                );
                            }
                            None => {
                                let at = point(
                                    cell.x + gpx(((one - esize) / 2.).floor()),
                                    cell.y + gpx(((one - esize) / 2.).floor()),
                                );
                                crate::emoji::paint(
                                    row.emoji,
                                    at,
                                    crate::emoji::size_large(),
                                    window,
                                );
                            }
                        }
                    }
                });
            },
        )
        .size_full();
        // `paintFadings`.
        let fade = |right: bool, alpha: f32| {
            let mut transparent = menu_bg;
            transparent.a = 0.;
            let (from, to) = if right {
                (transparent, menu_bg)
            } else {
                (menu_bg, transparent)
            };
            div()
                .absolute()
                .top(gpx(cells_y))
                .when(right, |d| d.left(gpx(cells_x + inner_w - lg(FADE_WIDTH))))
                .when(!right, |d| d.left(gpx(cells_x)))
                .w(gpx(lg(FADE_WIDTH)))
                .h(gpx(one))
                .opacity(alpha)
                .bg(gpui::linear_gradient(
                    90.,
                    gpui::linear_color_stop(from, 0.),
                    gpui::linear_color_stop(to, 1.),
                ))
        };
        let left_alpha = (scroll / lg(FADE_AFTER)).clamp(0., 1.);
        let right_alpha = ((scroll_max - scroll) / lg(FADE_AFTER)).clamp(0., 1.);
        let hit_rows = rows.len();
        Some(
            div()
                .id("emoji-suggestions")
                .absolute()
                .left(gpx(x))
                .top(gpx(y))
                .w(gpx(w))
                .h(gpx(h))
                .opacity(opacity)
                .occlude()
                .child(
                    div()
                        .absolute()
                        .left(gpx(pad))
                        .top(gpx(pad))
                        .w(gpx(w - 2. * pad))
                        .h(gpx(h - 2. * pad))
                        .rounded(px(6.))
                        .bg(menu_bg)
                        .shadow(super::layers::box_shadow()),
                )
                .child(
                    div()
                        .id("emoji-suggestions-cells")
                        .absolute()
                        .left(gpx(cells_x))
                        .top(gpx(cells_y))
                        .w(gpx(inner_w))
                        .h(gpx(one))
                        .cursor_pointer()
                        .child(cells)
                        .on_mouse_move(cx.listener(
                            move |this, e: &gpui::MouseMoveEvent, window, cx| {
                                let _ = window;
                                this.suggest_hover(e.position, cells_x + x, hit_rows, cx);
                            },
                        ))
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if !*hovered {
                                this.emoji_suggest.borrow_mut().set_selected(None, false);
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let mut s = this.emoji_suggest.borrow_mut();
                                s.pressed = s.selected;
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                let row = {
                                    let mut s = this.emoji_suggest.borrow_mut();
                                    let pressed = s.pressed.take();
                                    pressed
                                        .filter(|p| Some(*p) == s.selected)
                                        .and_then(|p| s.rows.get(p).cloned())
                                };
                                if let Some(row) = row {
                                    let input = this.input.clone();
                                    input.update(cx, |i, cx| i.choose_suggestion(&row, cx));
                                    window.focus(&input.focus_handle(cx));
                                }
                            }),
                        )
                        .on_scroll_wheel(cx.listener(
                            |this, e: &gpui::ScrollWheelEvent, window, cx| {
                                let d = e.delta.pixel_delta(window.line_height());
                                let delta = if d.x.abs() > d.y.abs() { d.x } else { d.y };
                                let mut s = this.emoji_suggest.borrow_mut();
                                let one = lg(emoji_suggest::SIZE);
                                s.scroll =
                                    (s.scroll - f32::from(delta) / one).clamp(0., s.scroll_max);
                                s.scroll_from = (s.scroll, None);
                                cx.notify();
                            },
                        )),
                )
                .when(scrolled, |d| {
                    d.child(fade(false, left_alpha))
                        .child(fade(true, right_alpha))
                })
                .into_any_element(),
        )
    }

    /// `selectByMouse`.
    fn suggest_hover(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        left: f32,
        rows: usize,
        cx: &mut Context<Self>,
    ) {
        let mut s = self.emoji_suggest.borrow_mut();
        let one = lg(emoji_suggest::SIZE);
        let local = f32::from(position.x) - left + s.scroll_now() * one;
        let index = (local / one).floor();
        let selected = (index >= 0. && (index as usize) < rows).then_some(index as usize);
        if selected != s.selected {
            s.set_selected(selected, false);
            cx.notify();
        }
    }

    /// The keywords of a language came (`langPackRefreshed`).
    pub(super) fn emoji_keywords_came(&mut self, cx: &mut Context<Self>) {
        self.emoji_suggest.borrow_mut().refreshed = true;
        cx.notify();
    }

    /// `handleTextChange`'s `emojiKeywords().refresh()`.
    pub(super) fn refresh_emoji_keywords(&mut self) {
        if let Some((languages, versions)) = emoji_suggest::refresh_request() {
            let _ = self
                .commands
                .send(crate::bridge::Command::RefreshEmojiKeywords {
                    languages,
                    versions,
                });
        }
    }
}
