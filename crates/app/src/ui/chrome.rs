//! The window's controls around the chat list and the chat, as tdesktop has them:
//! the folder column (`Window::FiltersMenu` of `Ui::SideBarButton`s), the chat search
//! field (`dialogsFilter`), the "Connecting..." box (`Window::ConnectionState`), the
//! chat header's buttons (`TopBarWidget`) and the message field's buttons
//! (`HistoryWidget`: attach, emoji, send / record, AI, expand).
//!
//! Buttons whose function is not there yet do nothing (listed in ARCHITECTURE).

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, point,
    prelude::FluentBuilder,
};
use tdr_theme::Color;

use super::{MainView, c};
use crate::bridge::{Command, DialogItem};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, lg, px};

// window.style: windowFiltersWidth, windowFiltersButton, windowFiltersMainMenu.
pub(super) const FILTERS_WIDTH: f32 = 72.;
const FILTER_MIN_HEIGHT: f32 = 62.;
const FILTER_MENU_HEIGHT: f32 = 54.;
const FILTER_TEXT_TOP: f32 = 40.;
const FILTER_TEXT_SKIP: f32 = 6.;
const FILTER_ICON_TOP: f32 = 6.;
const FILTER_FONT: f32 = 11.;
const FILTER_BADGE_FONT: f32 = 12.;
const FILTER_BADGE_HEIGHT: f32 = 17.;
const FILTER_BADGE_SKIP: f32 = 4.;
const FILTER_BADGE_STROKE: f32 = 2.;
const FILTER_BADGE_X: f32 = 3.;
const FILTER_BADGE_Y: f32 = 7.;
/// `chatsFiltersTabs` (`dialogsSearchTabs`): height, padding, `strictSkip`, the
/// label's top and font, the bar, the unread count's skip (`lineWidth * 5`) and
/// the selection's duration (`defaultSettingsSlider.duration`).
const TABS_HEIGHT: f32 = 33.;
const TABS_PADDING: f32 = 9.;
const TABS_STRICT_SKIP: f32 = 18.;
const TABS_LABEL_TOP: f32 = 7.;
const TABS_FONT: f32 = 13.;
const TABS_BAR_TOP: f32 = 30.;
const TABS_BAR_STROKE: f32 = 6.;
const TABS_BAR_RADIUS: f32 = 2.;
const TABS_UNREAD_SKIP: f32 = 5.;
const TABS_DURATION: f32 = 150.;

/// `historyComposeButton`: the label's top (its height is `COMPOSE_BUTTON_HEIGHT`).
const COMPOSE_BUTTON_TEXT_TOP: f32 = 14.;

/// `SideBarButton::kMaxLabelLines`: longer titles wrap, the button grows.
const FILTER_MAX_LINES: usize = 3;

// dialogs.style: dialogsFilterPadding, dialogsFilterSkip, dialogsFilter, dialogsMenuToggle.
const FILTER_PADDING: f32 = 7.;
const FILTER_SKIP: f32 = 4.;
const SEARCH_HEIGHT: f32 = 35.;
const SEARCH_MARGIN_LEFT: f32 = 12.;
const SEARCH_MARGIN_TOP: f32 = 8.;
const SEARCH_MARGIN_RIGHT: f32 = 30.;
const MENU_TOGGLE: f32 = 40.;
/// dialogsCancelSearch: a cross in a 35 px square, its arms 12 px from the sides.
const CANCEL_SIZE: f32 = 35.;
const CANCEL_SKIP: f32 = 12.;

// window.style: connecting*.
const CONNECTING_MARGIN: f32 = 2.;
const CONNECTING_CAP: f32 = 20.;
const CONNECTING_HEIGHT: f32 = 40.;
const CONNECTING_TEXT_LEFT: f32 = 18.;
const CONNECTING_TEXT_TOP: f32 = 11.;
const CONNECTING_TEXT_RIGHT: f32 = 18.;
const CONNECTING_RADIAL: f32 = 20.;
const CONNECTING_DURATION: Duration = Duration::from_millis(150);
/// `kIgnoreStartConnectingFor`, `kConnectingStateDelay`.
const IGNORE_START_CONNECTING: Duration = Duration::from_millis(3000);
const CONNECTING_DELAY: Duration = Duration::from_millis(1000);

// info.style: topBarSearch, topBarMenuToggle, topBarSkip, topBarCallSkip.
pub(super) const TOP_BAR_BUTTON: f32 = 40.;
pub(super) const TOP_BAR_MENU: f32 = 44.;
const TOP_BAR_ICON: (f32, f32) = (4., 11.);
pub(super) const TOP_BAR_MENU_ICON: (f32, f32) = (16., 17.);
pub(super) const TOP_BAR_RIPPLE_TOP: f32 = 7.;
const TOP_BAR_SKIP: f32 = -5.;
const TOP_BAR_CALL_SKIP: f32 = -1.;
/// window.style: columnMinimalWidthLeft + columnMinimalWidthMain: narrower is one
/// column, where the info panel button is hidden.
const ONE_COLUMN_BELOW: f32 = 260. + 380.;

// chat_helpers.style: historyAttach, historySendSize, historyAiComposeButton(Position).
pub(super) const COMPOSE_BUTTON_WIDTH: f32 = 44.;
pub(super) const COMPOSE_BUTTON_HEIGHT: f32 = 46.;
const COMPOSE_RIPPLE: (f32, f32) = (2., 3.);
const AI_BUTTON: f32 = 28.;
const AI_POSITION: (f32, f32) = (-8., -4.);
/// The field's own top inside the message area (`historySendPadding`).
const FIELD_TOP: f32 = 9.;
/// st::universalDuration: the send button changing what it shows.
const SEND_SWITCH: Duration = Duration::from_millis(120);

// The ripple of icon buttons (`defaultRippleAnimation`).
const RIPPLE_SHOW: f32 = 650.;
const RIPPLE_HIDE: f32 = 200.;

/// A press ripple on a button: it grows from the press point inside the button's
/// ripple area and fades out after the release.
pub(super) struct ButtonRipple {
    id: SharedString,
    /// The press point in the ripple area.
    center: (f32, f32),
    start: Instant,
    released: Option<Instant>,
}

impl ButtonRipple {
    fn done(&self) -> bool {
        self.released
            .is_some_and(|r| r.elapsed().as_secs_f32() * 1000. >= RIPPLE_HIDE)
    }
}

/// The "Connecting..." box state.
pub(super) struct Connection {
    pub connected: bool,
    /// When the connection was lost (the box shows a second later).
    pub lost_at: Option<Instant>,
    /// When the radial started turning.
    pub radial_start: Instant,
    pub shown: super::Fade,
    /// The mouse is over it: the text slides out.
    pub text: super::Fade,
}

impl Connection {
    pub fn new() -> Self {
        Self {
            connected: true,
            lost_at: None,
            radial_start: Instant::now(),
            shown: super::Fade::new(0., CONNECTING_DURATION),
            text: super::Fade::new(0., CONNECTING_DURATION),
        }
    }
}

fn ease_out_quint(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(5)
}

fn sine_in_out(t: f32) -> f32 {
    -((std::f32::consts::PI * t.clamp(0., 1.)).cos() - 1.) / 2.
}

/// `InfiniteRadialAnimation::computeState` with `defaultInfiniteRadialAnimation`: the
/// arc's start and length in Qt's 1/16 degrees, `ms` after it started.
fn radial_arc(ms: f32) -> (f32, f32) {
    const FULL: f32 = 5760.;
    const LINEAR_PERIOD: f32 = 1000.;
    const SINE_PERIOD: f32 = 3000.;
    const SINE_DURATION: f32 = 1000.;
    const SINE_SHIFT: f32 = 1500.;
    let (min, max) = ((FULL * 0.0625).round(), (FULL * 0.75).round());
    let linear = FULL - (ms * FULL / LINEAR_PERIOD) % FULL;
    if ms <= SINE_DURATION {
        let length = FULL + (min - FULL) * sine_in_out(ms / SINE_DURATION);
        return (linear, length);
    }
    let since = ms - SINE_DURATION;
    let cycles = (since / SINE_PERIOD).floor();
    let relative = since % SINE_PERIOD;
    let small = SINE_SHIFT - SINE_DURATION;
    let basic = (linear + min + cycles * (FULL + min - max)) % FULL;
    if relative <= small {
        (basic - min, min)
    } else if relative <= small + SINE_DURATION {
        let length = min + (max - min) * sine_in_out((relative - small) / SINE_DURATION);
        (basic - length, length)
    } else if relative <= SINE_PERIOD - SINE_DURATION {
        (basic - max, max)
    } else {
        let t = (relative - (SINE_PERIOD - SINE_DURATION)) / SINE_DURATION;
        (basic - max, max - (max - min) * sine_in_out(t))
    }
}

/// `InfiniteRadialAnimation::draw` in a `size` square, `ms` after it started.
pub(crate) fn paint_radial(
    bounds: gpui::Bounds<Pixels>,
    size: f32,
    thickness: f32,
    ms: f32,
    color: Color,
    window: &mut Window,
) {
    let inset = gpui::Bounds::new(
        bounds.origin + point(gpui::px(thickness / 2.), gpui::px(thickness / 2.)),
        gpui::size(gpui::px(size - thickness), gpui::px(size - thickness)),
    );
    paint_arc(
        inset,
        size - thickness,
        thickness,
        radial_arc(ms),
        color,
        window,
    );
}

/// Paints an arc like `QPainter::drawArc` (angles in 1/16 degrees, counterclockwise
/// from 3 o'clock) in a `size` square at `bounds`' top left, with round caps.
fn paint_arc(
    bounds: gpui::Bounds<Pixels>,
    size: f32,
    thickness: f32,
    (from, length): (f32, f32),
    color: Color,
    window: &mut Window,
) {
    use crate::vector::{Cap, Join, Stroke, paint_strokes};
    let r = size / 2.;
    let (cx, cy) = (f32::from(bounds.left()) + r, f32::from(bounds.top()) + r);
    let steps = ((length.abs() / 5760.) * 64.).ceil().max(2.) as usize;
    let points: Vec<(f32, f32)> = (0..=steps)
        .map(|i| {
            let angle =
                (from + length * i as f32 / steps as f32) / 16. * std::f32::consts::PI / 180.;
            (cx + r * angle.cos(), cy - r * angle.sin())
        })
        .collect();
    let stroke = Stroke::new(thickness).cap(Cap::Round).join(Join::Round);
    paint_strokes(&[&points], stroke, c(color), window);
}

impl MainView {
    /// The bottom of a chat that can't be written to (`HistoryWidget`'s
    /// `_muteUnmute` / `_joinChannel`, `historyComposeButton`): one wide flat button
    /// in capitals with the "Direct Messages" button at its left and the gift button
    /// at its right (`historyDirectMessage`, `historyGiftToChannel`; both stubs).
    /// "Mute" mutes forever, "Unmute" unmutes; joining is a stub too.
    pub(super) fn render_compose_bar(
        &self,
        chat: &DialogItem,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::bridge::Compose;
        let (label, direct, gift) = match chat.compose {
            Compose::Field => return None,
            Compose::Join {
                channel,
                direct,
                gift,
                request,
            } => (
                // `refreshJoinChannelText`.
                if channel {
                    "JOIN CHANNEL"
                } else if request {
                    "APPLY TO JOIN GROUP"
                } else {
                    "JOIN GROUP"
                },
                direct,
                gift,
            ),
            Compose::MuteUnmute { direct, gift } => {
                (if chat.muted { "UNMUTE" } else { "MUTE" }, direct, gift)
            }
        };
        let p = self.palette();
        let height = lg(COMPOSE_BUTTON_HEIGHT);
        let font = scale::font(13.);
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == "compose-bar");
        let peer = chat.peer;
        let mute = !chat.muted;
        let join = matches!(chat.compose, Compose::Join { .. });
        let main = div()
            .id("compose-bar")
            .absolute()
            .left_0()
            .top_0()
            .w(gpui::px(width))
            .h(gpui::px(height))
            .overflow_hidden()
            .cursor_pointer()
            .bg(c(if over {
                p.history_compose_button_bg_over
            } else {
                p.history_compose_button_bg
            }))
            .children(self.ripple_layer(
                "compose-bar",
                width,
                height,
                p.history_compose_button_bg_ripple,
                false,
            ))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(COMPOSE_BUTTON_TEXT_TOP))
                    .flex()
                    .justify_center()
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c(p.window_active_text_fg))
                    .child(label),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.button_hovered = Some("compose-bar".into());
                } else if this
                    .button_hovered
                    .as_ref()
                    .is_some_and(|h| h.as_ref() == "compose-bar")
                {
                    this.button_hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                    // The bar spans the chat column at the window's bottom right.
                    let size = window.viewport_size();
                    let at = (
                        f32::from(e.position.x) - (f32::from(size.width) - width),
                        f32::from(e.position.y)
                            - (f32::from(size.height) - lg(COMPOSE_BUTTON_HEIGHT)),
                    );
                    this.press_button("compose-bar".into(), at);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.release_buttons();
                    cx.notify();
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.release_buttons();
                if join {
                    let _ = this.commands.send(Command::Join(peer));
                } else {
                    this.set_chat_muted(peer, mute);
                    let _ = this.commands.send(Command::SetMuted { peer, mute });
                }
                cx.notify();
            }));
        let colors = (p.window_active_text_fg, p.window_active_text_fg);
        let side = |id: &'static str, glyph: Icon, this: &Self, cx: &mut Context<Self>| {
            this.icon_button(
                id,
                glyph,
                (COMPOSE_BUTTON_HEIGHT, COMPOSE_BUTTON_HEIGHT),
                None,
                ((3., 3.), 40.),
                colors,
                window,
                cx,
                |_, _, _| {},
            )
            .absolute()
            .top_0()
        };
        Some(
            div()
                .relative()
                .flex_none()
                .w_full()
                .h(gpui::px(height))
                .child(main)
                // The line over the bottom, as over the message field.
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top_0()
                        .h(gpui::px(lg(1.)))
                        .bg(c(p.shadow_fg)),
                )
                .children(
                    direct
                        .then(|| side("compose-direct", Icon::MenuChatDiscuss, self, cx).left_0()),
                )
                .children(
                    gift.then(|| side("compose-gift", Icon::MenuGiftPremium, self, cx).right_0()),
                )
                .into_any_element(),
        )
    }

    /// The mute state shown at once, before the server's answer.
    pub(super) fn set_chat_muted(&mut self, peer: tdr_data::tdr_storage::PeerKey, muted: bool) {
        let until = muted.then_some(i32::MAX);
        let set = |d: &mut DialogItem| {
            d.muted = muted;
            d.mute_until = until;
        };
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.peer == peer) {
            set(d);
        }
        if let Some(d) = self.extra_chats.get_mut(&peer) {
            set(d);
        }
        for list in self.folder_lists.values_mut() {
            if let Some(d) = list.iter_mut().find(|d| d.peer == peer) {
                set(d);
            }
        }
    }

    /// Animations of the controls: ripples, the connecting box.
    pub(super) fn tick_chrome(&mut self, window: &mut Window) -> bool {
        self.button_ripples.retain(|r| !r.done());
        let connecting = self.connection_box_wanted();
        self.connection.shown.set(if connecting { 1. } else { 0. });
        let animating = !self.button_ripples.is_empty()
            || self.connection.shown.animating()
            || self.connection.text.animating()
            || self.connection.shown.value() > 0.
            || self.send_switch.1.elapsed() < SEND_SWITCH;
        if animating {
            window.request_animation_frame();
        }
        animating
    }

    /// `ConnectionState::refreshState`: a lost connection shows the box after a second,
    /// and not in the first three seconds after the start.
    fn connection_box_wanted(&self) -> bool {
        match self.connection.lost_at {
            Some(lost) if !self.connection.connected => {
                lost.elapsed() >= CONNECTING_DELAY
                    && crate::STARTED.elapsed() >= IGNORE_START_CONNECTING
            }
            _ => false,
        }
    }

    pub(super) fn set_connected(&mut self, connected: bool, cx: &mut Context<Self>) {
        if self.connection.connected == connected {
            return;
        }
        self.connection.connected = connected;
        if connected {
            self.connection.lost_at = None;
        } else {
            self.connection.lost_at = Some(Instant::now());
            self.connection.radial_start = Instant::now();
            Self::wake_after(CONNECTING_DELAY, cx);
            let left = IGNORE_START_CONNECTING.saturating_sub(crate::STARTED.elapsed());
            if !left.is_zero() {
                Self::wake_after(left, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn press_button(&mut self, id: SharedString, center: (f32, f32)) {
        self.button_ripples.retain(|r| r.id != id);
        self.button_ripples.push(ButtonRipple {
            id,
            center,
            start: Instant::now(),
            released: None,
        });
    }

    pub(super) fn release_buttons(&mut self) {
        for r in &mut self.button_ripples {
            r.released.get_or_insert_with(Instant::now);
        }
    }

    /// The ripple circle of button `id` inside a `w` x `h` area, if pressed.
    pub(super) fn ripple_layer(
        &self,
        id: &str,
        w: f32,
        h: f32,
        color: Color,
        round: bool,
    ) -> Option<Div> {
        let r = self.button_ripples.iter().find(|r| r.id.as_ref() == id)?;
        let shown = ease_out_quint(r.start.elapsed().as_secs_f32() * 1000. / RIPPLE_SHOW);
        let fade = r.released.map_or(1., |t| {
            1. - (t.elapsed().as_secs_f32() * 1000. / RIPPLE_HIDE).min(1.)
        });
        if fade <= 0. {
            return None;
        }
        let mut color = c(color);
        color.a *= fade;
        let layer = if round {
            // An icon button's round area: the ripple fills it from the middle.
            let radius = w / 2. * shown;
            div()
                .absolute()
                .left(px(w / 2. - radius))
                .top(px(h / 2. - radius))
                .size(px(2. * radius))
                .rounded_full()
                .bg(color)
        } else {
            let (cx, cy) = r.center;
            let reach = [(0., 0.), (w, 0.), (0., h), (w, h)]
                .iter()
                .map(|(x, y)| ((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt())
                .fold(0., f32::max);
            let radius = reach * shown;
            div()
                .absolute()
                .left(gpui::px(cx - radius))
                .top(gpui::px(cy - radius))
                .size(gpui::px(2. * radius))
                .rounded_full()
                .bg(color)
        };
        Some(layer)
    }

    /// A tdesktop `IconButton`: `w` x `h`, the icon at `icon_at` (centered when
    /// `None`), a round ripple of `ripple_size` at `ripple_at`, the icon's color
    /// changing under the mouse. `on_click` runs on release over it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn icon_button(
        &self,
        id: &'static str,
        glyph: Icon,
        (w, h): (f32, f32),
        icon_at: Option<(f32, f32)>,
        (ripple_at, ripple_size): ((f32, f32), f32),
        (fg, fg_over): (Color, Color),
        window: &Window,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == id);
        let glyph_el = icon(glyph, if over { fg_over } else { fg }, window);
        let placed = match icon_at {
            Some((x, y)) => div().absolute().left(px(x)).top(px(y)).child(glyph_el),
            None => div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(glyph_el),
        };
        let ripple = div()
            .absolute()
            .left(px(ripple_at.0))
            .top(px(ripple_at.1))
            .size(px(ripple_size))
            .children(self.ripple_layer(id, ripple_size, ripple_size, p.window_bg_over, true));
        div()
            .id(id)
            .relative()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .cursor_pointer()
            .child(ripple)
            .child(placed)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.button_hovered = Some(id.into());
                } else if this
                    .button_hovered
                    .as_ref()
                    .is_some_and(|h| h.as_ref() == id)
                {
                    this.button_hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.press_button(id.into(), (0., 0.));
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.release_buttons();
                    on_click(this, window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.release_buttons();
                    cx.notify();
                }),
            )
    }

    // ----- The folder column -----

    /// The column of folders on the left, when the account has folders.
    pub(super) fn render_folders(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.filters_vertical() {
            return None;
        }
        let p = self.palette();
        // `windowFiltersMainMenuUnread(Muted)`.
        let dot = self.menu_dot(cx);
        let menu = self.side_bar_button(
            "folders-menu".into(),
            None,
            if dot.is_some() {
                Icon::DialogsMenuUnread
            } else {
                Icon::DialogsMenu
            },
            false,
            None,
            window,
            cx,
            |this, _, cx| {
                this.toggle_main_menu();
                cx.notify();
            },
        );
        let menu = match dot {
            Some(muted) => menu.child(Self::dot_layer(
                if muted {
                    p.side_bar_badge_bg_muted
                } else {
                    p.side_bar_badge_bg
                },
                window,
            )),
            None => menu,
        };
        let mut list = div().flex().flex_col();
        for folder in &self.folders {
            let id = folder.id;
            list = list.child(self.side_bar_button(
                format!("folder-{id}").into(),
                Some(folder.title.clone()),
                Icon::Folder(folder.icon),
                self.active_folder == id,
                folder.badge,
                window,
                cx,
                move |this, _, cx| this.select_folder(id, cx),
            ));
        }
        // "Edit" (`lng_filters_setup`): the folder settings come later.
        list = list.child(self.side_bar_button(
            "folders-edit".into(),
            Some("Edit".into()),
            Icon::Folder(tdr_data::filters::FilterIcon::Edit),
            false,
            None,
            window,
            cx,
            |_, _, _| {},
        ));
        Some(
            div()
                .w(px(FILTERS_WIDTH))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .bg(c(p.side_bar_bg))
                .child(menu)
                .child(
                    div()
                        .id("folders-scroll")
                        .flex_1()
                        .min_h(gpui::px(0.))
                        .overflow_y_scroll()
                        .child(list),
                ),
        )
    }

    /// `Ui::UnreadBadge` with the default style (`dialogsUnread*`): a count in a pill.
    pub(super) fn count_badge(&self, text: &str, muted: bool, window: &Window) -> Div {
        let p = self.palette();
        let font = scale::font(super::UNREAD_FONT_SIZE);
        let w = (super::text_width(
            window,
            text,
            super::UNREAD_FONT_SIZE,
            gpui::FontWeight::BOLD,
        ) + 2. * lg(super::UNREAD_PADDING))
        .max(lg(super::UNREAD_HEIGHT));
        div()
            .flex_none()
            .w(gpui::px(w.round()))
            .h(px(super::UNREAD_HEIGHT))
            .rounded(px(super::UNREAD_HEIGHT / 2.))
            .bg(c(if muted {
                p.dialogs_unread_bg_muted
            } else {
                p.dialogs_unread_bg
            }))
            .text_color(c(p.dialogs_unread_fg))
            .text_size(font.size)
            .line_height(font.line_height())
            .font_weight(gpui::FontWeight::BOLD)
            .flex()
            .items_center()
            .justify_center()
            .child(text.to_string())
    }

    /// The folders as tabs over the chat list when the folder column does not fit
    /// (`ChatsFiltersTabs` with `chatsFiltersTabs`): labels with unread counts, the
    /// rounded bar under the chosen one moving to a new choice (150 ms), the folder
    /// switching once the bar is there.
    pub(super) fn render_folder_tabs(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.folders.is_empty() || self.filters_vertical() {
            return None;
        }
        let p = self.palette();
        let font = scale::font(TABS_FONT);
        let chosen = self.tabs_chosen.unwrap_or(self.active_folder);
        // `SettingsSlider::countSectionsWidths` with `strictSkip`: each tab its
        // content and 9 px on both sides, after the 9 px padding.
        struct Tab {
            id: i32,
            left: f32,
            width: f32,
            content: f32,
            label: String,
            badge: Option<(String, bool)>,
        }
        let mut tabs = Vec::new();
        let mut x = lg(TABS_PADDING);
        for folder in &self.folders {
            let label = if folder.id == 0 {
                "All".to_string()
            } else {
                folder.title.clone()
            };
            let badge = folder.badge.map(|(n, muted)| {
                let text = if n > 999 {
                    "99+".to_string()
                } else {
                    n.to_string()
                };
                (text, muted)
            });
            let badge_w = badge.as_ref().map_or(0., |(t, _)| {
                lg(TABS_UNREAD_SKIP)
                    + (super::text_width(
                        window,
                        t,
                        super::UNREAD_FONT_SIZE,
                        gpui::FontWeight::BOLD,
                    ) + 2. * lg(super::UNREAD_PADDING))
                    .max(lg(super::UNREAD_HEIGHT))
                    .round()
            });
            let content =
                super::text_width(window, &label, TABS_FONT, gpui::FontWeight::SEMIBOLD) + badge_w;
            let width = lg(TABS_STRICT_SKIP) + content;
            tabs.push(Tab {
                id: folder.id,
                left: x.floor(),
                width: width.round(),
                content,
                label,
                badge,
            });
            x += width;
        }
        let total = x + lg(TABS_PADDING);
        // `getFinalActiveRange` with `barSnapToLabel`, animated from where it was.
        let target = tabs.iter().find(|t| t.id == chosen).map_or((0., 0.), |t| {
            (t.left + (t.width - t.content) / 2., t.content)
        });
        let range = match self.tabs_anim {
            Some((start, from_left, from_width)) => {
                let t = (start.elapsed().as_secs_f32() * 1000. / TABS_DURATION).min(1.);
                if t >= 1. {
                    self.tabs_anim = None;
                } else {
                    window.request_animation_frame();
                }
                (
                    crate::anim::mix(from_left, target.0, t),
                    crate::anim::mix(from_width, target.1, t),
                )
            }
            None => target,
        };
        self.tabs_bar.set(range);
        let height = lg(TABS_HEIGHT);
        let mut strip = div()
            .relative()
            .w(gpui::px(total.max(1.)))
            .h(gpui::px(height));
        for (i, tab) in tabs.iter().enumerate() {
            let id = tab.id;
            let active = id == chosen;
            let ripple_id: SharedString = format!("folder-tab-{id}").into();
            let press_id = ripple_id.clone();
            let label_left = (tab.width - tab.content) / 2.;
            let mut content = div()
                .absolute()
                .left(gpui::px(label_left))
                .top(px(TABS_LABEL_TOP))
                .flex()
                .items_start()
                .child(
                    div()
                        .text_size(font.size)
                        .line_height(font.line_height())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c(if active {
                            p.light_button_fg
                        } else {
                            p.window_sub_text_fg
                        }))
                        .whitespace_nowrap()
                        .child(tab.label.clone()),
                );
            if let Some((text, muted)) = &tab.badge {
                content = content.child(
                    div()
                        .pl(px(TABS_UNREAD_SKIP))
                        .child(self.count_badge(text, *muted, window)),
                );
            }
            let width = tab.width;
            strip = strip.child(
                div()
                    .id(("folder-tab", i))
                    .absolute()
                    .left(gpui::px(tab.left))
                    .top_0()
                    .w(gpui::px(width))
                    .h(gpui::px(height))
                    .overflow_hidden()
                    .cursor_pointer()
                    .children(self.ripple_layer(
                        &ripple_id,
                        width,
                        height,
                        if active {
                            p.light_button_bg_over
                        } else {
                            p.window_bg_over
                        },
                        false,
                    ))
                    .child(content)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.press_button(press_id.clone(), (width / 2., height / 2.));
                            cx.notify();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.release_buttons();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.release_buttons();
                        this.folder_tab_clicked(id, cx);
                    })),
            );
        }
        // The bar: `barStroke` high from `barTop`, its lower half cut by the strip.
        let add = lg(TABS_BAR_STROKE) / 2.;
        strip = strip.child(
            div()
                .absolute()
                .left(gpui::px((range.0 - add).max(0.)))
                .top(px(TABS_BAR_TOP))
                .w(gpui::px(range.1 + 2. * add))
                .h(px(TABS_BAR_STROKE))
                .rounded(px(TABS_BAR_RADIUS))
                .bg(c(p.slider_bg_active)),
        );
        Some(
            div()
                .id("folder-tabs")
                .relative()
                .flex_none()
                .w_full()
                .h(gpui::px(height))
                .overflow_hidden()
                .bg(c(p.dialogs_bg))
                .child(
                    div()
                        .id("folder-tabs-scroll")
                        .size_full()
                        .overflow_x_scroll()
                        .child(strip),
                )
                // `PlainShadow` along the bottom.
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(gpui::px(lg(1.)))
                        .bg(c(p.shadow_fg)),
                )
                .into_any_element(),
        )
    }

    /// A tab clicked: the bar moves there and the folder opens after it
    /// (`DiscreteSlider::_callbackAfterMs`); the open one scrolls the list up.
    fn folder_tab_clicked(&mut self, id: i32, cx: &mut Context<Self>) {
        let chosen = self.tabs_chosen.unwrap_or(self.active_folder);
        if id == chosen {
            if self.tabs_chosen.is_none() {
                self.start_scroll_anim(super::Area::Dialogs, 0.);
            }
            cx.notify();
            return;
        }
        let (left, width) = self.tabs_bar.get();
        // `DiscreteSlider::setActiveSection`: the folder is activated at once
        // (`activateCallback`), unless the bar's previous move still runs; then the
        // bar starts moving.
        let wait = self
            .tabs_anim
            .map(|(start, ..)| TABS_DURATION - start.elapsed().as_secs_f32() * 1000.)
            .filter(|wait| *wait > 0.);
        self.tabs_anim = Some((Instant::now(), left, width));
        match wait {
            None => {
                self.tabs_chosen = None;
                self.switch_folder_sliding(id, cx);
            }
            Some(wait) => {
                self.tabs_chosen = Some(id);
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(wait as u64))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.tabs_chosen == Some(id) {
                            this.tabs_chosen = None;
                            this.switch_folder_sliding(id, cx);
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// `Widget::switchToChatsFilter` from the tabs: the old list slides out to one
    /// side and the new one in from the other (`startChatsFilterSlide`).
    fn switch_folder_sliding(&mut self, id: i32, cx: &mut Context<Self>) {
        let was = self.active_folder;
        let old = self.dialogs.clone();
        let old_skip = self.list_skip();
        let (_, _, old_offset) = self.scroll_metrics(super::Area::Dialogs);
        let archive = self.archive_open;
        self.select_folder(id, cx);
        if archive || self.active_folder == was {
            return;
        }
        let index = |f: i32| self.folders.iter().position(|x| x.id == f);
        self.folder_slide = Some(super::FolderSlide {
            start: Instant::now(),
            old,
            old_skip,
            old_offset,
            left: index(id) < index(was),
        });
    }

    fn select_folder(&mut self, id: i32, cx: &mut Context<Self>) {
        // A folder chosen in the open archive closes it (`setActiveChatsFilter`).
        if self.archive_open {
            self.archive_open = false;
            self.archive_slide = None;
            let _ = self.commands.send(Command::CloseArchive);
            if self.active_folder == id {
                self.dialogs = self.folder_lists.get(&id).cloned().unwrap_or_default();
                self.set_scroll(super::Area::Dialogs, 0.);
            }
        }
        if self.active_folder != id {
            self.active_folder = id;
            let _ = self.commands.send(Command::SelectFolder(id));
            // The list changes at once, as in tdesktop: the folder's last list, or
            // empty until its chats come. The open chat keeps its header data.
            if let Some(peer) = self.selected
                && let Some(item) = self.dialogs.iter().find(|d| d.peer == peer)
            {
                self.extra_chats.insert(peer, item.clone());
            }
            self.dialogs = self.folder_lists.get(&id).cloned().unwrap_or_default();
            self.set_scroll(super::Area::Dialogs, 0.);
        }
        cx.notify();
    }

    /// `Ui::SideBarButton`: a folder (icon, title, unread badge) or the main menu.
    #[allow(clippy::too_many_arguments)]
    fn side_bar_button(
        &self,
        id: SharedString,
        title: Option<String>,
        glyph: Icon,
        active: bool,
        badge: Option<(u32, bool)>,
        window: &Window,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        let lines = title.as_deref().map(|t| Self::folder_lines(t, window));
        // `SideBarButton::resizeGetHeight`: a line of text more, a line higher.
        let height = match &lines {
            Some(lines) => Self::folder_height(lines.len()),
            None => lg(FILTER_MENU_HEIGHT),
        };
        let bg = if active {
            p.side_bar_bg_active
        } else {
            p.side_bar_bg
        };
        let fg = if active {
            p.side_bar_icon_fg_active
        } else {
            p.side_bar_icon_fg
        };
        let glyph_el = icon(glyph, fg, window);
        // `SideBarButton::iconLeft/iconTop`: centered in whole device pixels.
        let scale_f = scale::get();
        let width_dev = scale::device(FILTERS_WIDTH);
        let icon_w_dev = (crate::icons::icon_width(glyph, window) * scale_f).round();
        let icon_h_dev = (crate::icons::icon_height(glyph, window) * scale_f).round();
        let icon_left = ((width_dev - icon_w_dev) / 2.).floor() / scale_f;
        let icon_top = if lines.is_some() {
            lg(FILTER_ICON_TOP)
        } else {
            (((height * scale_f).round() - icon_h_dev) / 2.).floor() / scale_f
        };
        let icon_box = div()
            .absolute()
            .left(gpui::px(icon_left))
            .top(gpui::px(icon_top))
            .child(glyph_el);
        let text = lines.map(|lines| {
            let font = scale::font(FILTER_FONT);
            let count = lines.len();
            let mut column = div()
                .absolute()
                .top(px(FILTER_TEXT_TOP))
                .left(px(FILTER_TEXT_SKIP))
                .right(px(FILTER_TEXT_SKIP))
                .flex()
                .flex_col()
                .items_center()
                .text_size(font.size)
                .line_height(font.line_height())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c(if active {
                    p.side_bar_text_fg_active
                } else {
                    p.side_bar_text_fg
                }));
            for (i, line) in lines.into_iter().enumerate() {
                // The last line keeps the rest of a longer title, cut with "…".
                column = column.child(if i + 1 == count {
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(one_line(line).centered())
                } else {
                    div().whitespace_nowrap().child(line)
                });
            }
            column
        });
        // The badge, cut out of the icon by a ring of the background.
        let badge_el = badge.map(|(count, muted)| {
            let label = if count > 999 {
                "99+".to_string()
            } else {
                count.to_string()
            };
            let font = scale::font(FILTER_BADGE_FONT);
            let text_w = super::text_width(
                window,
                &label,
                FILTER_BADGE_FONT,
                gpui::FontWeight::SEMIBOLD,
            );
            let w = FILTER_BADGE_HEIGHT.max(text_w + 2. * FILTER_BADGE_SKIP);
            let fill = match (active, muted) {
                (false, false) => p.side_bar_badge_bg,
                (true, false) => p.side_bar_badge_bg_active,
                (false, true) => p.side_bar_badge_bg_muted,
                (true, true) => p.side_bar_badge_bg_muted_active,
            };
            let x = (FILTERS_WIDTH / 2. + FILTER_BADGE_X).min(FILTERS_WIDTH - w - 6.);
            div()
                .absolute()
                .left(px(x - FILTER_BADGE_STROKE))
                .top(px(FILTER_BADGE_Y - FILTER_BADGE_STROKE))
                .w(px(w + 2. * FILTER_BADGE_STROKE))
                .h(px(FILTER_BADGE_HEIGHT + 2. * FILTER_BADGE_STROKE))
                .rounded_full()
                .bg(c(bg))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(w))
                        .h(px(FILTER_BADGE_HEIGHT))
                        .rounded_full()
                        .bg(c(fill))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(font.size)
                        .line_height(font.line_height())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(c(p.side_bar_badge_fg))
                        .child(label),
                )
        });
        let ripple_id = id.clone();
        let ripple = self.ripple_layer(&id, lg(FILTERS_WIDTH), height, p.side_bar_bg_ripple, false);
        div()
            .id(id)
            .relative()
            .flex_none()
            .w(px(FILTERS_WIDTH))
            .h(gpui::px(height))
            .overflow_hidden()
            .bg(c(bg))
            .cursor_pointer()
            .children(ripple)
            .child(icon_box)
            .children(text)
            .children(badge_el)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    // The press point inside the button: the ripple grows from it.
                    let top = this.side_button_top(&ripple_id, window);
                    let center = (f32::from(e.position.x), f32::from(e.position.y) - top);
                    this.press_button(ripple_id.clone(), center);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.release_buttons();
                    on_click(this, window, cx);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.release_buttons();
                    cx.notify();
                }),
            )
    }

    /// Where a folder button starts, in window coordinates (for its ripple).
    fn side_button_top(&self, id: &str, window: &Window) -> f32 {
        let title = crate::scale::lg(super::TITLE_HEIGHT);
        if id == "folders-menu" {
            return title;
        }
        let mut top = title + lg(FILTER_MENU_HEIGHT);
        for f in &self.folders {
            if format!("folder-{}", f.id) == id {
                return top;
            }
            top += Self::folder_height(Self::folder_lines(&f.title, window).len());
        }
        top
    }

    /// A folder button with `lines` lines of title (`resizeGetHeight`).
    fn folder_height(lines: usize) -> f32 {
        lg(FILTER_MIN_HEIGHT) + (lines.max(1) - 1) as f32 * scale::font(FILTER_FONT).height
    }

    /// A folder's title in the lines it takes (at most three).
    fn folder_lines(title: &str, window: &Window) -> Vec<String> {
        let width = lg(FILTERS_WIDTH) - 2. * lg(FILTER_TEXT_SKIP);
        wrap_title(title, width, |t| {
            super::text_width(window, t, FILTER_FONT, gpui::FontWeight::SEMIBOLD)
        })
    }

    // ----- The chat list's header -----

    /// The header over the chat list: the main menu button (when there is no folder
    /// column) and the search field; narrow: one button in the middle.
    pub(super) fn render_dialogs_header(
        &self,
        dialogs_width: f32,
        narrow: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette();
        // The folder column holds the menu button; as tabs, the header keeps it.
        let filters = self.filters_vertical();
        // No line under it: the list starts right below (`Dialogs::Widget`).
        let header = div().relative().h(px(super::TOP_BAR_HEIGHT)).flex_none();
        if narrow {
            // `_searchForNarrowLayout` with folders, else the main menu button.
            let button = if filters {
                self.icon_button(
                    "search-narrow",
                    Icon::TopBarSearch,
                    (MENU_TOGGLE, MENU_TOGGLE),
                    Some((4., 4.)),
                    ((0., 0.), MENU_TOGGLE),
                    (p.menu_icon_fg, p.menu_icon_fg_over),
                    window,
                    cx,
                    |_, _, _| {},
                )
            } else {
                self.menu_toggle(window, cx)
            };
            return header.child(
                div()
                    .absolute()
                    .left(px((dialogs_width - MENU_TOGGLE) / 2.))
                    .top(px(FILTER_PADDING))
                    .child(button),
            );
        }
        let left = if filters {
            FILTER_SKIP + FILTER_PADDING
        } else {
            FILTER_PADDING + MENU_TOGGLE + FILTER_PADDING
        };
        let right = FILTER_SKIP + FILTER_PADDING;
        let header = header.when(!filters, |h| {
            h.child(
                div()
                    .absolute()
                    .left(px(FILTER_PADDING))
                    .top(px(FILTER_PADDING))
                    .child(self.menu_toggle(window, cx)),
            )
        });
        header.child(
            div()
                .absolute()
                .left(px(left))
                .right(px(right))
                .top(px((super::TOP_BAR_HEIGHT - SEARCH_HEIGHT) / 2.))
                .child(self.render_search(window, cx)),
        )
    }

    /// The other accounts' unread messages put a dot on the menu button
    /// (`OtherAccountsUnreadState`): `Some(muted)` when there are any.
    fn menu_dot(&self, cx: &Context<Self>) -> Option<bool> {
        let (count, muted) = self.accounts.read(cx).others_unread();
        (count > 0).then_some(muted)
    }

    /// The dot over a centered menu icon (`dialogs_menu_unread_dot`).
    fn dot_layer(color: Color, window: &Window) -> Div {
        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(Icon::DialogsMenuUnreadDot, color, window))
    }

    /// `dialogsMenuToggle`: the main menu; with unread messages in other accounts
    /// `dialogsMenuToggleUnread` (the dot in `dialogsUnreadBg`, or the icon's color
    /// when they are all muted).
    fn menu_toggle(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let p = self.palette();
        let dot = self.menu_dot(cx);
        let button = self.icon_button(
            "main-menu-toggle",
            if dot.is_some() {
                Icon::DialogsMenuUnread
            } else {
                Icon::DialogsMenu
            },
            (MENU_TOGGLE, MENU_TOGGLE),
            None,
            ((0., 0.), MENU_TOGGLE),
            (p.dialogs_menu_icon_fg, p.dialogs_menu_icon_fg_over),
            window,
            cx,
            |this, _, cx| {
                this.toggle_main_menu();
                cx.notify();
            },
        );
        match dot {
            Some(muted) => button.child(Self::dot_layer(
                if muted {
                    p.dialogs_menu_icon_fg
                } else {
                    p.dialogs_unread_bg
                },
                window,
            )),
            None => button,
        }
    }

    /// `dialogsFilter`: a rounded field with "Search"; searching comes later.
    fn render_search(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        use gpui::Focusable;
        let p = self.palette();
        let focused = self.search.focus_handle(cx).is_focused(window);
        let has_text = !self.search.read(cx).is_empty();
        let cancel = has_text.then(|| {
            let over = self
                .button_hovered
                .as_ref()
                .is_some_and(|h| h.as_ref() == "search-cancel");
            let color = if over {
                p.dialogs_menu_icon_fg_over
            } else {
                p.dialogs_menu_icon_fg
            };
            div()
                .id("search-cancel")
                .absolute()
                .right_0()
                .top_0()
                .size(px(CANCEL_SIZE))
                .cursor_pointer()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| paint_cross(bounds, color, window),
                    )
                    .size_full(),
                )
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.button_hovered = hovered.then(|| "search-cancel".into());
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.search
                            .update(cx, |s, cx| s.set_text(String::new(), cx));
                        window.focus(&this.search.focus_handle(cx));
                    }),
                )
        });
        div()
            .id("search-field")
            .relative()
            .h(px(SEARCH_HEIGHT))
            .rounded(px(SEARCH_HEIGHT / 2.))
            .bg(c(if focused {
                p.filter_input_active_bg
            } else {
                p.filter_input_inactive_bg
            }))
            .when(focused, |d| {
                d.border_1().border_color(c(p.window_bg_ripple))
            })
            .pl(px(SEARCH_MARGIN_LEFT))
            .pr(px(SEARCH_MARGIN_RIGHT))
            .pt(px(SEARCH_MARGIN_TOP))
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    window.focus(&this.search.focus_handle(cx));
                    let (position, shift, count) = (e.position, e.modifiers.shift, e.click_count);
                    this.search
                        .update(cx, |i, cx| i.press_at(position, shift, count, cx));
                }),
            )
            .child(self.search.clone())
            .children(cancel)
    }

    // ----- "Connecting..." -----

    /// `Window::ConnectionState`: a round box at the bottom left of the chat list with
    /// a spinner; under the mouse it widens with "Connecting...".
    pub(super) fn render_connecting(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let shown = self.connection.shown.value();
        if shown <= 0. {
            return None;
        }
        let p = self.palette();
        let text = "Connecting...";
        let text_w = super::text_width(window, text, super::FONT_SIZE, gpui::FontWeight::NORMAL);
        let content =
            (CONNECTING_TEXT_LEFT + text_w + CONNECTING_TEXT_RIGHT) * self.connection.text.value();
        let inner_w = 2. * CONNECTING_CAP + content;
        let total_h = CONNECTING_HEIGHT + 2. * CONNECTING_MARGIN;
        let radial_start = self.connection.radial_start;
        let spinner_color = p.menu_icon_fg;
        let shadow = p.window_shadow_fg;
        let body = p.window_bg;
        // The body between the caps: `connecting_body` is solid from 5 to 35 px, its
        // shadow from 4 to 38 px (fading below).
        let body_rows: [(f32, f32, Color, f32); 5] = [
            (4., 1., shadow, 21. / 255.),
            (35., 1., shadow, 61. / 255.),
            (36., 1., shadow, 26. / 255.),
            (37., 1., shadow, 8. / 255.),
            (5., 30., shadow, 77. / 255.),
        ];
        let mut middle = div()
            .absolute()
            .left(px(CONNECTING_MARGIN + CONNECTING_CAP))
            .top(px(CONNECTING_MARGIN))
            .w(px(content))
            .h(px(CONNECTING_HEIGHT));
        for (top, h, color, alpha) in body_rows {
            let mut fill = c(color);
            fill.a *= alpha;
            middle = middle.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(h))
                    .bg(fill),
            );
        }
        middle = middle.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(5.))
                .h(px(30.))
                .bg(c(body)),
        );
        let label = (content > CONNECTING_TEXT_LEFT).then(|| {
            div()
                .absolute()
                .left(px(CONNECTING_TEXT_LEFT))
                .top(px(CONNECTING_TEXT_TOP))
                .w(px((content - CONNECTING_TEXT_LEFT).max(0.)))
                .overflow_hidden()
                .text_color(c(p.window_sub_text_fg))
                .child(one_line(text.to_string()))
        });
        let middle = middle.children(label);
        let cap = |left: f32, glyph: Icon, glyph_shadow: Icon| {
            div()
                .absolute()
                .left(px(left))
                .top(px(CONNECTING_MARGIN))
                .child(div().absolute().child(icon(glyph_shadow, shadow, window)))
                .child(div().absolute().child(icon(glyph, body, window)))
        };
        let spinner = div()
            .absolute()
            .left(px((total_h - CONNECTING_RADIAL) / 2.))
            .top(px((total_h - CONNECTING_RADIAL) / 2.))
            .size(px(CONNECTING_RADIAL))
            .bg(c(p.window_bg))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let ms = radial_start.elapsed().as_secs_f32() * 1000.;
                        let size = f32::from(bounds.size.width);
                        let thickness = scale::lg(2.);
                        let inset = gpui::Bounds::new(
                            bounds.origin
                                + point(gpui::px(thickness / 2.), gpui::px(thickness / 2.)),
                            gpui::size(gpui::px(size - thickness), gpui::px(size - thickness)),
                        );
                        paint_arc(
                            inset,
                            size - thickness,
                            thickness,
                            radial_arc(ms),
                            spinner_color,
                            window,
                        );
                        window.request_animation_frame();
                    },
                )
                .size_full(),
            );
        // The proxy state at the right (`ProxyIcon`): no proxy, the grey shield.
        let shield = div()
            .absolute()
            .right(px((total_h - CONNECTING_RADIAL) / 2.))
            .top_0()
            .h(px(total_h))
            .w(px(CONNECTING_RADIAL))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(Icon::ProxyOff, p.menu_icon_fg, window));
        let slide = (total_h - CONNECTING_MARGIN) * (1. - shown);
        Some(
            div()
                .id("connecting")
                // A button (`ConnectionState::Widget`, it opens the proxy settings,
                // not done yet): presses never reach the list under it.
                .occlude()
                .cursor_pointer()
                .absolute()
                .left_0()
                .bottom(px(-slide))
                .w(px(inner_w + 2. * CONNECTING_MARGIN))
                .h(px(total_h))
                .child(cap(
                    CONNECTING_MARGIN,
                    Icon::ConnectingLeft,
                    Icon::ConnectingLeftShadow,
                ))
                .child(middle)
                .child(cap(
                    CONNECTING_MARGIN + CONNECTING_CAP + content,
                    Icon::ConnectingRight,
                    Icon::ConnectingRightShadow,
                ))
                // The spinner over the shield (`Progress` is made after `ProxyIcon`,
                // both on the window's color): folded, only the spinner shows.
                .child(shield)
                .child(spinner)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.connection.text.set(if *hovered { 1. } else { 0. });
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    // ----- The chat header -----

    /// `TopBarWidget`'s buttons on the right: search, call (users), the info panel
    /// toggle (not in one column), the menu. None of them works yet.
    pub(super) fn render_top_bar_buttons(
        &self,
        chat: &DialogItem,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        use tdr_data::describe::PeerKind as K;
        let p = self.palette();
        let colors = (p.menu_icon_fg, p.menu_icon_fg_over);
        let size = (TOP_BAR_BUTTON, super::TOP_BAR_HEIGHT);
        let ripple = ((0., TOP_BAR_RIPPLE_TOP), TOP_BAR_BUTTON);
        let calls = chat.kind == K::User
            && !chat.is_saved
            && chat.peer
                != tdr_data::tdr_storage::PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID);
        let one_column = f32::from(window.viewport_size().width) < ONE_COLUMN_BELOW;
        let mut row = div().flex_none().h_full().flex();
        row = row.child(self.icon_button(
            "top-search",
            Icon::TopBarSearch,
            size,
            Some(TOP_BAR_ICON),
            ripple,
            colors,
            window,
            cx,
            |_, _, _| {},
        ));
        row = row.child(div().w(px(TOP_BAR_CALL_SKIP)));
        if calls {
            row = row.child(self.icon_button(
                "top-call",
                Icon::TopBarCall,
                size,
                Some(TOP_BAR_ICON),
                ripple,
                colors,
                window,
                cx,
                |_, _, _| {},
            ));
        }
        if !one_column {
            row = row.child(self.icon_button(
                "top-info",
                Icon::TopBarProfile,
                size,
                Some(TOP_BAR_ICON),
                ripple,
                colors,
                window,
                cx,
                |_, _, _| {},
            ));
        }
        row = row.child(div().w(px(TOP_BAR_SKIP)));
        let peer = chat.peer;
        row.child(self.icon_button(
            "top-menu",
            Icon::MenuDots,
            (TOP_BAR_MENU, super::TOP_BAR_HEIGHT),
            Some(TOP_BAR_MENU_ICON),
            ((0., TOP_BAR_RIPPLE_TOP), TOP_BAR_BUTTON),
            colors,
            window,
            cx,
            move |this, window, _| {
                // Its top right under the button (`TopBarWidget::showPeerMenu`).
                let at = gpui::point(
                    window.viewport_size().width,
                    gpui::px(lg(super::TITLE_HEIGHT) + lg(super::TOP_BAR_HEIGHT) - lg(8.)),
                );
                this.open_chat_menu(at, peer);
            },
        ))
    }

    // ----- The message field -----

    /// `historyAttach`: on the left of the field.
    pub(super) fn render_attach(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        self.icon_button(
            "compose-attach",
            Icon::InputAttach,
            (COMPOSE_BUTTON_WIDTH, COMPOSE_BUTTON_HEIGHT),
            None,
            (COMPOSE_RIPPLE, 40.),
            (p.history_compose_icon_fg, p.history_compose_icon_fg_over),
            window,
            cx,
            |_, _, _| {},
        )
    }

    /// `historyAttachEmoji`: right of the field. `EmojiButton` draws the face's circle
    /// itself, inside the icon's middle half.
    pub(super) fn render_emoji(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == "compose-emoji");
        // Docked as the third column: the button is active
        // (`historyAttachEmojiActive`, `historyRecordVoiceFgActive`).
        let docked = self.emoji_section_width(window) > 0.;
        let line = if docked {
            p.window_bg_active
        } else if over {
            p.history_compose_icon_fg_over
        } else {
            p.history_compose_icon_fg
        };
        let (icon_w, icon_h) = (
            crate::icons::icon_width(Icon::InputSmile, window),
            crate::icons::icon_height(Icon::InputSmile, window),
        );
        let circle = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let left = f32::from(bounds.left()) + ((w - icon_w) / 2.).floor() + icon_w / 4.;
                let top = f32::from(bounds.top()) + ((h - icon_h) / 2.).floor() + icon_h / 4.;
                let size = icon_w / 2.;
                let at = gpui::Bounds::new(
                    point(gpui::px(left), gpui::px(top)),
                    gpui::size(gpui::px(size), gpui::px(size)),
                );
                paint_arc(at, size, 1.5, (0., 5760.), line, window);
            },
        )
        .absolute()
        .size_full();
        self.icon_button(
            "compose-emoji",
            Icon::InputSmile,
            (COMPOSE_BUTTON_WIDTH, COMPOSE_BUTTON_HEIGHT),
            None,
            (COMPOSE_RIPPLE, 40.),
            if docked {
                (p.window_bg_active, p.window_bg_active)
            } else {
                (p.history_compose_icon_fg, p.history_compose_icon_fg_over)
            },
            window,
            cx,
            |this, window, cx| this.emoji_button_clicked(window, cx),
        )
        .child(circle)
    }

    /// `Ui::SendButton`: "send" with text, else "record a voice message"; the two
    /// cross-fade (`_stateChangeAnimation`).
    pub(super) fn render_send(
        &self,
        has_text: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        let t = (self.send_switch.1.elapsed().as_secs_f32() / SEND_SWITCH.as_secs_f32()).min(1.);
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == "compose-send");
        let layer = |glyph: Icon, color: Color, opacity: f32, scale_by: f32| {
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .opacity(opacity)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(40. * scale_by))
                        .child(icon(glyph, color, window)),
                )
        };
        let send_color = if over {
            p.history_send_icon_fg_over
        } else {
            p.history_send_icon_fg
        };
        let record_color = if over {
            p.history_compose_icon_fg_over
        } else {
            p.history_compose_icon_fg
        };
        let (now_icon, now_color, was_icon, was_color) = if has_text {
            (Icon::Send, send_color, Icon::InputRecord, record_color)
        } else {
            (Icon::InputRecord, record_color, Icon::Send, send_color)
        };
        let ripple = div()
            .absolute()
            .left(px(COMPOSE_RIPPLE.0))
            .top(px(COMPOSE_RIPPLE.1))
            .size(px(40.))
            .children(self.ripple_layer("compose-send", 40., 40., p.window_bg_over, true));
        div()
            .id("compose-send")
            .relative()
            .flex_none()
            .w(px(COMPOSE_BUTTON_WIDTH))
            .h(px(COMPOSE_BUTTON_HEIGHT))
            .cursor_pointer()
            .child(ripple)
            .when(t < 1., |d| d.child(layer(was_icon, was_color, 1. - t, 1.)))
            .child(layer(now_icon, now_color, t, 1.))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.button_hovered = Some("compose-send".into());
                } else if this
                    .button_hovered
                    .as_ref()
                    .is_some_and(|h| h.as_ref() == "compose-send")
                {
                    this.button_hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.press_button("compose-send".into(), (0., 0.));
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.release_buttons();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    use gpui::Focusable;
                    this.release_buttons();
                    // Recording voice messages comes later.
                    if this.input.read(cx).has_text() {
                        let focus = this.input.focus_handle(cx);
                        window.focus(&focus);
                        window.dispatch_action(Box::new(crate::text_input::Enter), cx);
                    }
                    cx.notify();
                }),
            )
    }

    /// The send button changes what it shows when the field gets or loses its text.
    pub(super) fn update_send_state(&mut self, has_text: bool) {
        if self.send_switch.0 != has_text {
            self.send_switch = (has_text, Instant::now());
        }
    }

    /// The AI tools button (top left) and the expand button (top right) over a field
    /// of three lines or more (`HasEnoughLinesForAi`, `HasEnoughLinesForExpand`).
    /// Positions relative to the message area's top left.
    pub(super) fn render_field_extras(
        &self,
        area_width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let input = self.input.read(cx);
        if input.shown_lines() < 3 || !input.has_text() {
            return Vec::new();
        }
        let p = self.palette();
        let colors = (p.history_compose_icon_fg, p.history_compose_icon_fg_over);
        let ai_over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == "compose-ai");
        let ai_color = if ai_over { colors.1 } else { colors.0 };
        let ai = self
            .icon_button(
                "compose-ai",
                Icon::AiLetters,
                (AI_BUTTON, AI_BUTTON),
                None,
                ((0., 0.), AI_BUTTON),
                colors,
                window,
                cx,
                |_, _, _| {},
            )
            // The two stars over the letters (`ComposeAiButton::paintIcons`).
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(Icon::AiStar1, ai_color, window)),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(Icon::AiStar2, ai_color, window)),
            );
        let expand = self.icon_button(
            "compose-expand",
            Icon::Expand,
            (AI_BUTTON, AI_BUTTON),
            None,
            ((0., 0.), AI_BUTTON),
            colors,
            window,
            cx,
            |_, _, _| {},
        );
        let top = FIELD_TOP + AI_POSITION.1;
        let expand_left = area_width - AI_BUTTON + AI_POSITION.0;
        vec![
            div()
                .absolute()
                .left(px(-AI_POSITION.0))
                .top(px(top))
                .child(ai)
                .into_any_element(),
            div()
                .absolute()
                .left(px(expand_left))
                .top(px(top))
                .child(expand)
                .into_any_element(),
        ]
    }
}

/// `CrossAnimation` at rest: an X in a `CANCEL_SIZE` square, 1.5 px strokes.
pub(super) fn paint_cross(bounds: gpui::Bounds<Pixels>, color: Color, window: &mut Window) {
    let size = f32::from(bounds.size.width);
    let skip = scale::lg(CANCEL_SKIP) * size / scale::lg(CANCEL_SIZE);
    let (l, t) = (f32::from(bounds.left()), f32::from(bounds.top()));
    let width = scale::lg(1.5);
    let first = [(l + skip, t + skip), (l + size - skip, t + size - skip)];
    let second = [(l + size - skip, t + skip), (l + skip, t + size - skip)];
    crate::vector::paint_strokes(
        &[&first, &second],
        crate::vector::Stroke::new(width),
        c(color),
        window,
    );
}

/// A folder title wrapped like `Ui::Text::String` into lines of `width`: words go
/// to the next line whole, a word longer than a line breaks anywhere. The last
/// allowed line keeps all the rest (it is cut with "…" when drawn).
fn wrap_title(title: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = title.trim();
    while !rest.is_empty() {
        if lines.len() + 1 == FILTER_MAX_LINES || measure(rest) <= width {
            lines.push(rest.to_string());
            break;
        }
        let mut cut = None;
        for (i, _) in rest.match_indices(' ') {
            if measure(&rest[..i]) <= width {
                cut = Some(i);
            } else {
                break;
            }
        }
        let end = cut.unwrap_or_else(|| {
            // The first word alone is too long: as many characters as fit.
            let mut end = 0;
            for (i, ch) in rest.char_indices() {
                let next = i + ch.len_utf8();
                if end > 0 && measure(&rest[..next]) > width {
                    break;
                }
                end = next;
            }
            end
        });
        lines.push(rest[..end].trim_end().to_string());
        rest = rest[end..].trim_start();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_titles_wrap_like_tdesktop() {
        let measure = |t: &str| t.chars().count() as f32;
        assert_eq!(wrap_title("Work", 10., measure), ["Work"]);
        assert_eq!(
            wrap_title("Personal chats", 10., measure),
            ["Personal", "chats"]
        );
        assert_eq!(
            wrap_title("Supercalifragilistic", 10., measure),
            ["Supercalif", "ragilistic"]
        );
        // The third line keeps the rest.
        assert_eq!(
            wrap_title("one two three four five six", 5., measure),
            ["one", "two", "three four five six"]
        );
        assert_eq!(wrap_title("", 5., measure), [""]);
    }

    #[test]
    fn radial_arc_like_lib_ui() {
        // Starting: the full circle shrinks to the minimal arc in a second.
        let (_, length) = radial_arc(0.);
        assert!((length - 5760.).abs() < 1.);
        let (_, length) = radial_arc(1000.);
        assert!((length - 360.).abs() < 1.);
        // Then the arc grows to three quarters and shrinks back every 3 s.
        let (_, length) = radial_arc(1000. + 1500.);
        assert!((length - 4320.).abs() < 1.);
        let (_, length) = radial_arc(1000. + 3000.);
        assert!((length - 360.).abs() < 1.);
    }
}
