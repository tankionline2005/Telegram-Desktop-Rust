//! Popup menus (`Ui::PopupMenu` with `Ui::Menu::Menu`, lib_ui): a rounded panel
//! with a shadow that unrolls from the corner at the cursor (`PanelAnimation`: the
//! width from half, the height from a third, the items revealed top down under a
//! soft edge, 200 ms), items with a hover background and a ripple where they were
//! pressed, separators, and fades out (150 ms) when an item is chosen or the user
//! clicks elsewhere.
//!
//! Sizes: `menuWithIcons` / `popupMenuWithIcons` for menus with icons,
//! `defaultMenu` / `defaultPopupMenu` without (the tray menu).

use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement, Styled, TextRun,
    Window, div, font, linear_color_stop, linear_gradient, point, px as gpx,
};
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};

use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, lg, px};

const FONT_SIZE: f32 = 13.;
/// `whenReadStyle`: font(12px).
const WHEN_FONT_SIZE: f32 = 12.;
// widgets.style: defaultPopupMenu / popupMenuWithIcons.
const RADIUS: f32 = 8.;
const SHOW_MS: f32 = 200.;
const HIDE_MS: f32 = 150.;
const WIDTH_MIN: f32 = 156.;
const WIDTH_MAX: f32 = 300.;
// defaultPanelAnimation.
const START_WIDTH: f32 = 0.5;
const WIDTH_DURATION: f32 = 0.6;
const START_HEIGHT: f32 = 0.3;
const HEIGHT_DURATION: f32 = 0.9;
const OPACITY_DURATION: f32 = 0.3;
const FADE_HEIGHT: f32 = 0.2;
// defaultRippleAnimation.
const RIPPLE_SHOW_MS: f32 = 650.;
const RIPPLE_HIDE_MS: f32 = 200.;
// chat_helpers.style: whenReadPadding, whenReadIconPosition.
const WHEN_PADDING: [f32; 4] = [34., 3., 17., 4.];
const WHEN_ICON: (f32, f32) = (8., 0.);
// calls.style: groupCallJoinAsPadding, groupCallJoinAsPhotoSize, *TextTop, *NameTop.
const JOIN_AS_PADDING: [f32; 4] = [12., 8., 12., 7.];
pub const JOIN_AS_PHOTO: f32 = 30.;
const JOIN_AS_TEXT_TOP: f32 = 4.;
const JOIN_AS_NAME_TOP: f32 = 23.;
/// chat.style: historyBankCardMenuMultilinePosition.
const TITLE_POSITION: (f32, f32) = (18., 4.);

struct Style {
    /// left, top, right, bottom
    item_padding: [f32; 4],
    icon_position: (f32, f32),
    /// `scrollPadding` top and bottom.
    scroll_padding: f32,
}

const WITH_ICONS: Style = Style {
    item_padding: [54., 8., 17., 8.],
    icon_position: (15., 5.),
    scroll_padding: 5.,
};
const PLAIN: Style = Style {
    item_padding: [17., 8., 17., 7.],
    icon_position: (0., 0.),
    scroll_padding: 8.,
};

/// What an item does. Items of features that do not exist yet do nothing (they are
/// listed in docs/ARCHITECTURE.md).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Reply,
    QuoteReply,
    Edit,
    Pin,
    Forward,
    Delete,
    Select,
    AddContact,
    AddToCalendar,
    SetReminder,
    ViewProfile(PeerKey),
    ToggleNotifications,
    /// The message's whole text.
    CopyText(PeerKey, i32),
    CopySelected,
    /// Copy this, then show this toast (if any).
    Copy(String, Option<&'static str>),
    OpenUrl(String),
    /// The tray menu's first item: show or hide the window.
    TrayShow,
    TrayHide,
    TrayQuit,
}

/// The phone number's owner in a phone menu (`ResolvePhoneAction`).
#[derive(Clone, Debug, PartialEq)]
pub enum PhoneRow {
    Loading,
    Missing,
    Found { peer: PeerKey, name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Action {
        label: SharedString,
        icon: Option<Icon>,
        action: MenuAction,
    },
    /// `expandedMenuSeparator`: a wide band.
    WideSeparator,
    /// "today at 20:29" with the forwarded / edited icon (`WhenAction`).
    When {
        icon: Icon,
        text: SharedString,
    },
    Phone(PhoneRow),
    /// A grey line in the middle ("Loading...", "Failed").
    Status(SharedString),
    /// A bank's name: copies the card number.
    Title {
        text: SharedString,
        action: MenuAction,
    },
}

impl Entry {
    pub fn action(label: &str, icon: Icon, action: MenuAction) -> Self {
        Entry::Action {
            label: label.to_string().into(),
            icon: Some(icon),
            action,
        }
    }

    fn clickable(&self) -> bool {
        matches!(
            self,
            Entry::Action { .. } | Entry::Title { .. } | Entry::Phone(PhoneRow::Found { .. })
        )
    }
}

struct Ripple {
    row: usize,
    center: (f32, f32),
    start: Instant,
    released: Option<Instant>,
}

/// An open menu.
pub struct PopupMenu {
    pub entries: Vec<Entry>,
    icons: bool,
    /// Where it was asked for (the cursor), in the coordinates it is drawn in.
    anchor: Point<Pixels>,
    shown: Instant,
    hiding: Option<Instant>,
    hovered: Option<usize>,
    pressed: Option<usize>,
    ripple: Option<Ripple>,
}

/// Mouse input over a menu.
pub enum MenuEvent {
    /// The mouse entered or left a row. Rows report on their own, in any order,
    /// so leaving one never clears another that was just entered.
    Hover {
        row: usize,
        over: bool,
    },
    Press {
        row: usize,
        at: Point<Pixels>,
    },
    Release {
        row: usize,
    },
    /// A click outside: the menu goes.
    Dismiss,
}

pub struct Geometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    rows: Vec<(f32, f32)>,
    from_right: bool,
    from_bottom: bool,
}

fn ease_out_circ(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    (1. - (t - 1.) * (t - 1.)).max(0.).sqrt()
}

fn ease_out_quint(t: f32) -> f32 {
    let t = t.clamp(0., 1.) - 1.;
    t * t * t * t * t + 1.
}

fn hsla(c: Color) -> Hsla {
    gpui::rgba(c.rgba()).into()
}

fn text_width(window: &Window, text: &str, size: f32) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let run = TextRun {
        len: text.len(),
        font: font("Open Sans"),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .layout_line(text, scale::font(size).size, &[run], None)
            .width,
    )
    .ceil()
}

impl PopupMenu {
    pub fn new(entries: Vec<Entry>, icons: bool, anchor: Point<Pixels>) -> Self {
        Self {
            entries,
            icons,
            anchor,
            shown: Instant::now(),
            hiding: None,
            hovered: None,
            pressed: None,
            ripple: None,
        }
    }

    fn style(&self) -> &'static Style {
        if self.icons { &WITH_ICONS } else { &PLAIN }
    }

    pub fn hide(&mut self) {
        if self.hiding.is_none() {
            self.hiding = Some(Instant::now());
        }
    }

    pub fn hiding(&self) -> bool {
        self.hiding.is_some()
    }

    /// Faded out: drop it.
    pub fn gone(&self) -> bool {
        self.hiding
            .is_some_and(|h| h.elapsed().as_secs_f32() * 1000. >= HIDE_MS)
    }

    pub fn animating(&self) -> bool {
        self.shown.elapsed().as_secs_f32() * 1000. < SHOW_MS
            || self.hiding.is_some()
            || self.ripple.as_ref().is_some_and(|r| {
                r.released
                    .is_none_or(|t| t.elapsed().as_secs_f32() * 1000. < RIPPLE_HIDE_MS)
            })
    }

    /// Applies an event; returns the chosen action (the menu then fades out).
    pub fn handle(&mut self, event: MenuEvent, geometry: &Geometry) -> Option<MenuAction> {
        if self.hiding.is_some() {
            return None;
        }
        match event {
            MenuEvent::Hover { row, over } => {
                if over {
                    if self.entries[row].clickable() {
                        self.hovered = Some(row);
                    }
                } else if self.hovered == Some(row) {
                    self.hovered = None;
                }
                None
            }
            MenuEvent::Press { row, at } => {
                if !self.entries[row].clickable() {
                    return None;
                }
                self.pressed = Some(row);
                let (top, _) = geometry.rows[row];
                self.ripple = Some(Ripple {
                    row,
                    center: (
                        f32::from(at.x) - geometry.x,
                        f32::from(at.y) - geometry.y - top,
                    ),
                    start: Instant::now(),
                    released: None,
                });
                None
            }
            MenuEvent::Release { row } => {
                if let Some(r) = &mut self.ripple {
                    r.released.get_or_insert_with(Instant::now);
                }
                if self.pressed.take() != Some(row) {
                    return None;
                }
                let action = match &self.entries[row] {
                    Entry::Action { action, .. } | Entry::Title { action, .. } => action.clone(),
                    Entry::Phone(PhoneRow::Found { peer, .. }) => MenuAction::ViewProfile(*peer),
                    _ => return None,
                };
                self.hide();
                Some(action)
            }
            MenuEvent::Dismiss => {
                self.hide();
                None
            }
        }
    }

    /// Sizes and the place inside `area`: below and right of the cursor, flipped to
    /// the other side where it does not fit (`prepareGeometryFor`).
    pub fn geometry(&self, area: Bounds<Pixels>, window: &Window) -> Geometry {
        let st = self.style();
        let [pl, pt, pr, pb] = st.item_padding.map(lg);
        let font_h = scale::font(FONT_SIZE).height;
        let when_h = lg(WHEN_PADDING[1]) + scale::font(WHEN_FONT_SIZE).height + lg(WHEN_PADDING[3]);
        let join_h = lg(JOIN_AS_PADDING[1]) + lg(JOIN_AS_PHOTO) + lg(JOIN_AS_PADDING[3]);
        let mut width: f32 = 0.;
        let mut rows = Vec::with_capacity(self.entries.len());
        let mut y = lg(st.scroll_padding);
        for e in &self.entries {
            let (w, h) = match e {
                Entry::Action { label, .. } => (
                    pl + text_width(window, label, FONT_SIZE) + pr,
                    pt + font_h + pb,
                ),
                Entry::WideSeparator => (0., lg(4.) + lg(6.) + lg(4.)),
                Entry::When { text, .. } => (
                    lg(WHEN_PADDING[0])
                        + text_width(window, text, WHEN_FONT_SIZE)
                        + lg(WHEN_PADDING[2]),
                    when_h,
                ),
                Entry::Phone(row) => {
                    let text_left = 2. * lg(JOIN_AS_PADDING[0]) + lg(JOIN_AS_PHOTO);
                    let w = match row {
                        PhoneRow::Found { name, .. } => text_width(window, name, FONT_SIZE)
                            .max(text_width(window, "View profile", FONT_SIZE)),
                        _ => text_width(window, "View profile", FONT_SIZE),
                    };
                    (text_left + w + lg(JOIN_AS_PADDING[2]), join_h)
                }
                Entry::Status(_) => (0., lg(JOIN_AS_PHOTO)),
                Entry::Title { text, .. } => (
                    lg(TITLE_POSITION.0) + text_width(window, text, FONT_SIZE) + pr,
                    2. * lg(TITLE_POSITION.1) + font_h,
                ),
            };
            width = width.max(w.clamp(lg(WIDTH_MIN), lg(WIDTH_MAX)));
            rows.push((y, h));
            y += h;
        }
        let w = width.max(lg(WIDTH_MIN));
        let h = y + lg(st.scroll_padding);
        let (ax, ay) = (f32::from(self.anchor.x), f32::from(self.anchor.y));
        let (left, top) = (f32::from(area.origin.x), f32::from(area.origin.y));
        let (right, bottom) = (
            left + f32::from(area.size.width),
            top + f32::from(area.size.height),
        );
        let from_right = ax + w > right;
        let from_bottom = ay + h > bottom;
        let x = if from_right { ax - w } else { ax }.clamp(left, (right - w).max(left));
        let y = if from_bottom { ay - h } else { ay }.clamp(top, (bottom - h).max(top));
        Geometry {
            x: x.round(),
            y: y.round(),
            w: w.ceil(),
            h,
            rows,
            from_right,
            from_bottom,
        }
    }
}

/// Colors of a menu.
pub struct MenuColors {
    pub bg: Color,
    pub bg_over: Color,
    pub ripple: Color,
    pub fg: Color,
    pub fg_over: Color,
    /// `itemFgShortcut`: grey text.
    pub sub: Color,
    pub icon: Color,
    pub separator: Color,
}

impl MenuColors {
    pub fn from(p: &Palette) -> Self {
        // `defaultMenu`: itemBg, itemBgOver and the ripple are the window colors.
        Self {
            bg: p.window_bg,
            bg_over: p.window_bg_over,
            ripple: p.window_bg_ripple,
            fg: p.window_fg,
            fg_over: p.window_fg_over,
            sub: p.window_sub_text_fg,
            icon: p.window_bold_fg,
            separator: p.menu_separator_fg,
        }
    }
}

type Handler = Rc<dyn Fn(MenuEvent, &mut Window, &mut App)>;

/// The menu over `area` (the window or its part), with a layer under it that
/// takes clicks outside. `userpic` draws the phone row's userpic.
pub fn render(
    menu: &PopupMenu,
    colors: &MenuColors,
    area: Bounds<Pixels>,
    window: &mut Window,
    userpic: Option<AnyElement>,
    handler: impl Fn(MenuEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let handler: Handler = Rc::new(handler);
    let g = menu.geometry(area, window);
    let st = menu.style();
    let now_ms = menu.shown.elapsed().as_secs_f32() * 1000.;
    let dt = (now_ms / SHOW_MS).min(1.);
    let mut opacity = if dt < OPACITY_DURATION {
        ease_out_circ(dt / OPACITY_DURATION)
    } else {
        1.
    };
    if let Some(h) = menu.hiding {
        opacity *= 1. - (h.elapsed().as_secs_f32() * 1000. / HIDE_MS).min(1.);
    }
    let fw = g.w * (START_WIDTH + (1. - START_WIDTH) * ease_out_circ(dt / WIDTH_DURATION));
    let fh = g.h * (START_HEIGHT + (1. - START_HEIGHT) * ease_out_circ(dt / HEIGHT_DURATION));
    let frame_x = if g.from_right { g.x + g.w - fw } else { g.x };
    let frame_y = if g.from_bottom { g.y + g.h - fh } else { g.y };
    if menu.animating() {
        window.request_animation_frame();
    }
    let bg = hsla(colors.bg);
    let font_h = scale::font(FONT_SIZE);
    let when_font = scale::font(WHEN_FONT_SIZE);
    let [pl, pt, _, _] = st.item_padding.map(lg);
    let mut userpic = userpic;

    let mut rows: Vec<AnyElement> = Vec::with_capacity(menu.entries.len());
    for (i, (entry, (top, h))) in menu.entries.iter().zip(g.rows.iter().copied()).enumerate() {
        let hovered = menu.hovered == Some(i);
        let mut row = div()
            .id(("menu-row", i))
            .absolute()
            .left_0()
            .top(gpx(top))
            .w(gpx(g.w))
            .h(gpx(h))
            .overflow_hidden();
        if entry.clickable() {
            if hovered {
                row = row.bg(hsla(colors.bg_over));
            }
            if let Some(r) = menu.ripple.as_ref().filter(|r| r.row == i) {
                let shown =
                    ease_out_quint(r.start.elapsed().as_secs_f32() * 1000. / RIPPLE_SHOW_MS);
                let fade = r.released.map_or(1., |t| {
                    1. - (t.elapsed().as_secs_f32() * 1000. / RIPPLE_HIDE_MS).min(1.)
                });
                // The farthest corner from the press.
                let (cx, cy) = r.center;
                let reach = [(0., 0.), (g.w, 0.), (0., h), (g.w, h)]
                    .iter()
                    .map(|(x, y)| ((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt())
                    .fold(0., f32::max);
                let radius = reach * shown;
                if fade > 0. {
                    let mut c = hsla(colors.ripple);
                    c.a *= fade;
                    row = row.child(
                        div()
                            .absolute()
                            .left(gpx(cx - radius))
                            .top(gpx(cy - radius))
                            .size(gpx(2. * radius))
                            .rounded_full()
                            .bg(c),
                    );
                }
            }
            let h_ = handler.clone();
            let h2 = handler.clone();
            let h3 = handler.clone();
            let h4 = handler.clone();
            let h5 = handler.clone();
            row = row
                .on_hover(move |over, window, cx| {
                    h_(
                        MenuEvent::Hover {
                            row: i,
                            over: *over,
                        },
                        window,
                        cx,
                    );
                })
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    cx.stop_propagation();
                    h2(
                        MenuEvent::Press {
                            row: i,
                            at: e.position,
                        },
                        window,
                        cx,
                    );
                })
                .on_mouse_down(MouseButton::Right, move |e, window, cx| {
                    cx.stop_propagation();
                    h3(
                        MenuEvent::Press {
                            row: i,
                            at: e.position,
                        },
                        window,
                        cx,
                    );
                })
                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    h4(MenuEvent::Release { row: i }, window, cx);
                })
                .on_mouse_up(MouseButton::Right, move |_, window, cx| {
                    h5(MenuEvent::Release { row: i }, window, cx);
                })
                .cursor_pointer();
        }
        let fg = if hovered { colors.fg_over } else { colors.fg };
        let row = match entry {
            Entry::Action {
                label, icon: kind, ..
            } => row
                .children(kind.map(|k| {
                    div()
                        .absolute()
                        .left(px(st.icon_position.0))
                        .top(px(st.icon_position.1))
                        .child(icon(k, colors.icon, window))
                }))
                .child(
                    div()
                        .absolute()
                        .left(gpx(pl))
                        .top(gpx(pt))
                        .w(gpx(g.w - pl - lg(st.item_padding[2])))
                        .h(font_h.line_height())
                        .text_color(hsla(fg))
                        .child(one_line(label.clone())),
                ),
            Entry::WideSeparator => row.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(4.))
                    .w(gpx(g.w))
                    .h(px(6.))
                    .bg(hsla(colors.separator)),
            ),
            Entry::When { icon: kind, text } => row
                .child(
                    div()
                        .absolute()
                        .left(px(WHEN_ICON.0))
                        .top(px(WHEN_ICON.1))
                        .child(icon(*kind, colors.icon, window)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(WHEN_PADDING[0]))
                        .top(px(WHEN_PADDING[1]))
                        .w(gpx(g.w - lg(WHEN_PADDING[0]) - lg(WHEN_PADDING[2])))
                        .h(when_font.line_height())
                        .text_size(when_font.size)
                        .line_height(when_font.line_height())
                        .text_color(hsla(colors.fg))
                        .child(one_line(text.clone())),
                ),
            Entry::Phone(state) => {
                let text_left = 2. * lg(JOIN_AS_PADDING[0]) + lg(JOIN_AS_PHOTO);
                let inner = g.w - text_left - lg(JOIN_AS_PADDING[2]);
                match state {
                    PhoneRow::Found { name, .. } => row
                        .children(userpic.take().map(|u| {
                            div()
                                .absolute()
                                .left(px(JOIN_AS_PADDING[0]))
                                .top(px(JOIN_AS_PADDING[1]))
                                .child(u)
                        }))
                        .child(
                            div()
                                .absolute()
                                .left(gpx(text_left))
                                .top(px(JOIN_AS_TEXT_TOP))
                                .w(gpx(inner))
                                .h(font_h.line_height())
                                .text_color(hsla(fg))
                                .child(one_line(name.clone())),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(gpx(text_left))
                                .top(px(JOIN_AS_NAME_TOP))
                                .w(gpx(inner))
                                .h(font_h.line_height())
                                .text_color(hsla(colors.sub))
                                .child(one_line("View profile")),
                        ),
                    other => {
                        let text = if *other == PhoneRow::Loading {
                            "Loading..."
                        } else {
                            "This number is not on Telegram"
                        };
                        // Centered, up to two lines (`elisionLines = 2`).
                        let inner = g.w - lg(JOIN_AS_PADDING[0]) - lg(JOIN_AS_PADDING[2]);
                        row.child(
                            div()
                                .absolute()
                                .left(px(JOIN_AS_PADDING[0]))
                                .top_0()
                                .w(gpx(inner))
                                .h(gpx(h))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .w(gpx(inner))
                                        .text_center()
                                        .text_color(hsla(colors.sub))
                                        .child(text),
                                ),
                        )
                    }
                }
            }
            Entry::Status(text) => row.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(gpx(g.w))
                    .h(gpx(h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hsla(colors.sub))
                    .child(text.clone()),
            ),
            Entry::Title { text, .. } => row.child(
                div()
                    .absolute()
                    .left(px(TITLE_POSITION.0))
                    .top(px(TITLE_POSITION.1))
                    .w(gpx(g.w - lg(TITLE_POSITION.0) - lg(st.item_padding[2])))
                    .h(font_h.line_height())
                    .text_color(hsla(fg))
                    .font_weight(FontWeight::NORMAL)
                    .child(one_line(text.clone())),
            ),
        };
        rows.push(row.into_any_element());
    }

    // The items, at their final place inside the growing frame.
    let content = div()
        .absolute()
        .left(gpx(g.x - frame_x))
        .top(gpx(g.y - frame_y))
        .w(gpx(g.w))
        .h(gpx(g.h))
        .children(rows);
    // The soft edge that reveals the items (`_fadeMask`, `_fadeLast`).
    let fade_band = g.h * FADE_HEIGHT;
    let reveal = (g.h * ease_out_circ(dt)).min(fh);
    let mut transparent = bg;
    transparent.a = 0.;
    let (band, solid) = if dt >= 1. {
        (None, None)
    } else if g.from_bottom {
        let band_bottom = fh - reveal;
        let band_top = (band_bottom - fade_band).max(0.);
        (
            Some(
                div()
                    .absolute()
                    .left_0()
                    .top(gpx(band_top))
                    .w(gpx(fw))
                    .h(gpx(band_bottom - band_top))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(bg, 0.),
                        linear_color_stop(transparent, 1.),
                    )),
            ),
            Some(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(gpx(fw))
                    .h(gpx(band_top))
                    .rounded_tl(px(RADIUS))
                    .rounded_tr(px(RADIUS))
                    .bg(bg),
            ),
        )
    } else {
        let band_top = reveal;
        let band_bottom = (band_top + fade_band).min(fh);
        (
            Some(
                div()
                    .absolute()
                    .left_0()
                    .top(gpx(band_top))
                    .w(gpx(fw))
                    .h(gpx(band_bottom - band_top))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(transparent, 0.),
                        linear_color_stop(bg, 1.),
                    )),
            ),
            Some(
                div()
                    .absolute()
                    .left_0()
                    .top(gpx(band_bottom))
                    .w(gpx(fw))
                    .h(gpx((fh - band_bottom).max(0.)))
                    .rounded_bl(px(RADIUS))
                    .rounded_br(px(RADIUS))
                    .bg(bg),
            ),
        )
    };
    let frame = div()
        .id("popup-menu")
        .absolute()
        .left(gpx(frame_x))
        .top(gpx(frame_y))
        .w(gpx(fw))
        .h(gpx(fh))
        .rounded(px(RADIUS))
        .bg(bg)
        // defaultBoxShadow: blur 5, offset (0, 1), 25 % black.
        .shadow(vec![gpui::BoxShadow {
            color: gpui::hsla(0., 0., 0., 0.25),
            offset: point(gpx(0.), px(1.)),
            blur_radius: px(5.),
            spread_radius: gpx(0.),
        }])
        .overflow_hidden()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(content)
        .children(band)
        .children(solid);
    let outside = handler.clone();
    let outside2 = handler;
    div()
        .absolute()
        .left(area.origin.x)
        .top(area.origin.y)
        .w(area.size.width)
        .h(area.size.height)
        .child(
            // Clicks elsewhere close the menu and go no further (the menu grabs input).
            div()
                .id("popup-menu-outside")
                .absolute()
                .left_0()
                .top_0()
                .w(area.size.width)
                .h(area.size.height)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    outside(MenuEvent::Dismiss, window, cx);
                })
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    cx.stop_propagation();
                    outside2(MenuEvent::Dismiss, window, cx);
                }),
        )
        .child(
            div()
                .absolute()
                .left(-area.origin.x)
                .top(-area.origin.y)
                .opacity(opacity)
                .child(frame),
        )
        .into_any_element()
}
