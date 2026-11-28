//! The reaction button at a message's corner (tdesktop's `Reactions::Manager`
//! and `Reactions::Button`, the `cornerReaction` setting): it shows 300 ms
//! after the mouse stops over a bubble, grows when the mouse comes near and,
//! with the mouse inside, unrolls into a column of all the reactions (up when
//! there is room, scrolled by the wheel). A click chooses the reaction under
//! the mouse; it flies to its pill like one chosen in the strip.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    StatefulInteractiveElement, Styled, WeakEntity, canvas, deferred, div, linear_color_stop,
    linear_gradient, point, prelude::FluentBuilder,
};
use tdr_data::tdr_storage::PeerKey;

use super::reactions::{StripState, paint_reaction_icon};
use super::{MainView, c};
use crate::anim::{self, Simple};
use crate::bridge::Command;
use crate::scale::lg;

/// `reactionCornerSize`, `reactionCornerCenter`, `reactionCornerImage`,
/// `reactionCornerActiveAreaPadding`, `reactionCornerAddedHeightMax`,
/// `reactionCornerSkip`, `reactionExpandedSkip`, `reactionGradientSize`,
/// `reactionGradientFadeSize`, `reactionMainAppearShift`,
/// `reactionCollapseFadeThreshold`, `reactionCornerShadow`.
const SIZE: (f32, f32) = (36., 32.);
const CENTER: (f32, f32) = (7., -9.);
const IMAGE: f32 = 22.;
const ACTIVE_PADDING: f32 = 10.;
const ADDED_HEIGHT_MAX: f32 = 100.;
const SKIP: f32 = -4.;
const EXPANDED_SKIP: f32 = 2.;
const GRADIENT_SIZE: f32 = 24.;
const GRADIENT_FADE: f32 = 24.;
const MAIN_APPEAR_SHIFT: f32 = 20.;
const COLLAPSE_FADE_THRESHOLD: f32 = 40.;
const SHADOW: f32 = 4.;
/// `kToggleDuration`, `kActivateDuration`, `kExpandDuration`,
/// `kCollapseDuration`, `kButtonShowDelay`, `kButtonExpandDelay`,
/// `kButtonHideDelay`, `kMaxReactionsScrollAtOnce`.
const TOGGLE_MS: f32 = 120.;
const ACTIVATE_MS: f32 = 150.;
const EXPAND_MS: f32 = 300.;
const COLLAPSE_MS: f32 = 250.;
const SHOW_DELAY: Duration = Duration::from_millis(300);
const EXPAND_DELAY: Duration = Duration::from_millis(25);
const HIDE_DELAY: Duration = Duration::from_millis(300);
const MAX_SCROLL_AT_ONCE: f32 = 2.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hidden,
    Shown,
    Active,
    Inside,
}

/// `Button::ScaleForState`.
fn scale_for(state: State) -> f32 {
    match state {
        State::Hidden => 1. / 3.,
        State::Shown => 2. / 3.,
        State::Active | State::Inside => 1.,
    }
}

/// `Button::OpacityForScale`.
fn opacity_for(scale: f32) -> f32 {
    ((scale - scale_for(State::Hidden)) / (scale_for(State::Shown) - scale_for(State::Hidden)))
        .min(1.)
}

/// The corner buttons of the open chat: the current one, those fading out,
/// and the one about to show.
#[derive(Default)]
pub(super) struct CornerUi {
    current: Option<Corner>,
    hiding: Vec<Corner>,
    pending: Option<(PeerKey, i32, Instant)>,
}

/// One button (`Reactions::Button`).
pub(super) struct Corner {
    peer: PeerKey,
    msg_id: i32,
    state: State,
    final_scale: f32,
    scale: Option<Simple>,
    opacity: Option<Simple>,
    /// The height added to the collapsed button (the column unrolled).
    final_height: f32,
    height: Option<Simple>,
    expanded_added: f32,
    /// The whole column's added height (more than shown scrolls).
    inner_added: f32,
    up: bool,
    scroll: f32,
    /// `CollapseType::Fade` (scrolled far) or `Scroll` (rolls back).
    collapse_fade: bool,
    hide_at: Option<Instant>,
    expand_at: Option<Instant>,
    over_bubble: bool,
    over_active: bool,
    over_inner: bool,
    strip: StripState,
    /// Where the button was painted (its direction depends on the room).
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Corner {
    fn new(strip: StripState) -> Self {
        let n = strip.keys.len() as f32;
        let one = lg(SIZE.1) + lg(SKIP);
        let inner_added = (n - 1.).max(0.) * one + if n > 1. { 2. * lg(EXPANDED_SKIP) } else { 0. };
        Self {
            peer: strip.peer,
            msg_id: strip.msg_id,
            state: State::Hidden,
            final_scale: scale_for(State::Hidden),
            scale: None,
            opacity: None,
            final_height: 0.,
            height: None,
            expanded_added: inner_added.min(lg(ADDED_HEIGHT_MAX)),
            inner_added,
            up: true,
            scroll: 0.,
            collapse_fade: false,
            hide_at: None,
            expand_at: None,
            over_bubble: true,
            over_active: false,
            over_inner: false,
            strip,
            bounds: Default::default(),
        }
    }

    fn added(&self) -> f32 {
        anim::value(&self.height, self.final_height).round()
    }

    fn scroll_max(&self) -> f32 {
        (self.inner_added - self.expanded_added).max(0.)
    }

    fn current_scale(&self) -> f32 {
        anim::value(&self.scale, self.final_scale)
    }

    fn current_opacity(&self) -> f32 {
        anim::value(&self.opacity, opacity_for(scale_for(self.state)))
    }

    fn animating(&self) -> bool {
        let busy = |a: &Option<Simple>| a.is_some_and(|a| a.animating());
        busy(&self.scale) || busy(&self.opacity) || busy(&self.height) || self.strip.scaling()
    }

    fn gone(&self) -> bool {
        self.state == State::Hidden && !self.opacity.is_some_and(|a| a.animating())
    }

    /// `Button::applyState`.
    fn apply_state(&mut self, state: State) {
        if state == State::Hidden {
            self.expand_at = None;
            self.hide_at = None;
        }
        let final_height = match state {
            State::Hidden => self.added(),
            State::Inside => self.expanded_added,
            _ => 0.,
        };
        if self.final_height != final_height {
            if state == State::Hidden {
                self.height = None;
            } else {
                if !self.height.is_some_and(|h| h.animating()) {
                    self.collapse_fade = self.scroll >= lg(COLLAPSE_FADE_THRESHOLD);
                }
                let ms = if state == State::Inside {
                    EXPAND_MS
                } else {
                    COLLAPSE_MS
                };
                self.height = Some(Simple::new(
                    self.added(),
                    final_height,
                    ms,
                    anim::ease_out_circ,
                ));
            }
            self.final_height = final_height;
        }
        if self.state == state {
            return;
        }
        let ms = if state == State::Hidden || self.state == State::Hidden {
            TOGGLE_MS
        } else {
            ACTIVATE_MS
        };
        self.opacity = Some(Simple::new(
            opacity_for(scale_for(self.state)),
            opacity_for(scale_for(state)),
            ms,
            anim::sine_in_out,
        ));
        let final_scale = scale_for(state);
        if state != State::Hidden && self.final_scale != final_scale {
            self.scale = Some(Simple::new(
                self.final_scale,
                final_scale,
                ms,
                anim::sine_in_out,
            ));
            self.final_scale = final_scale;
        }
        self.state = state;
    }

    /// `Button::applyParameters`: the state the mouse asks for.
    fn apply_pointer(&mut self, visible: Option<(f32, f32)>) {
        let inside = self.over_inner || (self.over_active && self.state == State::Inside);
        if self.state != State::Inside
            && !self.height.is_some_and(|h| h.animating())
            && let (Some(b), Some((top, bottom))) = (self.bounds.get(), visible)
        {
            // `updateExpandDirection`: up when it fits above or not below.
            let y = f32::from(b.origin.y);
            let h = f32::from(b.size.height);
            self.up = y - self.expanded_added >= top || y + h + self.expanded_added > bottom;
        }
        let delay_inside = inside && self.state != State::Inside;
        if !delay_inside {
            self.expand_at = None;
        } else if self.expand_at.is_none() {
            self.expand_at = Some(Instant::now() + EXPAND_DELAY);
            crate::repaint::at(Instant::now() + EXPAND_DELAY);
        }
        let was_inside = self.state == State::Inside;
        let state = if inside && !delay_inside {
            State::Inside
        } else if self.over_active || self.over_inner {
            State::Active
        } else {
            State::Shown
        };
        self.apply_state(state);
        let outside = !self.over_bubble && !self.over_inner;
        if outside && self.state == State::Shown {
            if self.hide_at.is_none() {
                let delay = if was_inside {
                    Duration::ZERO
                } else {
                    HIDE_DELAY
                };
                self.hide_at = Some(Instant::now() + delay);
                crate::repaint::at(Instant::now() + delay);
            }
        } else {
            self.hide_at = None;
        }
    }
}

impl MainView {
    /// The visible part of the history (the column's direction depends on it).
    fn corner_visible(&self) -> Option<(f32, f32)> {
        let b = self.messages_state.viewport_bounds();
        Some((f32::from(b.top()), f32::from(b.bottom())))
    }

    /// The mouse moved over a message's bubble (`Manager::updateButton`).
    pub(super) fn corner_moved(&mut self, peer: PeerKey, msg_id: i32) {
        if self.text_ui.menu.is_some() {
            return;
        }
        let visible = self.corner_visible();
        let ui = &mut self.corner_ui;
        if let Some(corner) = &mut ui.current {
            if corner.peer == peer && corner.msg_id == msg_id {
                if !corner.over_bubble {
                    corner.over_bubble = true;
                    corner.apply_pointer(visible);
                }
                return;
            }
            // Another message: this one goes.
            let mut old = ui.current.take().unwrap();
            old.apply_state(State::Hidden);
            ui.hiding.push(old);
        }
        // Shown once the mouse rests.
        let now = Instant::now();
        ui.pending = Some((peer, msg_id, now));
        crate::repaint::at(now + SHOW_DELAY);
    }

    /// The mouse left a message's bubble.
    pub(super) fn corner_left(&mut self, peer: PeerKey, msg_id: i32) {
        let visible = self.corner_visible();
        let ui = &mut self.corner_ui;
        if ui
            .pending
            .is_some_and(|(p, id, _)| p == peer && id == msg_id)
        {
            ui.pending = None;
        }
        if let Some(corner) = &mut ui.current
            && corner.peer == peer
            && corner.msg_id == msg_id
        {
            corner.over_bubble = false;
            corner.apply_pointer(visible);
        }
    }

    /// The mouse entered or left the button (`inner`) or the area around it.
    fn corner_hovered(&mut self, peer: PeerKey, msg_id: i32, inner: bool, over: bool) {
        let visible = self.corner_visible();
        if let Some(corner) = &mut self.corner_ui.current
            && corner.peer == peer
            && corner.msg_id == msg_id
        {
            if inner {
                corner.over_inner = over;
            } else {
                corner.over_active = over;
            }
            corner.apply_pointer(visible);
        }
    }

    /// Hides the button at once (a menu opened, a reaction was chosen).
    pub(super) fn corner_hide(&mut self) {
        let ui = &mut self.corner_ui;
        ui.pending = None;
        if let Some(mut corner) = ui.current.take() {
            corner.apply_state(State::Hidden);
            ui.hiding.push(corner);
        }
    }

    /// Timers and animations of the buttons; true while something moves.
    pub(super) fn tick_corner(&mut self) -> bool {
        let now = Instant::now();
        if let Some((peer, msg_id, at)) = self.corner_ui.pending {
            if now >= at + SHOW_DELAY {
                self.corner_ui.pending = None;
                let keys = self.strip_reactions();
                if !keys.is_empty() && self.text_ui.menu.is_none() {
                    let mut strip = StripState::column(peer, msg_id, keys);
                    self.prepare_strip_cells(&mut strip, IMAGE, true);
                    let mut corner = Corner::new(strip);
                    corner.apply_state(State::Shown);
                    self.corner_ui.current = Some(corner);
                }
            } else {
                crate::repaint::at(at + SHOW_DELAY);
            }
        }
        if self.text_ui.menu.is_some() && self.corner_ui.current.is_some() {
            self.corner_hide();
        }
        let ui = &mut self.corner_ui;
        if let Some(corner) = &mut ui.current {
            if corner.expand_at.is_some_and(|at| now >= at) {
                corner.expand_at = None;
                if corner.over_inner {
                    corner.apply_state(State::Inside);
                }
            }
            if corner.hide_at.is_some_and(|at| now >= at) {
                let mut old = ui.current.take().unwrap();
                old.apply_state(State::Hidden);
                ui.hiding.push(old);
            } else {
                // Collapsed back: the column starts from its first again.
                if corner.added() == 0. && corner.state != State::Inside {
                    corner.scroll = 0.;
                }
                corner.strip.rearm();
            }
        }
        ui.hiding.retain(|c| !c.gone());
        ui.current.as_ref().is_some_and(Corner::animating) || !ui.hiding.is_empty()
    }

    /// The wheel over the unrolled column scrolls it (`consumeWheelEvent`).
    fn corner_wheel(&mut self, dy: f32) -> bool {
        let Some(corner) = &mut self.corner_ui.current else {
            return false;
        };
        let max = corner.scroll_max();
        if corner.state != State::Inside || max <= 0. {
            return false;
        }
        let one = lg(SIZE.1) + lg(SKIP);
        let limit = one * MAX_SCROLL_AT_ONCE;
        let shift = (dy * if corner.up { 1. } else { -1. }).clamp(-limit, limit);
        corner.scroll = (corner.scroll + shift).clamp(0., max);
        true
    }

    /// A reaction of the button was chosen (`Manager::lookupChosen`).
    fn corner_chosen(&mut self, k: usize) {
        let Some(corner) = &self.corner_ui.current else {
            return;
        };
        let Some(key) = corner.strip.keys.get(k).cloned() else {
            return;
        };
        let (peer, msg_id) = (corner.peer, corner.msg_id);
        let from = corner.strip.image_bounds(k);
        self.corner_hide();
        let chosen = self
            .messages
            .iter()
            .find(|m| m.id == msg_id)
            .and_then(|m| m.reactions.as_ref())
            .is_some_and(|r| r.buttons.iter().any(|b| b.key == key && b.chosen));
        if !chosen {
            self.start_fly(peer, msg_id, key.clone(), from);
        }
        let _ = self
            .commands
            .send(Command::ToggleReaction { peer, msg_id, key });
    }

    /// The buttons of message `msg_id` beside its bubble (`bubble` wide and
    /// high; `at_left` for bubbles on the right, `near` when a round button
    /// already stands at the right): drawn over the messages around it.
    pub(super) fn render_corner(
        &self,
        peer: Option<PeerKey>,
        msg_id: i32,
        bubble: (f32, f32),
        at_left: bool,
        near: bool,
        view: &WeakEntity<Self>,
    ) -> Vec<AnyElement> {
        let Some(peer) = peer else {
            return Vec::new();
        };
        let ui = &self.corner_ui;
        ui.hiding
            .iter()
            .map(|c| (c, false))
            .chain(ui.current.as_ref().map(|c| (c, true)))
            .filter(|(c, _)| c.peer == peer && c.msg_id == msg_id)
            .map(|(c, current)| self.render_one_corner(c, current, bubble, at_left, near, view))
            .collect()
    }

    /// `Manager::paintButton`.
    fn render_one_corner(
        &self,
        corner: &Corner,
        current: bool,
        (bubble_w, bubble_h): (f32, f32),
        at_left: bool,
        near: bool,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let (peer, msg_id) = (corner.peer, corner.msg_id);
        let (w, h) = (lg(SIZE.0), lg(SIZE.1));
        let cx = if at_left {
            -lg(CENTER.0)
        } else {
            bubble_w + if near { 0. } else { lg(CENTER.0) }
        };
        let cy = bubble_h + lg(CENTER.1);
        let added = corner.added();
        let ratio = if corner.expanded_added > 0. {
            (added / corner.expanded_added).clamp(0., 1.)
        } else {
            0.
        };
        let left = cx - w / 2.;
        let top = cy - h / 2. - if corner.up { added } else { 0. };
        let inner_h = h + added;
        let scale = corner.current_scale();
        let opacity = corner.current_opacity();
        let bg = c(p.window_bg);
        let shadow = gpui::BoxShadow {
            color: c(p.shadow_fg).into(),
            offset: point(gpui::px(0.), gpui::px(lg(1.))),
            blur_radius: gpui::px(lg(SHADOW)),
            spread_radius: gpui::px(0.),
        };
        // The round: scaled while collapsed, its corners rounder as it unrolls.
        let round = if added > 0. {
            let radius = h / 2. + (w / 2. - h / 2.) * ratio;
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(gpui::px(w))
                .h(gpui::px(inner_h))
                .rounded(gpui::px(radius))
                .bg(bg)
                .shadow(vec![shadow])
        } else {
            let (sw, sh) = (w * scale, h * scale);
            div()
                .absolute()
                .left(gpui::px((w - sw) / 2.))
                .top(gpui::px((h - sh) / 2.))
                .w(gpui::px(sw))
                .h(gpui::px(sh))
                .rounded(gpui::px(sh / 2.))
                .bg(bg)
                .shadow(vec![shadow])
        };
        // The column (`Strip::paint`), from the first at the bottom when it
        // goes up.
        let one = h + lg(SKIP);
        let expanded_skip = (ratio * lg(EXPANDED_SKIP)).round();
        let scroll = if !corner.collapse_fade && ratio < 1. {
            (ratio * corner.scroll).round().clamp(0., corner.scroll)
        } else {
            corner.scroll
        };
        let column_opacity = if corner.collapse_fade { ratio } else { 1. };
        let cell_top = |k: usize| {
            if corner.up {
                added - expanded_skip - k as f32 * one + scroll
            } else {
                expanded_skip + k as f32 * one - scroll
            }
        };
        let n = if added > 0. {
            corner.strip.keys.len()
        } else {
            1
        };
        let mut column = div()
            .absolute()
            .left_0()
            .top_0()
            .w(gpui::px(w))
            .h(gpui::px(inner_h))
            .overflow_hidden()
            .opacity(if added > 0. { column_opacity } else { 1. });
        for k in 0..n {
            let t = cell_top(k);
            if t + h < 0. || t > inner_h {
                continue;
            }
            let key = corner.strip.keys[k].clone();
            let cell = corner.strip.cells.get(k).cloned();
            let image = if added > 0. {
                lg(IMAGE) * corner.strip.scale(k)
            } else {
                lg(IMAGE) * scale * corner.strip.scale(k)
            };
            let bounds_out = corner.strip.cell_bounds.clone();
            let (hover, click) = (view.clone(), view.clone());
            column = column.child(
                div()
                    .id(("corner-cell", k))
                    .absolute()
                    .left_0()
                    .top(gpui::px(t))
                    .w(gpui::px(w))
                    .h(gpui::px(h))
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                let mut all = bounds_out.borrow_mut();
                                if all.len() <= k {
                                    all.resize(k + 1, Bounds::default());
                                }
                                all[k] = bounds;
                            },
                            move |bounds, _, window, _| {
                                paint_reaction_icon(&key, cell.as_ref(), bounds, image, window);
                            },
                        )
                        .size_full(),
                    )
                    .when(current, |d| {
                        d.on_hover(move |over, _, cx| {
                            let over = *over;
                            let _ = hover.update(cx, |this, cx| {
                                if let Some(corner) = &mut this.corner_ui.current {
                                    corner.strip.set_hovered(k, over);
                                }
                                cx.notify();
                            });
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            let _ = click.update(cx, |this, cx| {
                                this.corner_chosen(k);
                                cx.notify();
                            });
                        })
                    }),
            );
        }
        // Scrolled away reactions fade under gradients (`paintInnerGradients`).
        if added > 0. && corner.scroll_max() > 0. {
            let end = corner.scroll_max() - scroll;
            let (top_hidden, bottom_hidden) = if corner.up {
                (end, scroll)
            } else {
                (scroll, end)
            };
            let mut transparent = bg;
            transparent.a = 0.;
            let gradient = |hidden: f32, at_top: bool| {
                (hidden > 0.).then(|| {
                    let opacity = (ratio * hidden / lg(GRADIENT_FADE)).min(1.);
                    let (from, to) = if at_top {
                        (bg, transparent)
                    } else {
                        (transparent, bg)
                    };
                    div()
                        .absolute()
                        .left_0()
                        .top(gpui::px(if at_top {
                            0.
                        } else {
                            inner_h - lg(GRADIENT_SIZE)
                        }))
                        .w(gpui::px(w))
                        .h(gpui::px(lg(GRADIENT_SIZE)))
                        .opacity(opacity)
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(from, 0.),
                            linear_color_stop(to, 1.),
                        ))
                })
            };
            column = column
                .children(gradient(top_hidden, true))
                .children(gradient(bottom_hidden, false));
        }
        // Fading the column in: the first reaction moves from where it was.
        let main = (added > 0. && column_opacity < 1.).then(|| {
            let key = corner.strip.keys[0].clone();
            let cell = corner.strip.cells.first().cloned();
            let shift = lg(MAIN_APPEAR_SHIFT) * column_opacity;
            let t = if corner.up { added - shift } else { shift };
            div()
                .absolute()
                .left_0()
                .top(gpui::px(t))
                .w(gpui::px(w))
                .h(gpui::px(h))
                .opacity(1. - column_opacity)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            paint_reaction_icon(&key, cell.as_ref(), bounds, lg(IMAGE), window);
                        },
                    )
                    .size_full(),
                )
        });
        let bounds_out = corner.bounds.clone();
        let pad = lg(ACTIVE_PADDING);
        let (active_view, inner_view, wheel_view) = (view.clone(), view.clone(), view.clone());
        let inner = div()
            .id("corner-inner")
            .absolute()
            .left(gpui::px(pad))
            .top(gpui::px(pad))
            .w(gpui::px(w))
            .h(gpui::px(inner_h))
            .child(round)
            .child(column)
            .children(main)
            .child(
                canvas(
                    move |bounds, _, _| bounds_out.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when(current, |d| {
                d.occlude()
                    .on_hover(move |over, _, cx| {
                        let over = *over;
                        let _ = inner_view.update(cx, |this, cx| {
                            this.corner_hovered(peer, msg_id, true, over);
                            cx.notify();
                        });
                    })
                    .on_scroll_wheel(move |e, window, cx| {
                        let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                        let _ = wheel_view.update(cx, |this, cx| {
                            if this.corner_wheel(dy) {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        });
                    })
            });
        deferred(
            div()
                .id(("corner-active", msg_id as u64))
                .absolute()
                .left(gpui::px(left - pad))
                .top(gpui::px(top - pad))
                .w(gpui::px(w + 2. * pad))
                .h(gpui::px(inner_h + 2. * pad))
                .opacity(opacity)
                .when(current, |d| {
                    d.on_hover(move |over, _, cx| {
                        let over = *over;
                        let _ = active_view.update(cx, |this, cx| {
                            this.corner_hovered(peer, msg_id, false, over);
                            cx.notify();
                        });
                    })
                })
                .child(inner),
        )
        .with_priority(1)
        .into_any_element()
    }
}
