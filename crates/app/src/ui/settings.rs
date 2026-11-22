//! Settings (`Settings::Main` in an `Info::LayerWidget`): the account's cover, the
//! sections, the interface scale, Premium and help. The sections and buttons come
//! later (listed in ARCHITECTURE); the "⋮" menu's "Add Account" and "Log out" work.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
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
    /// `settingsButtonNoIcon`: the text at 22, padding (22, 10, 22, 8).
    None,
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
        let (text_left, row_height) = match lead {
            Lead::None => (ROW_NO_ICON_LEFT, ROW_NO_ICON_HEIGHT),
            _ => (ROW_TEXT_LEFT, ROW_HEIGHT),
        };
        let lead_el = match lead {
            Lead::None => div(),
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
            .h(px(row_height))
            .overflow_hidden()
            .bg(c(if over { p.window_bg_over } else { p.window_bg }))
            .cursor_pointer()
            .children(self.ripple_layer(id, w, lg(row_height), p.window_bg_ripple, false))
            .child(lead_el)
            .child(
                div()
                    .absolute()
                    .left(px(text_left))
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
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    let left = this.layers.settings_left.get();
                    this.press_button(
                        id.into(),
                        (f32::from(e.position.x) - left, lg(row_height) / 2.),
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
    fn scale_slider(&self, width: f32, big: bool, window: &Window) -> gpui::Div {
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
        let [pl, pt, pr, pb] = if big {
            BIG_SCALE_PADDING
        } else {
            SCALE_PADDING
        };
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
        let username = me.as_ref().and_then(|m| m.username.clone());
        let own = me.as_ref().map(|m| m.peer);
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
        // `Cover`: the name and the phone are selectable labels with "Copy Name" /
        // "Copy Phone Number" in their menus; the username is a link copying the
        // profile's link ("Add username" without one).
        let width = (self.layers.settings_width.get()
            - lg(NAME_LEFT)
            - lg(RIGHT_SKIP + QR_SIZE + RIGHT_SKIP))
        .max(lg(40.));
        let place =
            |top: f32, el: AnyElement| div().absolute().left(px(NAME_LEFT)).top(px(top)).child(el);
        let phone_hidden = me.as_ref().is_some_and(|m| m.phone_hidden);
        let name_el = own.map(|peer| {
            self.selectable_label(
                (peer, LABEL_NAME),
                &name,
                (TITLE_FONT, true, p.window_bold_fg),
                width,
                CoverLabel::Name,
                window,
                cx,
            )
        });
        let phone_el = own.map(|peer| {
            self.selectable_label(
                (peer, LABEL_PHONE),
                &phone,
                (13., false, p.window_fg),
                width,
                CoverLabel::Phone {
                    hidden: phone_hidden,
                },
                window,
                cx,
            )
        });
        use super::layers::{LinkTarget, Part};
        let username_el = self.linked_label(
            "settings-username",
            vec![match &username {
                Some(u) => Part::Link(
                    format!("@{u}"),
                    LinkTarget::CopyLink(format!("https://t.me/{u}")),
                ),
                None => Part::Link("Add username".into(), LinkTarget::None),
            }],
            None,
            13.,
            None,
            FontWeight::NORMAL,
            (p.window_sub_text_fg, p.window_sub_text_fg),
            cx,
        );
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
            .children(name_el.map(|el| place(NAME_TOP, el)))
            .children(phone_el.map(|el| place(PHONE_TOP, el)))
            .child(place(USERNAME_TOP, username_el))
            .child(qr)
    }

    /// A selectable one-line `FlatLabel` (`setSelectable`): drag, double and triple
    /// clicks select, Ctrl+C copies, its menu copies the selection or `copy` the
    /// whole text.
    #[allow(clippy::too_many_arguments)]
    fn selectable_label(
        &self,
        key: super::message_text::TextKey,
        text: &str,
        (size, semibold, fg): (f32, bool, tdr_theme::Color),
        width: f32,
        label: CoverLabel,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::rich_text::{Kind, TextColors, TextEntity};
        let p = self.palette();
        let mut entities = Vec::new();
        if semibold && !text.is_empty() {
            entities.push(TextEntity {
                range: 0..text.len(),
                kind: Kind::Bold,
            });
        }
        // `updatePhoneText`: the hidden number is one spoiler (a click shows it).
        if label == (CoverLabel::Phone { hidden: true }) && !text.is_empty() {
            entities.push(TextEntity {
                range: 0..text.len(),
                kind: Kind::Spoiler,
            });
        }
        let measured = self.rich_measured_sized(key, text, &entities, size, window);
        let layout = self.rich_layout(key, &measured, width, (0., 0.));
        let (state, dragging) = self.text_state(key);
        // `defaultTextPalette`: the selection over the text in its own color.
        let colors = TextColors {
            fg,
            link: fg,
            mono: fg,
            select_bg: p.msg_in_bg_selected,
            select_fg: fg,
            select_link: fg,
            select_mono: fg,
            spoiler: fg,
            select_spoiler: fg,
            quote: fg,
            dark: self.dark,
            syntax: [fg; 8],
            copy_hint: false,
        };
        let view = cx.entity().downgrade();
        let handler_layout = layout.clone();
        let full = text.to_string();
        crate::rich_text::view(
            gpui::SharedString::from(format!("label-{}-{}", key.0.id, key.1)),
            layout,
            colors,
            state,
            dragging,
            move |event, window, cx| {
                let full = full.clone();
                let _ = view.update(cx, |this, cx| match event {
                    crate::rich_text::TextEvent::Context { position, .. } => {
                        let selected = this.text_ui_selection_key().is_some_and(|k| k == key);
                        this.open_label_menu(position, selected, label, full);
                        cx.notify();
                    }
                    event => this.on_text_event(key, &handler_layout, event, window, cx),
                });
            },
        )
        .into_any_element()
    }

    /// The Settings layer, `width` wide and at most `max_h` tall.
    /// `Settings::Main`'s content, `width` wide.
    fn settings_content(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
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
            .child(self.scale_slider(width, false, window))
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
        content
    }

    pub(super) fn render_settings(
        &self,
        width: f32,
        max_h: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        self.layers.settings_width.set(width);
        let content = self.settings_content(width, window, cx);
        let (box_left, box_top, ..) = {
            let size = window.viewport_size();
            Self::settings_geometry(
                f32::from(size.width),
                f32::from(size.height) - lg(TITLE_HEIGHT),
            )
        };
        let buttons = div()
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
            ));
        let _ = p;
        self.layers.settings_left.set(box_left);
        self.info_layer_frame(width, max_h, buttons, content, cx)
    }

    /// The scrolled content of Settings with its `ScrollArea` bar: shown when the
    /// mouse comes in or the content scrolls, gone a second later, wider and
    /// darker under the mouse; the thumb can be dragged (as in the chat list).
    /// `fill`: the section fills its height; the layer takes the content's.
    fn settings_scroll_area(
        &self,
        content: gpui::Div,
        fill: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        use super::Area;
        let scroll = div()
            .id("settings-scroll")
            .flex_shrink()
            .min_h(gpui::px(0.))
            .when(fill, |d| d.flex_1())
            .overflow_y_scroll()
            .track_scroll(&self.layers.settings_scroll)
            .child(content);
        div()
            .id("settings-scroll-area")
            .relative()
            .flex_shrink()
            .min_h(gpui::px(0.))
            .when(fill, |d| d.flex_1())
            .flex()
            .flex_col()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.area_hovered(Area::Settings, *hovered, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.scrolled(Area::Settings, cx);
            }))
            .child(scroll)
            .children(self.render_scrollbar(Area::Settings, cx))
    }

    /// Settings as a section (`Info::WrapWidget` in one column): the top bar with the
    /// back arrow, "Settings", search and "⋮"; the content under it, `width` wide.
    pub(super) fn render_settings_section(
        &self,
        left: f32,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        self.layers.settings_left.set(left);
        self.layers.settings_width.set(width);
        let content = self.settings_content(width, window, cx);
        let title_font = scale::font(SECTION_TITLE_FONT);
        let back_y = ((SECTION_TOP_BAR - icons::style_size(Icon::Back).1) / 2.).floor();
        let top_bar = div()
            .flex_none()
            .relative()
            .h(px(SECTION_TOP_BAR))
            .bg(c(p.window_bg))
            .child(self.icon_button(
                "settings-section-back",
                Icon::Back,
                (SECTION_BACK_WIDTH, SECTION_TOP_BAR),
                Some((SECTION_BACK_ICON_LEFT, back_y)),
                ((6., 6.), 42.),
                (p.box_title_close_fg, p.box_title_close_fg_over),
                window,
                cx,
                |this, _, _| this.layers.hide_all(),
            ))
            .child(
                div()
                    .absolute()
                    .left(px(SECTION_BACK_WIDTH))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(title_font.size)
                    .line_height(title_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.window_bold_fg))
                    .child("Settings"),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .flex()
                    .child(self.icon_button(
                        "settings-section-search",
                        Icon::TopBarSearch,
                        (56., SECTION_TOP_BAR),
                        None,
                        ((6., 6.), 42.),
                        (p.box_title_close_fg, p.box_title_close_fg_over),
                        window,
                        cx,
                        |_, _, _| {},
                    ))
                    .child(self.icon_button(
                        "settings-section-menu",
                        Icon::MenuDots,
                        (48., SECTION_TOP_BAR),
                        None,
                        ((1., 6.), 42.),
                        (p.box_title_close_fg, p.box_title_close_fg_over),
                        window,
                        cx,
                        move |this, _, cx| {
                            // Under the button, its top right at the bar's right.
                            let at = gpui::point(
                                gpui::px(left + width),
                                gpui::px(lg(TITLE_HEIGHT) + lg(SECTION_TOP_BAR)),
                            );
                            this.open_settings_menu(at, cx);
                        },
                    )),
            );
        div()
            .id("settings-section")
            .occlude()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.window_bg))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(top_bar)
            .child(div().flex_none().h(gpui::px(lg(1.))).bg(c(p.shadow_fg)))
            .child(self.settings_scroll_area(content, true, cx))
            .into_any_element()
    }

    /// `Info::LayerWidget` / `Settings::LayerWidget`: the rounded box with the top
    /// bar ("Settings", the buttons on the right) over the scrolled content.
    fn info_layer_frame(
        &self,
        width: f32,
        max_h: f32,
        buttons: gpui::Div,
        content: gpui::Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let title_font = scale::font(TITLE_FONT);
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
            .child(div().absolute().top_0().right_0().child(buttons));
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
            .child(self.settings_scroll_area(content, false, cx))
            .child(div().flex_none().h(px(BOX_RADIUS)))
            .into_any_element()
    }

    /// The login's settings (`Settings::CreateIntroSettings`): language, connection,
    /// the tray and window options, the interface scale, the themes, the FAQ. All
    /// of them come later; the layer only shows them.
    pub(super) fn render_intro_settings(
        &self,
        width: f32,
        max_h: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let skip = || div().flex_none().h(px(SKIP));
        let mut content = div()
            .flex()
            .flex_col()
            .child(self.settings_divider())
            .child(skip())
            .child(self.settings_row(
                "intro-language",
                Lead::Icon(Icon::MenuTranslate),
                "Language",
                Some("English".into()),
                None,
                width,
                window,
                cx,
            ))
            // `SetupConnectionType`.
            .child(self.settings_row(
                "intro-connection",
                Lead::Icon(Icon::MenuNetwork),
                "Connection type",
                Some("Default (TCP used)".into()),
                None,
                width,
                window,
                cx,
            ))
            .child(skip())
            // `SetupSystemIntegrationContent`, `SetupWindowTitleContent` (Windows,
            // no account yet).
            .child(self.settings_divider())
            .child(skip());
        let checks = [
            ("intro-tray", "Show tray icon", true),
            ("intro-monochrome", "Use monochrome icon", false),
            ("intro-taskbar", "Show taskbar icon", true),
            ("intro-sendto", "Place Telegram in \"Send to\" menu", false),
            ("intro-frame", "Use system window frame", false),
        ];
        for (id, label, checked) in checks {
            content = content.child(self.settings_checkbox(id, label, checked, window, cx));
        }
        content = content
            .child(skip())
            // `SetupInterfaceScale` without icons and `SetupDefaultThemes`.
            .child(self.settings_divider())
            .child(skip())
            .child(self.settings_row(
                "intro-scale",
                Lead::None,
                "Default interface scale",
                None,
                Some(true),
                width,
                window,
                cx,
            ))
            .child(self.scale_slider(width, true, window))
            .child(div().flex_none().h(px(THUMB_SKIP)))
            .child(self.theme_previews(width, window))
            .child(self.accent_palette(width))
            .child(self.settings_checkbox("intro-accent", "System accent color", false, window, cx))
            .child(skip())
            .child(self.settings_divider())
            .child(skip())
            .child(self.settings_row(
                "intro-faq",
                Lead::None,
                "Telegram FAQ",
                None,
                None,
                width,
                window,
                cx,
            ));
        let buttons = div().flex().child(self.top_bar_button(
            "intro-settings-close",
            Icon::InfoClose,
            48.,
            4.,
            window,
            cx,
            |this, _, _| this.layers.hide_all(),
        ));
        self.info_layer_frame(width, max_h, buttons, content, cx)
    }

    /// A `Ui::Checkbox` with `settingsCheckbox` (rows with `settingsCheckboxPadding`):
    /// the rounded square, the check when on, the label. Not wired yet.
    fn settings_checkbox(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let [pl, pt, _, pb] = CHECKBOX_PADDING;
        let font = scale::font(13.);
        let check = div()
            .absolute()
            .left(px(pl))
            .top(px(pt))
            .size(px(CHECK_SIZE))
            .rounded(px(3.))
            .border(px(2.))
            .border_color(c(if checked {
                p.window_bg_active
            } else {
                p.checkbox_fg
            }))
            .when(checked, |d| {
                d.bg(c(p.window_bg_active)).child(
                    div()
                        .absolute()
                        .left(px(4. - 2.))
                        .top(px(7. - 2.))
                        .child(icon(
                            Icon::CheckboxCheck,
                            p.overview_check_fg_active,
                            window,
                        )),
                )
            });
        let ripple_size = CHECK_SIZE + 16.;
        let ripple = div()
            .absolute()
            .left(px(pl - 8.))
            .top(px(pt - 8.))
            .size(px(ripple_size))
            .children(self.ripple_layer(
                id,
                lg(ripple_size),
                lg(ripple_size),
                p.window_bg_over,
                true,
            ));
        div()
            .id(id)
            .relative()
            .flex_none()
            .h(px(pt + CHECK_SIZE + pb))
            .child(ripple)
            .child(check)
            .child(
                div()
                    .absolute()
                    .left(px(pl + CHECK_SIZE + CHECK_TEXT_LEFT))
                    .top(px(pt + CHECK_TEXT_TOP))
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .text_color(c(p.window_fg))
                    .child(label),
            )
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.press_button(id.into(), (0., 0.));
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.release_buttons();
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

    /// `SetupDefaultThemes`: the four built-in themes (`CloudListCheck` previews
    /// with their radio), the shown one chosen.
    fn theme_previews(&self, width: f32, window: &Window) -> gpui::Div {
        let p = self.palette();
        let chosen = if self.dark { 2 } else { 0 };
        let inner = width - 2. * lg(ROW_NO_ICON_LEFT);
        let count = SCHEMES.len() as f32;
        let min_skip = lg(THEME_MIN_SKIP);
        let desired = lg(THEME_PREVIEW.0);
        let single = if inner >= 3. * min_skip + count * desired {
            desired
        } else {
            ((inner - 3. * min_skip) / count).floor()
        };
        let gap = (inner - count * single) / 3.;
        let font = scale::font(13.);
        let mut row = div()
            .flex_none()
            .relative()
            .h(gpui::px(lg(THEME_TEXT_TOP) + font.height));
        for (i, scheme) in SCHEMES.iter().enumerate() {
            let left = (lg(ROW_NO_ICON_LEFT) + i as f32 * (single + gap)).round();
            let on = i == chosen;
            let radio_color = if on {
                scheme.radio_active
            } else {
                scheme.radio_inactive
            };
            let rgb = |hex: u32| c(tdr_theme::Color((hex << 8) | 0xff));
            let bubble = |x: f32, y: f32, color: u32| {
                div()
                    .absolute()
                    .left(gpui::px(x))
                    .top(px(y))
                    .w(px(THEME_BUBBLE.0))
                    .h(px(THEME_BUBBLE.1))
                    .rounded(px(2.))
                    .bg(rgb(color))
            };
            let radio = lg(RADIO_SIZE);
            let mut radio_el = div()
                .absolute()
                .left(gpui::px(((single - radio) / 2.).floor()))
                .top(px(THEME_PREVIEW.1 - RADIO_SIZE - THEME_RADIO_BOTTOM))
                .size(gpui::px(radio))
                .child(
                    // `RadioView`: the ring (`outerSkip` 1 px, 2 px thick).
                    div()
                        .absolute()
                        .left(px(1.))
                        .top(px(1.))
                        .size(px(RADIO_SIZE - 2.))
                        .rounded_full()
                        .border(px(2.))
                        .border_color(rgb(radio_color)),
                );
            if on {
                // The dot: 6 px in from the edge.
                radio_el = radio_el.child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top(px(6.))
                        .size(px(RADIO_SIZE - 12.))
                        .rounded_full()
                        .bg(rgb(radio_color)),
                );
            }
            let preview = div()
                .absolute()
                .left(gpui::px(left))
                .top_0()
                .w(gpui::px(single))
                .h(px(THEME_PREVIEW.1))
                .rounded(px(6.))
                .overflow_hidden()
                .bg(rgb(scheme.background))
                .child(bubble(
                    lg(THEME_BUBBLE_POS.0),
                    THEME_BUBBLE_POS.1,
                    scheme.received,
                ))
                .child(bubble(
                    single - lg(THEME_BUBBLE.0) - lg(THEME_BUBBLE_POS.0),
                    THEME_BUBBLE_POS.1 + THEME_BUBBLE.1 + THEME_BUBBLE_SKIP,
                    scheme.sent,
                ))
                .child(radio_el);
            row = row.child(preview).child(
                div()
                    .absolute()
                    .left(gpui::px(left))
                    .top(px(THEME_TEXT_TOP))
                    .w(gpui::px(single))
                    .flex()
                    .justify_center()
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .text_color(c(if on {
                        p.window_active_text_fg
                    } else {
                        p.window_sub_text_fg
                    }))
                    .child(scheme.name),
            );
        }
        let _ = window;
        row
    }

    /// `ColorsPalette` of the chosen theme: its accent and eight more, the shown one
    /// ringed, and the custom color button (seven dots).
    fn accent_palette(&self, width: f32) -> gpui::Div {
        let p = self.palette();
        let scheme = &SCHEMES[if self.dark { 2 } else { 0 }];
        let mut colors = vec![scheme.accent];
        colors.extend_from_slice(scheme.accents);
        let rgb = |hex: u32| c(tdr_theme::Color((hex << 8) | 0xff));
        let size = lg(ACCENT_SIZE);
        let count = colors.len() as f32 + 1.;
        let inner = width - 2. * lg(ROW_NO_ICON_LEFT);
        let skip = (inner - size * count) / (count - 1.);
        let top = lg(2. * SKIP);
        let mut row = div()
            .flex_none()
            .relative()
            .h(gpui::px(top + size + lg(SKIP)));
        for (i, color) in colors.iter().enumerate() {
            let x = (lg(ROW_NO_ICON_LEFT) + i as f32 * (size + skip)).round();
            let mut circle = div()
                .absolute()
                .left(gpui::px(x))
                .top(gpui::px(top))
                .size(gpui::px(size))
                .rounded_full()
                .bg(rgb(*color));
            if i == 0 {
                // `PaintRoundColorButton`, selected: a ring of the box color.
                let line = ACCENT_LINE;
                let inset = ACCENT_SKIP - line / 2.;
                circle = circle.child(
                    div()
                        .absolute()
                        .left(px(inset))
                        .top(px(inset))
                        .size(px(ACCENT_SIZE - 2. * inset))
                        .rounded_full()
                        .border(px(line))
                        .border_color(c(p.box_bg)),
                );
            }
            row = row.child(circle);
        }
        // `PaintCustomButton`: one dot in the middle, six around.
        let x = (lg(ROW_NO_ICON_LEFT) + (count - 1.) * (size + skip)).round();
        let dot = ACCENT_SIZE / 8.;
        let mut custom = div()
            .absolute()
            .left(gpui::px(x))
            .top(gpui::px(top))
            .size(gpui::px(size));
        for i in 0..7 {
            let (dx, dy) = if i == 0 {
                (0., 0.)
            } else {
                let angle = (i - 1) as f32 * std::f32::consts::PI / 3.;
                let r = 1. - 2. * dot / ACCENT_SIZE;
                (angle.cos() * r, angle.sin() * r)
            };
            let cx_ = ACCENT_SIZE * (1. + dx) / 2.;
            let cy_ = ACCENT_SIZE * (1. + dy) / 2.;
            custom = custom.child(
                div()
                    .absolute()
                    .left(gpui::px(lg(cx_ - dot)))
                    .top(gpui::px(lg(cy_ - dot)))
                    .size(gpui::px(lg(2. * dot)))
                    .rounded_full()
                    .bg(rgb(colors[i.min(colors.len() - 1)])),
            );
        }
        row.child(custom)
    }
}

/// Which label of the cover a menu is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverLabel {
    Name,
    Phone { hidden: bool },
}

/// Keys of the cover's selectable labels (message ids are positive).
const LABEL_NAME: i32 = -1;
const LABEL_PHONE: i32 = -2;

/// `infoTopBar` of a section: its height, the back arrow (`infoTopBarBack`: 60 px,
/// the icon at 11), the 14 px semibold title.
const SECTION_TOP_BAR: f32 = 54.;
const SECTION_BACK_WIDTH: f32 = 60.;
const SECTION_BACK_ICON_LEFT: f32 = 11.;
const SECTION_TITLE_FONT: f32 = 14.;

/// `settingsCheckbox` (`defaultBoxCheckbox`): the square, the label after it,
/// `settingsCheckboxPadding`.
const CHECKBOX_PADDING: [f32; 4] = [22., 10., 10., 10.];
const CHECK_SIZE: f32 = 22.;
const CHECK_TEXT_LEFT: f32 = 15.;
const CHECK_TEXT_TOP: f32 = 1.;
/// `settingsButtonNoIcon`: padding (22, 10, 22, 8).
const ROW_NO_ICON_LEFT: f32 = 22.;
const ROW_NO_ICON_HEIGHT: f32 = 38.;
/// `settingsBigScalePadding`, `settingsThumbSkip`.
const BIG_SCALE_PADDING: [f32; 4] = [21., 7., 21., 4.];
const THUMB_SKIP: f32 = 16.;
/// `settingsTheme*`.
const THEME_PREVIEW: (f32, f32) = (80., 92.);
const THEME_BUBBLE: (f32, f32) = (40., 14.);
const THEME_BUBBLE_POS: (f32, f32) = (6., 8.);
const THEME_BUBBLE_SKIP: f32 = 6.;
const THEME_RADIO_BOTTOM: f32 = 12.;
const THEME_MIN_SKIP: f32 = 4.;
const THEME_TEXT_TOP: f32 = 99.;
const RADIO_SIZE: f32 = 22.;
/// `settingsAccentColor*`.
const ACCENT_SIZE: f32 = 24.;
const ACCENT_SKIP: f32 = 4.;
const ACCENT_LINE: f32 = 3.;

/// A built-in theme (`EmbeddedScheme`): preview colors, its accent and the accents
/// offered for it (`DefaultAccentColors`).
struct Scheme {
    name: &'static str,
    background: u32,
    sent: u32,
    received: u32,
    radio_inactive: u32,
    radio_active: u32,
    accent: u32,
    accents: &'static [u32],
}

const SCHEMES: [Scheme; 4] = [
    Scheme {
        name: "Classic",
        background: 0x9bd494,
        sent: 0xeaffdc,
        received: 0xffffff,
        radio_inactive: 0xeaffdc,
        radio_active: 0xffffff,
        accent: 0x40a7e3,
        accents: &[
            0x45bce7, 0x52b440, 0xd46c99, 0xdf8a49, 0x9978c8, 0xc55245, 0x687b98, 0xdea922,
        ],
    },
    Scheme {
        name: "Day",
        background: 0x7ec4ea,
        sent: 0xd7f0ff,
        received: 0xffffff,
        radio_inactive: 0xd7f0ff,
        radio_active: 0xffffff,
        accent: 0x40a7e3,
        accents: &[
            0x45bce7, 0x52b440, 0xd46c99, 0xdf8a49, 0x9978c8, 0xc55245, 0x687b98, 0xdea922,
        ],
    },
    Scheme {
        name: "Tinted",
        background: 0x485761,
        sent: 0x5ca7d4,
        received: 0x6b808d,
        radio_inactive: 0x6b808d,
        radio_active: 0x5ca7d4,
        accent: 0x5288c1,
        accents: &[
            0x58bfe8, 0x466f42, 0xaa6084, 0xa46d3c, 0x917bbd, 0xab5149, 0x697b97, 0x9b834b,
        ],
    },
    Scheme {
        name: "Night",
        background: 0x485761,
        sent: 0x6b808d,
        received: 0x6b808d,
        radio_inactive: 0x6b808d,
        radio_active: 0x75bfb5,
        accent: 0x3fc1b0,
        accents: &[
            0x60a8e7, 0x4e9c57, 0xca7896, 0xcc925c, 0xa58ed2, 0xd27570, 0x7b8799, 0xcbac67,
        ],
    },
];
