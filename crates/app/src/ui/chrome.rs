//! The window's controls around the chat list and the chat, as tdesktop has them:
//! the folder column (`Window::FiltersMenu` of `Ui::SideBarButton`s), the chat search
//! field (`dialogsFilter`), the "Connecting..." box (`Window::ConnectionState`), the
//! chat header's buttons (`TopBarWidget`) and the message field's buttons
//! (`HistoryWidget`: attach, emoji, send / record, AI, expand).
//!
//! Buttons whose function is not there yet do nothing (listed in ARCHITECTURE).

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    PathBuilder, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    point, prelude::FluentBuilder,
};
use tdr_theme::Color;

use super::{MainView, c};
use crate::bridge::{Command, DialogItem};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, px};

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
const TOP_BAR_BUTTON: f32 = 40.;
const TOP_BAR_MENU: f32 = 44.;
const TOP_BAR_ICON: (f32, f32) = (4., 11.);
const TOP_BAR_MENU_ICON: (f32, f32) = (16., 17.);
const TOP_BAR_RIPPLE_TOP: f32 = 7.;
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
    use gpui::{PathStyle, StrokeOptions};
    use lyon::tessellation::LineCap;
    let r = size / 2.;
    let (cx, cy) = (f32::from(bounds.left()) + r, f32::from(bounds.top()) + r);
    let steps = ((length.abs() / 5760.) * 64.).ceil().max(2.) as usize;
    let mut path = PathBuilder::stroke(gpui::px(thickness)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(thickness)
            .with_line_cap(LineCap::Round),
    ));
    for i in 0..=steps {
        let angle = (from + length * i as f32 / steps as f32) / 16. * std::f32::consts::PI / 180.;
        let at = point(
            gpui::px(cx + r * angle.cos()),
            gpui::px(cy - r * angle.sin()),
        );
        if i == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, c(color));
    }
}

impl MainView {
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
    fn icon_button(
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
        if self.folders.is_empty() {
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
                this.main_menu_open = !this.main_menu_open;
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

    fn select_folder(&mut self, id: i32, cx: &mut Context<Self>) {
        if self.active_folder != id {
            self.active_folder = id;
            let _ = self.commands.send(Command::SelectFolder(id));
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
        let height = if title.is_some() {
            FILTER_MIN_HEIGHT
        } else {
            FILTER_MENU_HEIGHT
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
        let icon_box = if title.is_some() {
            div()
                .absolute()
                .top(px(FILTER_ICON_TOP))
                .w_full()
                .flex()
                .justify_center()
                .child(glyph_el)
        } else {
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(glyph_el)
        };
        let text = title.map(|t| {
            div()
                .absolute()
                .top(px(FILTER_TEXT_TOP))
                .left(px(FILTER_TEXT_SKIP))
                .right(px(FILTER_TEXT_SKIP))
                .flex()
                .justify_center()
                .text_size(scale::font(FILTER_FONT).size)
                .line_height(scale::font(FILTER_FONT).line_height())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c(if active {
                    p.side_bar_text_fg_active
                } else {
                    p.side_bar_text_fg
                }))
                .child(one_line(t).centered())
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
        let ripple = self.ripple_layer(&id, FILTERS_WIDTH, height, p.side_bar_bg_ripple, false);
        div()
            .id(id)
            .relative()
            .flex_none()
            .w(px(FILTERS_WIDTH))
            .h(px(height))
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
    fn side_button_top(&self, id: &str, _window: &Window) -> f32 {
        let title = crate::scale::lg(super::TITLE_HEIGHT);
        if id == "folders-menu" {
            return title;
        }
        let mut top = title + FILTER_MENU_HEIGHT;
        for f in &self.folders {
            if format!("folder-{}", f.id) == id {
                return top;
            }
            top += FILTER_MIN_HEIGHT;
        }
        top
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
        let filters = !self.folders.is_empty();
        let header = div()
            .relative()
            .h(px(super::TOP_BAR_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(c(p.shadow_fg));
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
                this.main_menu_open = !this.main_menu_open;
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
        let slide = (total_h - CONNECTING_MARGIN) * (1. - shown);
        Some(
            div()
                .id("connecting")
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
                .child(spinner)
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
        row.child(self.icon_button(
            "top-menu",
            Icon::MenuDots,
            (TOP_BAR_MENU, super::TOP_BAR_HEIGHT),
            Some(TOP_BAR_MENU_ICON),
            ((0., TOP_BAR_RIPPLE_TOP), TOP_BAR_BUTTON),
            colors,
            window,
            cx,
            |_, _, _| {},
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
        let line = if over {
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
            (p.history_compose_icon_fg, p.history_compose_icon_fg_over),
            window,
            cx,
            |_, _, _| {},
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
fn paint_cross(bounds: gpui::Bounds<Pixels>, color: Color, window: &mut Window) {
    use gpui::{PathStyle, StrokeOptions};
    let size = f32::from(bounds.size.width);
    let skip = scale::lg(CANCEL_SKIP) * size / scale::lg(CANCEL_SIZE);
    let (l, t) = (f32::from(bounds.left()), f32::from(bounds.top()));
    let width = scale::lg(1.5);
    for (a, b) in [
        ((skip, skip), (size - skip, size - skip)),
        ((size - skip, skip), (skip, size - skip)),
    ] {
        let mut path = PathBuilder::stroke(gpui::px(width)).with_style(PathStyle::Stroke(
            StrokeOptions::default().with_line_width(width),
        ));
        path.move_to(point(gpui::px(l + a.0), gpui::px(t + a.1)));
        path.line_to(point(gpui::px(l + b.0), gpui::px(t + b.1)));
        if let Ok(path) = path.build() {
            window.paint_path(path, c(color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
