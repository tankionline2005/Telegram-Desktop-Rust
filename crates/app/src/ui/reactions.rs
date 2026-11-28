//! Reactions under a message (tdesktop's `HistoryView::Reactions::InlineList`):
//! a row of pills, each with its reaction's icon and the count or the userpics
//! of who chose it, wrapped to the bubble's width; the message's time takes a
//! skip block after the last pill (`updateSkipBlock`), as after a text. A click
//! chooses the reaction or takes ours back (`toggleReaction`).

use std::collections::HashMap;

use gpui::{
    AnyElement, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, point,
    prelude::FluentBuilder,
};
use tdr_data::describe::ReactionKey;
use tdr_data::tdr_storage::PeerKey;

use super::{MainView, c, text_width};
use crate::bridge::{Author, Command, MessageReactions, ReactionButton};
use crate::scale::{self, lg, px};

/// `reactionInlinePadding`, `reactionInlineSize`, `reactionInlineImage`,
/// `reactionInlineSkip`, `reactionInlineEmptySkip`, `reactionInlineBetween`,
/// `reactionInlineInBubbleLeft`.
const PADDING: [f32; 4] = [5., 2., 7., 2.];
const SIZE: f32 = 18.;
const IMAGE: f32 = 32.;
const SKIP: f32 = 3.;
const EMPTY_SKIP: f32 = 2.;
const BETWEEN: f32 = 4.;
const IN_BUBBLE_LEFT: f32 = -3.;
/// `reactionInlineUserpicsPadding` and `reactionInlineUserpics`.
const USERPICS_PADDING: [f32; 4] = [1., 1., 1., 1.];
const USERPIC: f32 = 20.;
const USERPIC_SHIFT: f32 = 7.;
const USERPIC_STROKE: f32 = 1.;
/// `kInNonChosenOpacity`, `kOutNonChosenOpacity`.
const IN_NON_CHOSEN: f32 = 0.12;
const OUT_NON_CHOSEN: f32 = 0.18;
/// `semiboldFont`.
const COUNT_FONT: f32 = 13.;
/// `mediaInBubbleSkip`: between the text and the reactions.
pub(super) const MEDIA_IN_BUBBLE_SKIP: f32 = 5.;

/// The server's reactions as the window needs them.
#[derive(Default)]
pub(super) struct ReactionsUi {
    /// An emoji reaction's icon: its document and whether it is the select
    /// animation (`fromSelectAnimation`: drawn at half size).
    icons: HashMap<String, (i64, bool)>,
    /// The server's list, in its order (the selector).
    pub(super) available: Vec<tdr_data::reactions::Available>,
    /// Our top reactions (`messages.getTopReactions`): the strip's first ones.
    pub(super) top: Vec<ReactionKey>,
    /// The account has Premium (set each frame).
    pub(super) premium: bool,
    asked: bool,
}

/// The pills of a message's reactions laid out at a width
/// (`InlineList::countCurrentSize`).
pub(super) struct ReactionsLayout {
    /// Each pill's place (logical, from the text's left) and size.
    pills: Vec<(f32, f32, f32, f32)>,
    pub(super) height: f32,
}

impl MainView {
    /// Asks for the server's reactions once.
    pub(super) fn ask_reactions(&mut self) {
        if !self.reactions_ui.asked {
            self.reactions_ui.asked = true;
            let _ = self.commands.send(Command::LoadReactions);
        }
    }

    /// The server's reactions came: their icons are custom emoji documents.
    pub(super) fn reactions_came(
        &mut self,
        list: Vec<tdr_data::reactions::Available>,
        top: Vec<ReactionKey>,
    ) {
        self.reactions_ui.top = top;
        self.reactions_ui.icons.clear();
        for r in &list {
            for doc in r.documents() {
                crate::custom_emoji::remember(doc);
            }
            let icon = match (&r.center, &r.select) {
                (Some(center), _) => Some((center.id, false)),
                (None, Some(select)) => Some((select.id, true)),
                _ => None,
            };
            if let Some(icon) = icon {
                self.reactions_ui.icons.insert(r.emoticon.clone(), icon);
            }
        }
        self.reactions_ui.available = list;
    }

    /// The width of one pill (`InlineList::countOptimalSize`).
    fn pill_width(b: &ReactionButton, window: &Window) -> f32 {
        let [pl, _, pr, _] = PADDING;
        match &b.userpics {
            Some(pics) => {
                let n = pics.len().max(1) as f32;
                lg(pl)
                    + lg(SIZE)
                    + lg(USERPICS_PADDING[0])
                    + lg(USERPICS_PADDING[2])
                    + lg(USERPIC)
                    + (n - 1.) * (lg(USERPIC) - lg(USERPIC_SHIFT))
            }
            None if b.count == 0 => lg(pl) + lg(SIZE) + lg(pr) - lg(EMPTY_SKIP),
            None => {
                let text = crate::present::count_short(b.count);
                lg(pl)
                    + lg(SIZE)
                    + lg(SKIP)
                    + lg(pr)
                    + text_width(window, &text, COUNT_FONT, FontWeight::SEMIBOLD).ceil()
            }
        }
    }

    /// The widest the reactions get on one row with the skip block
    /// (`InlineList::maxWidth`), from the text's left.
    pub(super) fn reactions_max_width(r: &MessageReactions, skip_w: f32, window: &Window) -> f32 {
        let mut x = lg(IN_BUBBLE_LEFT);
        for b in &r.buttons {
            x += Self::pill_width(b, window) + lg(BETWEEN);
        }
        x - lg(BETWEEN) + skip_w
    }

    /// `InlineList::countCurrentSize`: rows of pills at `width`, the skip block
    /// after the last one (or below it).
    pub(super) fn layout_reactions(
        r: &MessageReactions,
        width: f32,
        (skip_w, skip_h): (f32, f32),
        window: &Window,
    ) -> ReactionsLayout {
        let left = lg(IN_BUBBLE_LEFT);
        let h = lg(PADDING[1]) + lg(SIZE) + lg(PADDING[3]);
        let (mut x, mut y) = (left, 0.);
        let mut pills = Vec::with_capacity(r.buttons.len());
        for b in &r.buttons {
            let w = Self::pill_width(b, window);
            if x > left && x + w > left + width {
                x = left;
                y += h + lg(BETWEEN);
            }
            pills.push((x, y, w, h));
            x += w + lg(BETWEEN);
        }
        let Some(&(lx, _, lw, _)) = pills.last() else {
            return ReactionsLayout { pills, height: 0. };
        };
        let add = if lx + lw + skip_w > width { skip_h } else { 0. };
        ReactionsLayout {
            pills,
            height: y + h + add,
        }
    }

    /// The pills of a message (`InlineList::paint` in a bubble).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_reactions(
        &self,
        peer: Option<PeerKey>,
        msg_id: i32,
        r: &MessageReactions,
        layout: &ReactionsLayout,
        out: bool,
        live: bool,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let bubble = c(if out { p.msg_out_bg } else { p.msg_in_bg });
        let file_bg = c(if out {
            p.msg_file_out_bg
        } else {
            p.msg_file_in_bg
        });
        let mut row = div().relative().w_full().h(gpui::px(layout.height));
        for (k, (b, &(x, y, w, h))) in r.buttons.iter().zip(&layout.pills).enumerate() {
            let chosen = b.chosen;
            let opacity = if chosen {
                1.
            } else if out {
                OUT_NON_CHOSEN
            } else {
                IN_NON_CHOSEN
            };
            let mut bg = file_bg;
            bg.a *= opacity;
            // What the pill shows over the bubble (the userpics' rings).
            let shown = blend(bubble, file_bg, opacity);
            let text_fg = if chosen {
                c(if out {
                    p.history_file_out_icon_fg
                } else {
                    p.history_file_in_icon_fg
                })
            } else {
                c(if out {
                    p.msg_out_service_fg
                } else {
                    p.msg_in_service_fg
                })
            };
            let id = SharedString::from(format!(
                "reaction-{}-{}-{msg_id}-{k}",
                peer.map_or(0, |p| p.id),
                live as u8
            ));
            let mut ripple_color = text_fg;
            ripple_color.a = 25. / 255.;
            let icon = self.reaction_icon(&b.key, text_fg);
            let count = b
                .userpics
                .is_none()
                .then_some(b.count)
                .filter(|n| *n > 0)
                .map(crate::present::count_short);
            let pics = b
                .userpics
                .as_ref()
                .map(|pics| self.render_userpics_row(pics, shown));
            let key = b.key.clone();
            let ripple_id = id.clone();
            // The pill's place on screen (the ripple starts at the press).
            let pill_bounds = std::rc::Rc::new(std::cell::Cell::new(None));
            let bounds_out = pill_bounds.clone();
            row = row.child(
                div()
                    .id(id.clone())
                    .absolute()
                    .left(gpui::px(x))
                    .top(gpui::px(y))
                    .w(gpui::px(w))
                    .h(gpui::px(h))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(bg)
                    .when(live, |d| d.cursor_pointer())
                    .children(self.ripple_layer_rgba(&id, w, h, ripple_color, false))
                    .child(
                        canvas(
                            move |bounds, _, _| bounds_out.set(Some(bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full(),
                    )
                    .child(icon)
                    .children(count.map(|text| {
                        div()
                            .absolute()
                            .left(gpui::px(lg(PADDING[0]) + lg(SIZE) + lg(SKIP)))
                            .top_0()
                            .h(gpui::px(h))
                            .flex()
                            .items_center()
                            .text_size(scale::font(COUNT_FONT).size)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_fg)
                            .child(text)
                    }))
                    .children(pics.map(|pics| {
                        div()
                            .absolute()
                            .left(gpui::px(
                                lg(PADDING[0]) + lg(SIZE) + lg(USERPICS_PADDING[0]),
                            ))
                            .top(gpui::px(lg(USERPICS_PADDING[1])))
                            .child(pics)
                    }))
                    .when(live, |d| {
                        let (down, up, hover, click) =
                            (view.clone(), view.clone(), view.clone(), view.clone());
                        d.on_mouse_down(MouseButton::Left, move |e, _, cx| {
                            cx.stop_propagation();
                            // The press point inside the pill (it starts at the
                            // middle when the place is not known).
                            let at = pill_bounds
                                .get()
                                .map(|b: gpui::Bounds<gpui::Pixels>| {
                                    (
                                        f32::from(e.position.x - b.origin.x),
                                        f32::from(e.position.y - b.origin.y),
                                    )
                                })
                                .unwrap_or((w / 2., h / 2.));
                            let id = ripple_id.clone();
                            let _ = down.update(cx, |this, cx| {
                                this.press_button(id, at);
                                cx.notify();
                            });
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            let _ = up.update(cx, |this, cx| {
                                this.release_buttons();
                                cx.notify();
                            });
                        })
                        .on_hover(move |hovered, _, cx| {
                            if !*hovered {
                                let _ = hover.update(cx, |this, cx| {
                                    this.release_buttons();
                                    cx.notify();
                                });
                            }
                        })
                        .on_click(move |_, _, cx| {
                            let key = key.clone();
                            let _ = click.update(cx, |this, cx| {
                                if let Some(peer) = peer {
                                    let _ = this.commands.send(Command::ToggleReaction {
                                        peer,
                                        msg_id,
                                        key,
                                    });
                                }
                                cx.notify();
                            });
                        })
                    }),
            );
        }
        row.into_any_element()
    }

    /// The reaction's icon in its pill: an emoji reaction's first frame at
    /// `reactionInlineImage` centered on the `reactionInlineSize` square, a
    /// custom emoji at the custom emoji size in the text's color.
    fn reaction_icon(&self, key: &ReactionKey, text_fg: gpui::Rgba) -> AnyElement {
        let key = key.clone();
        let icon = match &key {
            ReactionKey::Emoji(e) => self.reactions_ui.icons.get(e).copied(),
            _ => None,
        };
        let emoji = match &key {
            ReactionKey::Emoji(e) => tdr_emoji::find(e).map(|(e, _)| e),
            _ => None,
        };
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let s = scale::get();
                let inner = point(
                    bounds.origin.x + gpui::px(lg(PADDING[0])),
                    bounds.origin.y + gpui::px(lg(PADDING[1])),
                );
                match (&key, icon) {
                    (ReactionKey::Custom(id), _) => {
                        let side = crate::custom_emoji::size_normal();
                        let skip =
                            ((lg(crate::emoji::EMOJI_SIZE) * s - side as f32) / 2. / s).floor();
                        let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                        crate::custom_emoji::paint(*id, at, side, text_fg.into(), window);
                    }
                    (_, Some((id, from_select))) => {
                        // `resolveImageFor`: the select animation's frame is drawn
                        // at half size in the middle.
                        let full = lg(IMAGE);
                        let size = if from_select { full / 2. } else { full };
                        let side = (size * s).round() as u32;
                        let skip = (lg(SIZE) - size) / 2.;
                        let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                        crate::custom_emoji::paint_still(id, at, side, window);
                    }
                    (ReactionKey::Emoji(_), None) => {
                        // Until the server's list comes: the emoji itself.
                        if let Some(emoji) = emoji {
                            let side = crate::emoji::size_normal();
                            let size = side as f32 / s;
                            let skip = ((lg(SIZE) - size) / 2.).floor();
                            let at = point(inner.x + gpui::px(skip), inner.y + gpui::px(skip));
                            crate::emoji::paint(emoji, at, side, window);
                        }
                    }
                    (ReactionKey::Paid, _) => {}
                }
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full()
        .into_any_element()
    }

    /// `GenerateUserpicsInRow`: the first on top, each ringed (transparent in
    /// tdesktop: here in the color the pill shows there).
    fn render_userpics_row(&self, authors: &[Author], ring: gpui::Rgba) -> AnyElement {
        let size = lg(USERPIC);
        let step = size - lg(USERPIC_SHIFT);
        let stroke = lg(USERPIC_STROKE);
        let mut row = div().relative().h(gpui::px(size)).w(gpui::px(
            size + authors.len().saturating_sub(1) as f32 * step,
        ));
        for (i, a) in authors.iter().enumerate().rev() {
            let left = i as f32 * step;
            row = row.child(
                div()
                    .absolute()
                    .top(gpui::px(-stroke / 2.))
                    .left(gpui::px(left - stroke / 2.))
                    .size(gpui::px(size + stroke))
                    .rounded_full()
                    .bg(ring)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(USERPIC))
                            .rounded_full()
                            .overflow_hidden()
                            .child(self.userpic(
                                USERPIC,
                                a.key,
                                a.photo,
                                &a.initials,
                                a.color,
                                false,
                            )),
                    ),
            );
        }
        row.into_any_element()
    }
}

/// `color` over `under` at `opacity`.
fn blend(under: gpui::Rgba, color: gpui::Rgba, opacity: f32) -> gpui::Rgba {
    let a = color.a * opacity;
    gpui::Rgba {
        r: under.r * (1. - a) + color.r * a,
        g: under.g * (1. - a) + color.g * a,
        b: under.b * (1. - a) + color.b * a,
        a: 1.,
    }
}

/// `reactStripExtend`, `reactStripHeight`, `reactStripSize`, `reactStripImage`,
/// `reactStripSkip`, `reactStripMinWidth`, `reactStripBubbleRight`,
/// `reactionCornerShadow`, `kHoverScale`, `kHoverScaleDuration`,
/// `kMinNonTransparentColumns`.
pub(super) const STRIP_EXTEND: [f32; 4] = [21., 49., 39., 0.];
const STRIP_HEIGHT: f32 = 40.;
const STRIP_SIZE: f32 = 32.;
const STRIP_IMAGE: f32 = 26.;
const STRIP_SKIP: f32 = 7.;
const STRIP_MIN_WIDTH: f32 = 60.;
const STRIP_BUBBLE_RIGHT: f32 = 20.;
pub(super) const STRIP_SHADOW: [f32; 4] = [4., 8., 4., 8.];
const HOVER_SCALE: f32 = 1.24;
const HOVER_SCALE_MS: f32 = 200.;
const MIN_COLUMNS: usize = 7;

/// The reaction strip over a message's menu (`Reactions::Selector` collapsed,
/// `AttachSelectorToMenu`).
pub(super) struct StripState {
    pub(super) peer: PeerKey,
    pub(super) msg_id: i32,
    /// The reactions it offers, in order (`PossibleItemReactions::recent`).
    pub(super) keys: Vec<ReactionKey>,
    /// The cells shown: reactions, then the expand button when they do not all
    /// fit (`Strip::AddedButton::Expand`).
    pub(super) columns: usize,
    pub(super) expand: bool,
    /// The cell under the mouse and since when (its scale animates).
    pub(super) hovered: Option<(usize, std::time::Instant)>,
    pub(super) unhovered: Option<(usize, std::time::Instant)>,
}

impl StripState {
    pub(super) fn new(peer: PeerKey, msg_id: i32, keys: Vec<ReactionKey>) -> Self {
        Self {
            peer,
            msg_id,
            keys,
            columns: 0,
            expand: false,
            hovered: None,
            unhovered: None,
        }
    }

    /// `Selector::countWidth`: as many cells as fit over the menu (`menu_width`
    /// is the menu's own); returns the width the menu gets (the strip's less
    /// its extends).
    pub(super) fn fit(&mut self, menu_width: f32) -> f32 {
        let [el, _, er, _] = STRIP_EXTEND.map(lg);
        let added = el + er;
        let (size, skip) = (lg(STRIP_SIZE), self.skip_x());
        let desired = menu_width + added;
        let max_width = lg(crate::popup_menu::WIDTH_MAX) + added;
        let desired_columns = (((desired - 2. * skip) / size).ceil() as usize).max(MIN_COLUMNS);
        let possible = desired_columns.min(((max_width - 2. * skip) / size).floor() as usize);
        let max = self.keys.len();
        self.columns = possible.min(max).max(1);
        self.expand = self.columns < max;
        (2. * skip + self.columns as f32 * size).max(desired) - added
    }

    /// `countSkipLeft`.
    fn skip_x(&self) -> f32 {
        let n = self.keys.len() as f32;
        ((lg(STRIP_MIN_WIDTH) - n * lg(STRIP_SIZE)) / 2.).max(lg(STRIP_SKIP))
    }

    /// The scale of cell `k` now (`selectedScale`).
    fn scale(&self, k: usize) -> f32 {
        let progress = |at: std::time::Instant| {
            let t = (at.elapsed().as_secs_f32() * 1000. / HOVER_SCALE_MS).min(1.);
            1. - (1. - t).powi(3)
        };
        match (self.hovered, self.unhovered) {
            (Some((h, at)), _) if h == k => 1. + (HOVER_SCALE - 1.) * progress(at),
            (_, Some((u, at))) if u == k => HOVER_SCALE - (HOVER_SCALE - 1.) * progress(at),
            _ => 1.,
        }
    }

    pub(super) fn animating(&self) -> bool {
        let busy = |t: Option<(usize, std::time::Instant)>| {
            t.is_some_and(|(_, at)| at.elapsed().as_secs_f32() * 1000. < HOVER_SCALE_MS)
        };
        busy(self.hovered) || busy(self.unhovered)
    }
}

impl ReactionsUi {
    /// The strip's icon of an emoji reaction: its select animation (played
    /// under the mouse, its first frame otherwise).
    fn select_icon(&self, emoticon: &str) -> Option<i64> {
        self.available
            .iter()
            .find(|r| r.emoticon == emoticon)
            .and_then(|r| r.select.as_ref().or(r.center.as_ref()))
            .map(|d| d.id)
    }
}

impl MainView {
    /// The reactions a message's strip offers (`LookupPossibleReactions`): the
    /// top ones the server keeps for us, then the rest of its list; premium
    /// ones only with Premium.
    pub(super) fn strip_reactions(&self) -> Vec<ReactionKey> {
        let premium = self.reactions_ui.premium;
        let allowed = |k: &ReactionKey| match k {
            ReactionKey::Emoji(e) => self
                .reactions_ui
                .available
                .iter()
                .find(|r| &r.emoticon == e)
                .is_some_and(|r| !r.inactive && (premium || !r.premium)),
            ReactionKey::Custom(_) => premium,
            ReactionKey::Paid => false,
        };
        let mut keys: Vec<ReactionKey> = self
            .reactions_ui
            .top
            .iter()
            .filter(|k| allowed(k))
            .cloned()
            .collect();
        for r in &self.reactions_ui.available {
            let k = ReactionKey::Emoji(r.emoticon.clone());
            if allowed(&k) && !keys.contains(&k) {
                keys.push(k);
            }
        }
        keys
    }

    /// The strip over the menu's box at (`mx`, `my`) `mw` wide
    /// (`Selector::paintCollapsed`): a rounded bar with a shadow and a tail
    /// towards the menu, a cell for each reaction (its icon grows under the
    /// mouse), the expand arrow last.
    pub(super) fn render_strip(
        &self,
        strip: &StripState,
        (mx, my, mw): (f32, f32, f32),
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        use crate::icons::{Icon, icon, icon_width};
        let p = self.palette();
        let [_, et, er, _] = STRIP_EXTEND.map(lg);
        let size = lg(STRIP_SIZE);
        let skip = strip.skip_x();
        let inner_w = 2. * skip + strip.columns as f32 * size;
        let inner_h = lg(STRIP_HEIGHT);
        // Right-aligned with the menu's extended right edge, `et` above its box.
        let right = mx + mw + er;
        let (x, y) = (right - inner_w, my - et);
        let radius = inner_h / 2.;
        let bubble_w = icon_width(Icon::ReactionsBubble, window);
        let bubble_right = lg(STRIP_BUBBLE_RIGHT).min((inner_w - bubble_w) / 2.);
        let tail_x = inner_w - bubble_right - bubble_w;
        let mut bar = div()
            .id("reaction-strip")
            .absolute()
            .left(gpui::px(x))
            .top(gpui::px(y))
            .w(gpui::px(inner_w))
            .h(gpui::px(inner_h))
            .occlude()
            // The tail under the bar, its shadow first.
            .child(
                div()
                    .absolute()
                    .left(gpui::px(tail_x))
                    .top(gpui::px(inner_h))
                    .child(icon(
                        Icon::ReactionsBubbleShadow,
                        p.window_shadow_fg,
                        window,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(gpui::px(tail_x))
                    .top(gpui::px(inner_h))
                    .child(icon(Icon::ReactionsBubble, p.emoji_pan_bg, window)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .rounded(gpui::px(radius))
                    .bg(c(p.emoji_pan_bg))
                    .shadow(vec![gpui::BoxShadow {
                        color: c(p.window_shadow_fg).into(),
                        offset: point(gpui::px(0.), gpui::px(lg(1.))),
                        blur_radius: gpui::px(lg(4.)),
                        spread_radius: gpui::px(0.),
                    }]),
            );
        let cells = strip.columns;
        for k in 0..cells {
            let expand = strip.expand && k + 1 == cells;
            let key = (!expand).then(|| strip.keys[k].clone());
            let scale = strip.scale(k);
            let hovered = strip.hovered.is_some_and(|(h, _)| h == k);
            let image = key.clone().map(|key| {
                let select = match &key {
                    ReactionKey::Emoji(e) => self.reactions_ui.select_icon(e),
                    _ => None,
                };
                let emoji = match &key {
                    ReactionKey::Emoji(e) => tdr_emoji::find(e).map(|(e, _)| e),
                    _ => None,
                };
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let s = scale::get();
                        let image = lg(STRIP_IMAGE) * scale;
                        let side = (image * s).round() as u32;
                        let w = f32::from(bounds.size.width);
                        let h = f32::from(bounds.size.height);
                        let at = point(
                            bounds.origin.x + gpui::px((w - image) / 2.),
                            bounds.origin.y + gpui::px((h - image) / 2.),
                        );
                        match (&key, select) {
                            (ReactionKey::Custom(id), _) => {
                                crate::custom_emoji::paint(
                                    *id,
                                    at,
                                    side,
                                    gpui::Hsla::default(),
                                    window,
                                );
                            }
                            (_, Some(id)) if hovered => {
                                crate::custom_emoji::paint(
                                    id,
                                    at,
                                    side,
                                    gpui::Hsla::default(),
                                    window,
                                );
                            }
                            (_, Some(id)) => {
                                crate::custom_emoji::paint_still(id, at, side, window);
                            }
                            _ => {
                                if let Some(emoji) = emoji {
                                    let side = crate::emoji::size_large();
                                    let size = side as f32 / s;
                                    let at = point(
                                        bounds.origin.x + gpui::px(((w - size) / 2.).floor()),
                                        bounds.origin.y + gpui::px(((h - size) / 2.).floor()),
                                    );
                                    crate::emoji::paint(emoji, at, side, window);
                                }
                            }
                        }
                    },
                )
                .size_full()
            });
            let (hover, click) = (view.clone(), view.clone());
            let cell = div()
                .id(("strip-cell", k))
                .absolute()
                .left(gpui::px(skip + k as f32 * size))
                .top(gpui::px((inner_h - size) / 2.))
                .size(gpui::px(size))
                .cursor_pointer()
                .children(image)
                .when(expand, |d| {
                    d.flex().items_center().justify_center().child(
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(Icon::ReactionsRoundBig, p.window_bg_ripple, window))
                            .child(div().absolute().child(icon(
                                Icon::ReactionsExpandPanel,
                                p.window_sub_text_fg,
                                window,
                            ))),
                    )
                })
                .on_hover(move |over, _, cx| {
                    let over = *over;
                    let _ = hover.update(cx, |this, cx| {
                        if let Some(strip) = this.menu_strip_mut() {
                            let now = std::time::Instant::now();
                            if over {
                                if let Some((h, _)) = strip.hovered
                                    && h != k
                                {
                                    strip.unhovered = Some((h, now));
                                }
                                strip.hovered = Some((k, now));
                            } else if strip.hovered.is_some_and(|(h, _)| h == k) {
                                strip.hovered = None;
                                strip.unhovered = Some((k, now));
                            }
                        }
                        cx.notify();
                    });
                })
                .on_click(move |_, _, cx| {
                    let key = key.clone();
                    let _ = click.update(cx, |this, cx| {
                        this.strip_chosen(key);
                        cx.notify();
                    });
                });
            bar = bar.child(cell);
        }
        bar.into_any_element()
    }
}
