//! tdesktop's `LayerStackWidget`: the main menu sliding in from the left, a
//! "special layer" (Settings, `Info::LayerWidget`) and a box on top (About, the
//! logout confirmation), over a dim background.
//!
//! Animations follow `LayerStackWidget::BackgroundWidget`: everything takes
//! `boxDuration` (200 ms); the menu's visible width goes with easeOutCirc and it
//! shows its right part as it slides; boxes fade (linear); the dim fades in with
//! easeOutCirc and out linearly; a box showing while the menu hides is moved right
//! by half the menu's visible width.

use gpui::{
    AnyElement, BoxShadow, Context, FontWeight, HighlightStyle, InteractiveElement,
    InteractiveText, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement, Styled,
    StyledText, UnderlineStyle, Window, div, hsla, point, prelude::FluentBuilder,
};
use tdr_theme::Color;

use super::{MainView, TITLE_HEIGHT, c, main_menu::MENU_WIDTH, text_width};
use crate::anim::{self, Simple};
use crate::scale::{self, lg, px};

/// `boxDuration`.
const DURATION: f32 = 200.;
/// `boxRadius`.
pub(super) const BOX_RADIUS: f32 = 8.;
/// `boxWidth`, `boxPadding`, `boxLabelStyle`'s line height.
const BOX_WIDTH: f32 = 320.;
const BOX_PADDING: [f32; 4] = [24., 14., 24., 8.];
const BOX_LABEL_LINE: f32 = 22.;
/// `defaultBox`: margin top, the buttons' padding (left, top, right, bottom) and height.
const BOX_MARGIN_TOP: f32 = 10.;
const BUTTON_PADDING: [f32; 4] = [6., 10., 10., 10.];
const BUTTON_HEIGHT: f32 = 34.;
/// `defaultBoxButton`: `width: -30px` (text plus 30), `textTop`, 14 px semibold.
const BUTTON_TEXT_PAD: f32 = 30.;
const BUTTON_TEXT_TOP: f32 = 7.;
const BUTTON_FONT: f32 = 14.;
/// `buttonRadius`.
const BUTTON_RADIUS: f32 = 4.;
/// `boxTitlePosition`, `boxTitleHeight`, `boxTitleFont`.
pub(super) const TITLE_POSITION: (f32, f32) = (24., 13.);
const TITLE_HEIGHT_BOX: f32 = 48.;
pub(super) const TITLE_FONT: f32 = 16.;
/// `aboutWidth`, `aboutTopSkip`, `aboutSkip`, `aboutLabel` (13 px, 22 px lines),
/// `boxRowPadding`.
const ABOUT_WIDTH: f32 = 390.;
const ABOUT_TOP_SKIP: f32 = 19.;
const ABOUT_SKIP: f32 = 14.;
const ABOUT_LINE: f32 = 22.;
const ROW_PADDING: f32 = 24.;

/// The special layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Special {
    Settings,
    /// The login's settings (`Settings::LayerWidget` over the intro).
    IntroSettings,
}

/// A box over everything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LayerBox {
    About,
    /// "Are you sure you want to log out?" for this account.
    LogOut(u32),
    /// The login's country list.
    Countries,
}

/// `LayerStackWidget::BackgroundWidget::Action`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    ShowMainMenu,
    ShowSpecial(Special),
    ShowLayer(LayerBox),
    HideLayer,
    HideAll,
}

#[derive(Default)]
pub(super) struct Layers {
    menu: bool,
    special: Option<Special>,
    layer: Option<LayerBox>,
    /// Still drawn while they fade out.
    special_drawn: Option<Special>,
    layer_drawn: Option<LayerBox>,
    a_menu: Option<Simple>,
    a_special: Option<Simple>,
    a_layer: Option<Simple>,
    a_bg: Option<Simple>,
    /// The link under the mouse in a label: (label, link index).
    pub link_hover: Option<(&'static str, usize)>,
    pub settings_scroll: gpui::ScrollHandle,
}

impl Layers {
    fn shown(&self) -> bool {
        self.menu || self.special.is_some() || self.layer.is_some()
    }

    pub fn menu_open(&self) -> bool {
        self.menu
    }

    pub fn any(&self) -> bool {
        self.shown()
    }

    /// `BackgroundWidget::startAnimation`.
    fn start(&mut self, action: Action) {
        let was_shown = self.shown();
        let fade = |on: bool| {
            Some(Simple::new(
                f32::from(!on),
                f32::from(on),
                DURATION,
                anim::linear,
            ))
        };
        let menu = match action {
            Action::ShowMainMenu => true,
            Action::HideLayer => self.menu,
            _ => false,
        };
        if menu != self.menu {
            self.menu = menu;
            self.a_menu = Some(Simple::new(
                f32::from(!menu),
                f32::from(menu),
                DURATION,
                anim::ease_out_circ,
            ));
        }
        let special = match action {
            Action::ShowSpecial(s) => Some(s),
            Action::ShowMainMenu | Action::HideAll => None,
            _ => self.special,
        };
        if special.is_some() != self.special.is_some() {
            self.a_special = fade(special.is_some());
        }
        if special.is_some() {
            self.special_drawn = special;
        }
        self.special = special;
        let layer = match action {
            Action::ShowLayer(b) => Some(b),
            Action::ShowSpecial(_) => self.layer,
            _ => None,
        };
        if layer.is_some() != self.layer.is_some() {
            self.a_layer = fade(layer.is_some());
        }
        if layer.is_some() {
            self.layer_drawn = layer;
        }
        self.layer = layer;
        // `checkWasShown`: hiding fades the dim linearly, matching the boxes.
        if self.shown() != was_shown {
            self.a_bg = Some(Simple::new(
                f32::from(was_shown),
                f32::from(!was_shown),
                DURATION,
                if was_shown {
                    anim::linear
                } else {
                    anim::ease_out_circ
                },
            ));
        }
        self.link_hover = None;
    }

    pub fn show_main_menu(&mut self) {
        self.start(Action::ShowMainMenu);
    }

    pub fn show_special(&mut self, special: Special) {
        if self.special != Some(special) {
            self.settings_scroll = gpui::ScrollHandle::new();
        }
        self.start(Action::ShowSpecial(special));
    }

    pub fn show_layer(&mut self, layer: LayerBox) {
        self.start(Action::ShowLayer(layer));
    }

    pub fn hide_layer(&mut self) {
        self.start(Action::HideLayer);
    }

    pub fn hide_all(&mut self) {
        if self.shown() {
            self.start(Action::HideAll);
        }
    }

    /// `hideCurrent`: the box, or else everything.
    pub fn hide_current(&mut self) {
        if self.layer.is_some() {
            self.hide_layer();
        } else {
            self.hide_all();
        }
    }

    /// Gone at once (the window shows something else).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn animating(&self) -> bool {
        [&self.a_menu, &self.a_special, &self.a_layer, &self.a_bg]
            .iter()
            .any(|a| a.as_ref().is_some_and(Simple::animating))
    }

    fn menu_progress(&self) -> f32 {
        anim::value(&self.a_menu, f32::from(self.menu))
    }

    fn special_opacity(&self) -> f32 {
        anim::value(&self.a_special, f32::from(self.special.is_some()))
    }

    fn layer_opacity(&self) -> f32 {
        anim::value(&self.a_layer, f32::from(self.layer.is_some()))
    }

    fn bg_opacity(&self) -> f32 {
        anim::value(&self.a_bg, f32::from(self.shown()))
    }

    /// Drops what has faded out.
    fn settle(&mut self) {
        if self.special.is_none() && !self.a_special.is_some_and(|a| a.animating()) {
            self.special_drawn = None;
        }
        if self.layer.is_none() && !self.a_layer.is_some_and(|a| a.animating()) {
            self.layer_drawn = None;
        }
    }
}

/// A piece of a label: text, and what it opens when it is a link.
pub(super) enum Part {
    Text(&'static str),
    Link(String, LinkTarget),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum LinkTarget {
    Url(String),
    About,
}

/// The repository's pages (every link to the repository is made here).
pub(super) fn repo_url(path: &str) -> String {
    if path.is_empty() {
        crate::REPO_URL.to_string()
    } else {
        format!("{}/{path}", crate::REPO_URL)
    }
}

/// The boxRoundShadow look (`round_shadow_box_*`, 10 px around).
pub(super) fn box_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.2),
        offset: point(gpui::px(0.), gpui::px(lg(1.))),
        blur_radius: gpui::px(lg(10.)),
        spread_radius: gpui::px(0.),
    }]
}

impl MainView {
    pub(super) fn toggle_main_menu(&mut self) {
        if self.layers.menu_open() {
            self.layers.hide_all();
        } else {
            self.layers.show_main_menu();
        }
    }

    /// A `FlatLabel` with links: wrapped at `width`, links in `link` and underlined
    /// under the mouse, clicks open them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn linked_label(
        &self,
        id: &'static str,
        parts: Vec<Part>,
        width: Option<f32>,
        size: f32,
        line: Option<f32>,
        weight: FontWeight,
        (fg, link): (Color, Color),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text = String::new();
        let mut ranges = Vec::new();
        let mut targets = Vec::new();
        for part in parts {
            match part {
                Part::Text(t) => text.push_str(t),
                Part::Link(t, target) => {
                    let start = text.len();
                    text.push_str(&t);
                    ranges.push(start..text.len());
                    targets.push(target);
                }
            }
        }
        let hovered = self
            .layers
            .link_hover
            .filter(|(label, _)| *label == id)
            .map(|(_, i)| i);
        let highlights: Vec<_> = ranges
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.clone(),
                    HighlightStyle {
                        color: Some(c(link).into()),
                        underline: (hovered == Some(i)).then(|| UnderlineStyle {
                            thickness: gpui::px(lg(1.)),
                            color: Some(c(link).into()),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let styled = StyledText::new(text).with_highlights(highlights);
        let view = cx.entity().downgrade();
        let hover_view = view.clone();
        let hover_ranges = ranges.clone();
        let label = InteractiveText::new(id, styled)
            .on_click(ranges, move |i, window, cx| {
                let target = targets[i].clone();
                let _ = view.update(cx, |this, cx| this.open_link_target(target, window, cx));
            })
            .on_hover(move |ix, _, _, cx| {
                let over = ix.and_then(|ix| hover_ranges.iter().position(|r| r.contains(&ix)));
                let _ = hover_view.update(cx, |this, cx| {
                    let now = over.map(|i| (id, i));
                    let mine = this.layers.link_hover.is_some_and(|(l, _)| l == id);
                    if this.layers.link_hover != now && (now.is_some() || mine) {
                        this.layers.link_hover = now;
                        cx.notify();
                    }
                });
            });
        let font = scale::font(size);
        div()
            .when_some(width, |d, w| d.w(gpui::px(w)))
            .text_size(font.size)
            .line_height(line.map_or(font.line_height(), px))
            .font_weight(weight)
            .text_color(c(fg))
            .child(label)
            .into_any_element()
    }

    fn open_link_target(
        &mut self,
        target: LinkTarget,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            LinkTarget::Url(url) => cx.open_url(&url),
            LinkTarget::About => self.layers.show_layer(LayerBox::About),
        }
        cx.notify();
    }

    /// A `RoundButton` of a box (`defaultBoxButton` / `attentionBoxButton`).
    pub(super) fn box_button(
        &self,
        id: &'static str,
        text: &'static str,
        attention: bool,
        window: &Window,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == id);
        let (fg, fg_over, bg_over, ripple) = if attention {
            (
                p.attention_button_fg,
                p.attention_button_fg_over,
                p.attention_button_bg_over,
                p.attention_button_bg_ripple,
            )
        } else {
            (
                p.light_button_fg,
                p.light_button_fg_over,
                p.light_button_bg_over,
                p.light_button_bg_ripple,
            )
        };
        let w = text_width(window, text, BUTTON_FONT, FontWeight::SEMIBOLD) + lg(BUTTON_TEXT_PAD);
        let h = lg(BUTTON_HEIGHT);
        let font = scale::font(BUTTON_FONT);
        div()
            .id(id)
            .relative()
            .flex_none()
            .w(gpui::px(w))
            .h(gpui::px(h))
            .rounded(px(BUTTON_RADIUS))
            .overflow_hidden()
            .bg(c(if over { bg_over } else { p.light_button_bg }))
            .cursor_pointer()
            .children(self.ripple_layer(id, w, h, ripple, false))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(BUTTON_TEXT_TOP))
                    .flex()
                    .justify_center()
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(if over { fg_over } else { fg }))
                    .child(text),
            )
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
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let _ = e;
                    this.press_button(id.into(), (w / 2., h / 2.));
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.release_buttons();
                on_click(this, window, cx);
                cx.notify();
            }))
    }

    /// A box's frame: its background, rounding, shadow, and the buttons row.
    pub(super) fn box_frame(
        &self,
        width: f32,
        body: gpui::Div,
        buttons: Vec<gpui::Stateful<gpui::Div>>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let [bl, bt, br, bb] = BUTTON_PADDING;
        let mut row = div()
            .flex_none()
            .flex()
            .flex_row_reverse()
            .pt(px(bt))
            .pb(px(bb))
            .pr(px(br))
            .h(px(bt + BUTTON_HEIGHT + bb));
        for (i, b) in buttons.into_iter().enumerate() {
            row = row.child(if i > 0 { b.mr(px(bl)) } else { b });
        }
        div()
            .id("layer-box")
            .occlude()
            .w(px(width))
            .flex()
            .flex_col()
            .bg(c(p.box_bg))
            .rounded(px(BOX_RADIUS))
            .shadow(box_shadow())
            // Clicks inside stay in the box (`LayerWidget::mousePressEvent`).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body)
            .child(row)
    }

    /// `AboutBox`, about this client.
    fn render_about(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let width = ABOUT_WIDTH;
        let text_w = lg(width) - 2. * lg(ROW_PADDING);
        let title_font = scale::font(TITLE_FONT);
        let label = |this: &Self, id, parts, cx: &mut Context<Self>| {
            div()
                .pl(px(ROW_PADDING))
                .pb(px(ABOUT_SKIP))
                .child(this.linked_label(
                    id,
                    parts,
                    Some(text_w),
                    13.,
                    Some(ABOUT_LINE),
                    FontWeight::NORMAL,
                    (p.window_fg, p.window_active_text_fg),
                    cx,
                ))
        };
        let version = format!("version {} x64", env!("CARGO_PKG_VERSION"));
        let body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(TITLE_HEIGHT_BOX))
                    .pl(px(TITLE_POSITION.0))
                    .pt(px(TITLE_POSITION.1))
                    .text_size(title_font.size)
                    .line_height(title_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.box_title_fg))
                    .child(crate::APP_NAME),
            )
            // The version (`aboutVersionLink`, raised by 3 px): the releases.
            .child(
                div()
                    .pl(px(ROW_PADDING))
                    .mt(px(-3.))
                    .child(self.linked_label(
                        "about-version",
                        vec![Part::Link(version, LinkTarget::Url(repo_url("releases")))],
                        None,
                        13.,
                        None,
                        FontWeight::NORMAL,
                        (p.window_sub_text_fg, p.window_sub_text_fg),
                        cx,
                    )),
            )
            .child(div().h(px(ABOUT_TOP_SKIP)))
            .child(label(
                self,
                "about-1",
                vec![
                    Part::Text("Unofficial free messaging app based on "),
                    Part::Link(
                        "Telegram API".into(),
                        LinkTarget::Url("https://core.telegram.org/api".into()),
                    ),
                    Part::Text(
                        ", a port of Telegram Desktop to Rust. It is not affiliated with Telegram.",
                    ),
                ],
                cx,
            ))
            .child(label(
                self,
                "about-2",
                vec![
                    Part::Text("This software is licensed under "),
                    Part::Link(
                        "GNU GPL".into(),
                        LinkTarget::Url(repo_url("blob/main/LICENSE")),
                    ),
                    Part::Text(" version 3. Source code is available on "),
                    Part::Link("GitHub".into(), LinkTarget::Url(repo_url(""))),
                    Part::Text("."),
                ],
                cx,
            ))
            .child(label(
                self,
                "about-3",
                vec![
                    Part::Text("Visit the "),
                    Part::Link(
                        "Telegram FAQ".into(),
                        LinkTarget::Url("https://telegram.org/faq".into()),
                    ),
                    Part::Text(" for more info."),
                ],
                cx,
            ));
        let close = self.box_button("about-close", "Close", false, window, cx, |this, _, _| {
            this.layers.hide_layer();
        });
        self.box_frame(width, body, vec![close]).into_any_element()
    }

    /// `Window::Controller::showLogoutConfirmation`: a `ConfirmBox`.
    fn render_logout(&self, account: u32, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let font = scale::font(14.);
        let [pl, pt, pr, pb] = BOX_PADDING;
        let body = div()
            .pt(px(BOX_MARGIN_TOP + pt))
            .pl(px(pl))
            .pr(px(pr))
            .pb(px(pb))
            .text_size(font.size)
            .line_height(px(BOX_LABEL_LINE))
            .text_color(c(p.box_text_fg))
            .child("Are you sure you want to log out?");
        let confirm = self.box_button(
            "logout-confirm",
            "Log out",
            true,
            window,
            cx,
            move |this, _, cx| {
                this.layers.hide_all();
                this.accounts.update(cx, |a, cx| a.log_out(account, cx));
            },
        );
        let cancel = self.box_button(
            "logout-cancel",
            "Cancel",
            false,
            window,
            cx,
            |this, _, _| {
                this.layers.hide_layer();
            },
        );
        self.box_frame(BOX_WIDTH, body, vec![confirm, cancel])
            .into_any_element()
    }

    /// The layers over the window's body (below the title bar).
    pub(super) fn render_layers(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.layers.settle();
        if self.layers.animating() {
            window.request_animation_frame();
        }
        let menu_p = self.layers.menu_progress();
        let bg = self.layers.bg_opacity();
        let special = self.layers.special_drawn;
        let layer = self.layers.layer_drawn;
        if menu_p <= 0. && bg <= 0. && special.is_none() && layer.is_none() {
            return None;
        }
        let p = self.palette();
        let size = window.viewport_size();
        let (w, h) = (
            f32::from(size.width),
            f32::from(size.height) - lg(TITLE_HEIGHT),
        );
        let menu_w = lg(MENU_WIDTH);
        let menu_right = (menu_w * menu_p).round();
        // Boxes showing while the menu hides move right by half its visible part.
        let shift = if self.layers.a_menu.is_some_and(|a| a.animating()) {
            (menu_right / 2.).round()
        } else {
            0.
        };
        let special_op = self.layers.special_opacity();
        let layer_op = self.layers.layer_opacity();

        // Below the title bar, over the whole window body.
        let mut root = div()
            .absolute()
            .top(gpui::px(lg(TITLE_HEIGHT)))
            .left_0()
            .w(gpui::px(w))
            .h(gpui::px(h))
            .overflow_hidden()
            .child(
                div()
                    .id("layer-dim")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::px(menu_right))
                    .w(gpui::px(w - menu_right))
                    .bg(c(p.layer_bg))
                    .opacity(bg)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            // `backgroundClicked`.
                            this.layers.hide_current();
                            cx.notify();
                        }),
                    ),
            );
        if menu_right > 0. {
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(menu_right - menu_w))
                    .w(gpui::px(menu_w))
                    .h_full()
                    .shadow(box_shadow())
                    .child(self.render_main_menu(window, cx)),
            );
        }
        if let Some(special) = special {
            let (left, top, width, max_h) = Self::settings_geometry(w, h);
            let over = layer.map(|_| layer_op * special_op).unwrap_or(0.);
            root = root.child(
                div()
                    .absolute()
                    .left(gpui::px(left + shift))
                    .top(gpui::px(top))
                    .w(gpui::px(width))
                    .opacity(special_op)
                    .child(match special {
                        Special::Settings => self.render_settings(width, max_h, window, cx),
                        Special::IntroSettings => {
                            self.render_intro_settings(width, max_h, window, cx)
                        }
                    })
                    .when(over > 0., |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .rounded(px(BOX_RADIUS))
                                .bg(c(p.layer_bg))
                                .opacity(over),
                        )
                    }),
            );
        }
        if let Some(b) = layer {
            let content = match b {
                LayerBox::About => self.render_about(window, cx),
                LayerBox::LogOut(id) => self.render_logout(id, window, cx),
                LayerBox::Countries => self.render_country_box(window, cx),
            };
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(shift))
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(layer_op)
                    .child(content),
            );
        }
        Some(root.into_any_element())
    }

    /// `Info::LayerWidget::countGeometry`: left, top, width and the most height.
    pub(super) fn settings_geometry(w: f32, h: f32) -> (f32, f32, f32, f32) {
        let width = (w - 2. * lg(48.)).min(lg(392.)).max(lg(200.)).floor();
        let left = ((w - width) / 2.).floor();
        let top = (h / 24.).floor().clamp(lg(20.), lg(40.));
        let max_h = (h - 2. * top).max(lg(100.));
        (left, top, width, max_h)
    }
}
