//! Settings (`Settings::Main` in an `Info::LayerWidget`): the account's cover, the
//! sections, the interface scale, Premium and help. The sections and buttons come
//! later (listed in ARCHITECTURE); the "⋮" menu's "Add Account" and "Log out" work.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div,
};

use super::layers::{BOX_RADIUS, TITLE_FONT, box_shadow};
use super::{MainView, TITLE_HEIGHT, c, text_width};
use crate::icons::{self, Icon, icon, icon_width};
use crate::one_line::one_line;
use crate::scale::{self, lg, px};

/// `infoLayerTopBarHeight`, the title at `titlePosition`.
const TOP_BAR: f32 = 56.;
const TOP_TITLE: (f32, f32) = (24., 17.);
/// `infoLayerTopBarMenuPosition`: the "⋮" menu's top right, from the button.
const MENU_POSITION: (f32, f32) = (48., 37.);
/// `settingsPhotoLeft/Top/Bottom`, `infoProfilePhotoInnerSize`, `settingsNameLeft`
/// and the tops of the name, phone and username.
const PHOTO_LEFT: f32 = 22.;
const PHOTO_TOP: f32 = 8.;
const PHOTO_SIZE: f32 = 72.;
const PHOTO_BOTTOM: f32 = 16.;
const NAME_LEFT: f32 = 112.;
const NAME_TOP: f32 = 12.;
const PHONE_TOP: f32 = 37.;
const USERNAME_TOP: f32 = 58.;
/// `infoProfileCover.rightSkip`, `infoProfileLabeledButtonQr` (34 px) and its inset.
const RIGHT_SKIP: f32 = 20.;
const QR_SIZE: f32 = 34.;
const QR_INSET: f32 = 5.;
/// `settingsButton`: padding (60, 10, 22, 10) around a 20 px line, the icon at 20,
/// `toggleSkip`.
const ROW_HEIGHT: f32 = 40.;
const ROW_TEXT_LEFT: f32 = 60.;
const ROW_TEXT_TOP: f32 = 10.;
const ROW_RIGHT: f32 = 22.;
const ROW_ICON_LEFT: f32 = 20.;
const TOGGLE_SKIP: f32 = 20.;
const ROW_FONT: f32 = 14.;
/// `boxDividerHeight`, `defaultVerticalListSkip`.
const DIVIDER: f32 = 8.;
const SKIP: f32 = 6.;
/// `settingsScalePadding`, `settingsScale.seekSize`, the slider's line.
const SCALE_PADDING: [f32; 4] = [60., 7., 22., 4.];
const SEEK: f32 = 15.;
const SLIDER_LINE: f32 = 3.;
/// The premium star: the 14 px font's ascent.
const STAR_SIZE: f32 = 15.;

/// What a row shows on its left.
#[derive(Clone, Copy)]
enum Lead {
    Icon(Icon),
    /// The gradient star: Premium or Stars.
    Star {
        credits: bool,
    },
}

impl MainView {
    /// The top bar's button (`infoLayerTopBar*`): `w` wide, the icon at `x`, a 40 px
    /// round ripple at (0, 8).
    #[allow(clippy::too_many_arguments)]
    fn top_bar_button(
        &self,
        id: &'static str,
        glyph: Icon,
        w: f32,
        x: f32,
        window: &Window,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        // `iconPosition` with y < 0: centered vertically.
        let y = ((TOP_BAR - icons::style_size(glyph).1) / 2.).floor();
        self.icon_button(
            id,
            glyph,
            (w, TOP_BAR),
            Some((x, y)),
            ((0., 8.), 40.),
            (p.box_title_close_fg, p.box_title_close_fg_over),
            window,
            cx,
            on_click,
        )
    }

    /// `Ui::BoxContentDivider`: `boxDividerBg` with the `box_divider_top` and
    /// `box_divider_bottom` shades (5 px strips; their coverage is drawn as rows).
    fn settings_divider(&self) -> gpui::Div {
        const TOP: [u8; 5] = [15, 8, 3, 0, 0];
        const BOTTOM: [u8; 5] = [0, 0, 0, 0, 3];
        let p = self.palette();
        let shade = |coverage: u8| {
            let mut color = c(p.box_divider_fg);
            color.a *= f32::from(coverage) / 255.;
            div().flex_none().h(px(1.)).bg(color)
        };
        let mut d = div()
            .flex_none()
            .relative()
            .h(px(DIVIDER))
            .bg(c(p.box_divider_bg));
        let mut top = div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col();
        for k in TOP {
            top = top.child(shade(k));
        }
        let mut bottom = div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col();
        for k in BOTTOM {
            bottom = bottom.child(shade(k));
        }
        d = d.child(top).child(bottom);
        d
    }

    /// A `SettingsButton` (`settingsButton`): the icon, the text, a label or a toggle
    /// on the right. The sections come later: a click only ripples.
    #[allow(clippy::too_many_arguments)]
    fn settings_row(
        &self,
        id: &'static str,
        lead: Lead,
        text: &'static str,
        label: Option<String>,
        toggle: Option<bool>,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let over = self
            .button_hovered
            .as_ref()
            .is_some_and(|h| h.as_ref() == id);
        let font = scale::font(ROW_FONT);
        let lead_el = match lead {
            Lead::Icon(glyph) => div()
                .absolute()
                .left(px(ROW_ICON_LEFT))
                .top_0()
                .h_full()
                .flex()
                .items_center()
                .child(icon(glyph, p.window_bold_fg, window)),
            Lead::Star { credits } => {
                // `AddPremiumStar`: centered where a menu icon would be.
                let left = lg(ROW_ICON_LEFT)
                    + ((icon_width(Icon::MenuShop, window) - lg(STAR_SIZE)) / 2.).floor();
                let stops: Vec<(f32, tdr_theme::Color)> = if credits {
                    vec![(0., p.credits_bg1), (1., p.credits_bg2)]
                } else {
                    vec![
                        (0., p.premium_button_bg1),
                        (0.6, p.premium_button_bg2),
                        (1., p.premium_button_bg3),
                    ]
                };
                div()
                    .absolute()
                    .left(gpui::px(left))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(icons::premium_star(&stops, STAR_SIZE, window))
            }
        };
        let right = toggle.map(|on| {
            div()
                .absolute()
                .right(px(TOGGLE_SKIP))
                .top_0()
                .h_full()
                .flex()
                .items_center()
                .child(Self::toggle_view(p, on, over))
        });
        let label_el = label.map(|l| {
            div()
                .absolute()
                .right(px(ROW_RIGHT))
                .top(px(ROW_TEXT_TOP))
                .h(px(20.))
                .flex()
                .items_center()
                .text_size(font.size)
                .text_color(c(p.window_active_text_fg))
                .child(l)
        });
        let w = width;
        div()
            .id(id)
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .bg(c(if over { p.window_bg_over } else { p.window_bg }))
            .cursor_pointer()
            .children(self.ripple_layer(id, w, lg(ROW_HEIGHT), p.window_bg_ripple, false))
            .child(lead_el)
            .child(
                div()
                    .absolute()
                    .left(px(ROW_TEXT_LEFT))
                    .right(px(ROW_RIGHT + 60.))
                    .top(px(ROW_TEXT_TOP))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .text_color(c(if over {
                        p.window_bold_fg_over
                    } else {
                        p.window_bold_fg
                    }))
                    .child(one_line(text)),
            )
            .children(label_el)
            .children(right)
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
                cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                    let size = window.viewport_size();
                    let (left, ..) = Self::settings_geometry(
                        f32::from(size.width),
                        f32::from(size.height) - lg(TITLE_HEIGHT),
                    );
                    this.press_button(
                        id.into(),
                        (f32::from(e.position.x) - left, lg(ROW_HEIGHT) / 2.),
                    );
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
            .on_click(cx.listener(|this, _, _, cx| {
                this.release_buttons();
                cx.notify();
            }))
    }

    /// `SetupInterfaceScale`'s slider: the scale in use, among 50 %..300 % in 5 %
    /// steps, its value on the right.
    fn scale_slider(&self, width: f32, window: &Window) -> gpui::Div {
        let p = self.palette();
        let percent = (scale::get() * 100.).round();
        let label = format!("{percent}%");
        let label_font = 13.;
        let space = text_width(window, " ", label_font, FontWeight::NORMAL).max(lg(3.));
        let label_w = text_width(window, &label, label_font, FontWeight::NORMAL).max(text_width(
            window,
            "300%",
            label_font,
            FontWeight::NORMAL,
        ));
        let [pl, pt, pr, pb] = SCALE_PADDING;
        let row_h = lg(SEEK).max(scale::font(label_font).height);
        let slider_w = (width - lg(pl) - lg(pr) - label_w - 2. * space).max(lg(SEEK));
        let t = ((percent - 50.) / 250.).clamp(0., 1.);
        let seek = lg(SEEK);
        let x = seek / 2. + (slider_w - seek) * t;
        let line = lg(SLIDER_LINE);
        let line_top = ((row_h - line) / 2.).floor();
        let font = scale::font(label_font);
        div()
            .flex_none()
            .relative()
            .h(gpui::px(lg(pt) + row_h + lg(pb)))
            .child(
                div()
                    .absolute()
                    .left(px(pl))
                    .top(px(pt))
                    .w(gpui::px(slider_w))
                    .h(gpui::px(row_h))
                    .child(
                        div()
                            .absolute()
                            .left(gpui::px(seek / 2.))
                            .right(gpui::px(seek / 2.))
                            .top(gpui::px(line_top))
                            .h(gpui::px(line))
                            .rounded(gpui::px(line / 2.))
                            .bg(c(p.media_player_inactive_fg)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(gpui::px(seek / 2.))
                            .w(gpui::px(x - seek / 2.))
                            .top(gpui::px(line_top))
                            .h(gpui::px(line))
                            .rounded(gpui::px(line / 2.))
                            .bg(c(p.media_player_active_fg)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(gpui::px(x - seek / 2.))
                            .top(gpui::px(((row_h - seek) / 2.).floor()))
                            .size(gpui::px(seek))
                            .rounded_full()
                            .bg(c(p.media_player_active_fg)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(pr))
                    .top(px(pt))
                    .h(gpui::px(row_h))
                    .flex()
                    .items_center()
                    .text_size(font.size)
                    .text_color(c(p.window_active_text_fg))
                    .child(label),
            )
    }

    /// The cover (`Settings::Cover`): userpic, name, phone, username, QR button.
    fn settings_cover(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.palette();
        let me = self.accounts.read(cx).active().and_then(|s| s.me.clone());
        let name = me.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let phone = me
            .as_ref()
            .and_then(|m| m.phone.clone())
            .unwrap_or_default();
        let username = me
            .as_ref()
            .and_then(|m| m.username.clone())
            .map_or_else(|| "Add username".to_string(), |u| format!("@{u}"));
        let name_font = scale::font(TITLE_FONT);
        let font = scale::font(13.);
        let qr = div()
            .absolute()
            .right(px(RIGHT_SKIP - QR_INSET))
            .top(px(
                ((PHOTO_TOP + PHOTO_SIZE + PHOTO_BOTTOM - QR_SIZE) / 2.).floor()
            ))
            .child(self.icon_button(
                "settings-qr",
                Icon::MenuQrCode,
                (QR_SIZE, QR_SIZE),
                None,
                ((0., 0.), QR_SIZE),
                (p.window_active_text_fg, p.window_active_text_fg),
                window,
                cx,
                |_, _, _| {},
            ));
        let text = |top: f32, size: scale::Font, weight: FontWeight, color, s: String| {
            div()
                .absolute()
                .left(px(NAME_LEFT))
                .right(px(RIGHT_SKIP + QR_SIZE + RIGHT_SKIP))
                .top(px(top))
                .text_size(size.size)
                .line_height(size.line_height())
                .font_weight(weight)
                .text_color(c(color))
                .child(one_line(s))
        };
        div()
            .flex_none()
            .relative()
            .h(px(PHOTO_TOP + PHOTO_SIZE + PHOTO_BOTTOM))
            .child(
                div()
                    .absolute()
                    .left(px(PHOTO_LEFT))
                    .top(px(PHOTO_TOP))
                    .child(self.active_userpic(PHOTO_SIZE, cx)),
            )
            .child(text(
                NAME_TOP,
                name_font,
                FontWeight::SEMIBOLD,
                p.window_bold_fg,
                name,
            ))
            .child(text(
                PHONE_TOP,
                font,
                FontWeight::NORMAL,
                p.window_sub_text_fg,
                phone,
            ))
            .child(text(
                USERNAME_TOP,
                font,
                FontWeight::NORMAL,
                p.window_sub_text_fg,
                username,
            ))
            .child(qr)
    }

    /// The Settings layer, `width` wide and at most `max_h` tall.
    pub(super) fn render_settings(
        &self,
        width: f32,
        max_h: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let skip = || div().flex_none().h(px(SKIP));
        let row = |this: &Self, id, lead, text, label: Option<String>, cx: &mut Context<Self>| {
            this.settings_row(id, lead, text, label, None, width, window, cx)
        };
        let mut content = div()
            .flex()
            .flex_col()
            .child(self.settings_cover(window, cx))
            .child(self.settings_divider())
            .child(skip());
        let sections = [
            ("settings-account", Icon::MenuProfile, "My Account"),
            (
                "settings-notify",
                Icon::MenuNotifications,
                "Notifications and Sounds",
            ),
            ("settings-privacy", Icon::MenuLock, "Privacy and Security"),
            ("settings-chat", Icon::MenuChatBubble, "Chat Settings"),
            ("settings-folders", Icon::MenuShowInFolder, "Folders"),
            ("settings-advanced", Icon::MenuManage, "Advanced"),
            ("settings-devices", Icon::MenuUnmute, "Speakers and Camera"),
            (
                "settings-power",
                Icon::MenuPowerUsage,
                "Battery and Animations",
            ),
        ];
        for (id, glyph, text) in sections {
            content = content.child(row(self, id, Lead::Icon(glyph), text, None, cx));
        }
        content = content
            .child(row(
                self,
                "settings-language",
                Lead::Icon(Icon::MenuTranslate),
                "Language",
                Some("English".into()),
                cx,
            ))
            .child(skip())
            // `BuildInterfaceScale`.
            .child(self.settings_divider())
            .child(skip())
            .child(self.settings_row(
                "settings-scale",
                Lead::Icon(Icon::MenuShowInChat),
                "Default interface scale",
                None,
                Some(true),
                width,
                window,
                cx,
            ))
            .child(self.scale_slider(width, window))
            .child(skip())
            // `BuildPremiumSection`.
            .child(self.settings_divider())
            .child(skip())
            .child(row(
                self,
                "settings-premium",
                Lead::Star { credits: false },
                "Telegram Premium",
                None,
                cx,
            ))
            .child(row(
                self,
                "settings-stars",
                Lead::Star { credits: true },
                "My Stars",
                None,
                cx,
            ))
            .child(row(
                self,
                "settings-business",
                Lead::Icon(Icon::MenuShop),
                "Telegram Business",
                None,
                cx,
            ))
            .child(row(
                self,
                "settings-gift",
                Lead::Icon(Icon::MenuGiftPremium),
                "Send a Gift",
                None,
                cx,
            ))
            .child(skip())
            // `BuildHelpSection`.
            .child(self.settings_divider())
            .child(skip())
            .child(row(
                self,
                "settings-faq",
                Lead::Icon(Icon::MenuFaq),
                "Telegram FAQ",
                None,
                cx,
            ))
            .child(row(
                self,
                "settings-features",
                Lead::Icon(Icon::MenuEmojiObjects),
                "Telegram Features",
                None,
                cx,
            ))
            .child(row(
                self,
                "settings-ask",
                Lead::Icon(Icon::MenuDiscussion),
                "Ask a Question",
                None,
                cx,
            ))
            .child(skip());

        let title_font = scale::font(TITLE_FONT);
        let (box_left, box_top, ..) = {
            let size = window.viewport_size();
            Self::settings_geometry(
                f32::from(size.width),
                f32::from(size.height) - lg(TITLE_HEIGHT),
            )
        };
        let top_bar = div()
            .flex_none()
            .relative()
            .h(px(TOP_BAR))
            .child(
                div()
                    .absolute()
                    .left(px(TOP_TITLE.0))
                    .top(px(TOP_TITLE.1))
                    .text_size(title_font.size)
                    .line_height(title_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.box_title_fg))
                    .child("Settings"),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .flex()
                    .child(self.top_bar_button(
                        "settings-search",
                        Icon::TopBarSearch,
                        48.,
                        7.,
                        window,
                        cx,
                        |_, _, _| {},
                    ))
                    .child(self.top_bar_button(
                        "settings-menu",
                        Icon::MenuDots,
                        40.,
                        16.,
                        window,
                        cx,
                        move |this, _, cx| {
                            // The menu's top right at `infoLayerTopBarMenuPosition` from
                            // the button.
                            let toggle_left = box_left + width - lg(48.) - lg(40.);
                            let at = gpui::point(
                                gpui::px(toggle_left + lg(MENU_POSITION.0)),
                                gpui::px(lg(TITLE_HEIGHT) + box_top + lg(MENU_POSITION.1)),
                            );
                            this.open_settings_menu(at, cx);
                        },
                    ))
                    .child(self.top_bar_button(
                        "settings-close",
                        Icon::InfoClose,
                        48.,
                        4.,
                        window,
                        cx,
                        |this, _, _| this.layers.hide_all(),
                    )),
            );
        div()
            .id("settings-layer")
            .occlude()
            .w(gpui::px(width))
            .max_h(gpui::px(max_h))
            .flex()
            .flex_col()
            .bg(c(p.box_bg))
            .rounded(px(BOX_RADIUS))
            .shadow(box_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(top_bar)
            .child(
                div()
                    .id("settings-scroll")
                    .flex_shrink()
                    .min_h(gpui::px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.layers.settings_scroll)
                    .child(content),
            )
            .child(div().flex_none().h(px(BOX_RADIUS)))
            .into_any_element()
    }
}
