//! The main menu (tdesktop's `Window::MainMenu`): the account's cover with the
//! accounts toggle, the accounts list with "Add Account", the archive and the menu
//! items. Only the accounts (switching, adding) and Night Mode work; the other items
//! come with their sections (listed in ARCHITECTURE).

use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, PathBuilder, SharedString, StatefulInteractiveElement, Styled, Window, canvas,
    div, point, prelude::FluentBuilder,
};

use super::{MainView, c, empty_userpic};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, px};

// window_main_menu.style, window.style (mainMenu*).
pub(super) const MENU_WIDTH: f32 = 274.;
const COVER_HEIGHT: f32 = 134.;
const USERPIC_LEFT: f32 = 24.;
const USERPIC_TOP: f32 = 20.;
const USERPIC_SIZE: f32 = 48.;
const NAME_LEFT: f32 = 26.;
const NAME_TOP: f32 = 84.;
const STATUS_TOP: f32 = 103.;
const SKIP: f32 = 6.;
const FOOTER_MIN: f32 = 80.;
const FOOTER_LEFT: f32 = 25.;
const TELEGRAM_BOTTOM: f32 = 38.;
const VERSION_BOTTOM: f32 = 17.;
const TOGGLE_SIZE: f32 = 6.;
const TOGGLE_POSITION: f32 = 30.;
/// `mainMenuButton`: padding (61, 11, 20, 9) around a 20 px line, icon at 21.
const ROW_HEIGHT: f32 = 40.;
const ROW_TEXT_LEFT: f32 = 61.;
const ROW_ICON_LEFT: f32 = 21.;
const ROW_RIGHT: f32 = 20.;
const TOGGLE_SKIP: f32 = 19.;
/// `mainMenuAddAccountButton.iconLeft`, `mainMenuAccountSize`, `mainMenuAccountLine`.
const ACCOUNT_ICON_LEFT: f32 = 23.;
const ACCOUNT_SIZE: f32 = 26.;
const ACCOUNT_LINE: f32 = 2.;
/// `settingsIconAdd` is 20 px, in a round `windowBgActive` circle.
const ADD_ICON: f32 = 20.;
/// `mainMenuBadgeSize`, `mainMenuBadgeFont`, `dialogsUnreadPadding`.
const BADGE_SIZE: f32 = 18.;
const BADGE_FONT: f32 = 11.;
const BADGE_PADDING: f32 = 5.;

/// `Lang::FormatCountToShort`: 12345 is "12.3K", under 10 000 the number itself.
pub(super) fn short_count(n: u32) -> String {
    let shorten = |divider: u32, suffix: char| {
        let rounded = n / (divider / 10);
        if !rounded.is_multiple_of(10) {
            format!("{}.{}{suffix}", rounded / 10, rounded % 10)
        } else {
            format!("{}{suffix}", rounded / 10)
        }
    };
    if n >= 1_000_000_000 {
        shorten(1_000_000_000, 'B')
    } else if n >= 1_000_000 {
        shorten(1_000_000, 'M')
    } else if n >= 10_000 {
        shorten(1_000, 'K')
    } else {
        n.to_string()
    }
}

/// An account's userpic data, taken out of `Accounts` before drawing.
struct AccountPic {
    image: Option<std::sync::Arc<gpui::RenderImage>>,
    initials: String,
    color: u8,
}

/// What a menu row does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowAction {
    None,
    NightMode,
    Account(u32),
    AddAccount,
}

impl MainView {
    /// `Ui::PaintUnreadBadge` with the main menu's style.
    fn menu_badge(&self, count: u32, muted: bool, window: &Window) -> Div {
        let p = self.palette();
        let text = short_count(count);
        let w = super::text_width(window, &text, BADGE_FONT, FontWeight::BOLD);
        let font = scale::font(BADGE_FONT);
        div()
            .flex_none()
            .h(px(BADGE_SIZE))
            .min_w(px(BADGE_SIZE))
            .w(px((w + 2. * BADGE_PADDING).max(BADGE_SIZE)))
            .rounded_full()
            .bg(c(if muted {
                p.dialogs_unread_bg_muted
            } else {
                p.dialogs_unread_bg
            }))
            .flex()
            .items_center()
            .justify_center()
            .text_size(font.size)
            .line_height(font.line_height())
            .font_weight(FontWeight::BOLD)
            .text_color(c(p.dialogs_unread_fg))
            .child(text)
    }

    /// A `mainMenuButton` row: an icon, semibold text, something on the right.
    #[allow(clippy::too_many_arguments)]
    fn menu_row(
        &self,
        id: SharedString,
        leading: AnyElement,
        text: String,
        trailing: Option<AnyElement>,
        action: RowAction,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = self.palette();
        let over = self.button_hovered.as_ref() == Some(&id);
        let ripple = self.ripple_layer(
            &id,
            scale::lg(MENU_WIDTH),
            ROW_HEIGHT,
            p.window_bg_ripple,
            false,
        );
        let hover_id = id.clone();
        let press_id = id.clone();
        div()
            .id(id)
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .bg(c(if over { p.window_bg_over } else { p.window_bg }))
            .cursor_pointer()
            .children(ripple)
            .child(leading)
            .child(
                div()
                    .absolute()
                    .left(px(ROW_TEXT_LEFT))
                    .right(px(ROW_RIGHT + 60.))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(if over {
                        p.window_bold_fg_over
                    } else {
                        p.window_bold_fg
                    }))
                    .child(one_line(text)),
            )
            .children(trailing.map(|t| {
                div()
                    .absolute()
                    .right(px(ROW_RIGHT))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(t)
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.button_hovered = Some(hover_id.clone());
                } else if this.button_hovered.as_ref() == Some(&hover_id) {
                    this.button_hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    // The press point in the row: the menu starts at the window's left.
                    this.press_button(press_id.clone(), (f32::from(e.position.x), ROW_HEIGHT / 2.));
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
                this.menu_row_clicked(action, window, cx);
            }))
    }

    fn menu_row_clicked(&mut self, action: RowAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            RowAction::None => {}
            RowAction::NightMode => self.toggle_theme(cx),
            RowAction::Account(id) => {
                let active = self.accounts.read(cx).registry.active;
                self.main_menu_open = false;
                if id != active {
                    // The draft of the chat left stays.
                    self.save_draft(cx);
                    self.accounts.update(cx, |a, cx| a.activate(id, cx));
                }
            }
            RowAction::AddAccount => {
                let accounts = self.accounts.read(cx);
                let count = accounts.slots.len();
                let max = accounts.max_accounts();
                if count >= max {
                    // `AccountsLimitBox`.
                    self.show_toast("You have reached the limit of connected accounts.", None);
                } else {
                    self.main_menu_open = false;
                    self.start_add_account(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn row_icon(&self, glyph: Icon, over: bool, window: &Window) -> AnyElement {
        let p = self.palette();
        div()
            .absolute()
            .left(px(ROW_ICON_LEFT))
            .top_0()
            .h_full()
            .flex()
            .items_center()
            .child(icon(
                glyph,
                if over {
                    p.window_bold_fg_over
                } else {
                    p.window_bold_fg
                },
                window,
            ))
            .into_any_element()
    }

    /// `ToggleView` of the Night Mode row.
    fn night_toggle(&self, over: bool) -> AnyElement {
        let p = self.palette();
        let on = self.dark;
        let fg = if on {
            p.window_bg_active
        } else if over {
            p.menu_icon_fg_over
        } else {
            p.menu_icon_fg
        };
        const BORDER: f32 = 2.;
        const SHIFT: f32 = 1.;
        const DIAMETER: f32 = 16.;
        const WIDTH: f32 = 14.;
        let knob_left = BORDER + if on { WIDTH } else { 0. };
        div()
            .relative()
            .flex_none()
            .mr(px(TOGGLE_SKIP - ROW_RIGHT))
            .w(px(DIAMETER + WIDTH + 2. * BORDER))
            .h(px(DIAMETER + 2. * BORDER))
            .child(
                div()
                    .absolute()
                    .left(px(BORDER + SHIFT))
                    .top(px(BORDER + SHIFT))
                    .w(px(DIAMETER + WIDTH - 2. * SHIFT))
                    .h(px(DIAMETER - 2. * SHIFT))
                    .rounded(px((DIAMETER - 2. * SHIFT) / 2.))
                    .bg(c(fg)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(knob_left - BORDER / 2.))
                    .top(px(BORDER / 2.))
                    .size(px(DIAMETER + BORDER))
                    .rounded_full()
                    .border_2()
                    .border_color(c(fg))
                    .bg(c(p.window_bg)),
            )
            .into_any_element()
    }

    /// An account's userpic: its photo or the colored circle with initials.
    fn account_userpic(&self, pic: &AccountPic, size: f32) -> AnyElement {
        let p = self.palette();
        match &pic.image {
            Some(image) => gpui::img(image.clone())
                .size(px(size))
                .rounded_full()
                .into_any_element(),
            None => empty_userpic(p, size, &pic.initials, pic.color, false).into_any_element(),
        }
    }

    pub(super) fn render_main_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette();
        let accounts = self.accounts.read(cx);
        let active_id = accounts.registry.active;
        let shown = accounts.registry.menu_accounts;
        let (others, others_muted) = accounts.others_unread();
        let me = accounts.active().and_then(|s| s.me.clone());
        let state = accounts.active().map(|s| s.state).unwrap_or_default();
        let ordered: Vec<(u32, String, u32, bool, AccountPic)> = accounts
            .ordered()
            .iter()
            .map(|s| {
                (
                    s.id,
                    s.me.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
                    s.state.unread,
                    s.state.unread_muted,
                    AccountPic {
                        image: s.userpic.as_ref().map(|(_, i)| i.clone()),
                        initials: s
                            .me
                            .as_ref()
                            .map(|m| m.initials.clone())
                            .unwrap_or_default(),
                        color: s.me.as_ref().map_or(0, |m| m.color),
                    },
                )
            })
            .collect();
        let active_pic = ordered
            .iter()
            .find(|o| o.0 == active_id)
            .map(|o| self.account_userpic(&o.4, USERPIC_SIZE));
        let can_add = ordered.len() < crate::accounts::PREMIUM_MAX_ACCOUNTS;
        let width = MENU_WIDTH;

        // ----- The cover -----
        // `ToggleAccountsButton`: the arrow, and while the list is hidden the other
        // accounts' unread count (unless all muted).
        let badge =
            (!shown && others > 0 && !others_muted).then(|| self.menu_badge(others, false, window));
        let badge_w = badge
            .as_ref()
            .map(|_| {
                let text = short_count(others);
                (super::text_width(window, &text, BADGE_FONT, FontWeight::BOLD)
                    + 2. * BADGE_PADDING)
                    .max(BADGE_SIZE)
            })
            .unwrap_or(0.);
        let right_skip = TOGGLE_POSITION
            + 2. * TOGGLE_SIZE
            + if badge.is_some() {
                2. * TOGGLE_SIZE + badge_w
            } else {
                0.
            };
        let arrow_color = p.window_sub_text_fg;
        let up = shown;
        let arrow = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                // `Ui::ToggleUpDownArrowPath`: a chevron, up while the list is open.
                use gpui::{PathStyle, StrokeOptions};
                let (x, y) = (
                    f32::from(bounds.right()) - scale::lg(TOGGLE_POSITION),
                    f32::from(bounds.bottom()) - scale::lg(TOGGLE_POSITION),
                );
                let s = scale::lg(TOGGLE_SIZE);
                let dy = if up { -s / 2. } else { s / 2. };
                let width = scale::lg(1.5);
                let mut path = PathBuilder::stroke(gpui::px(width)).with_style(PathStyle::Stroke(
                    StrokeOptions::default()
                        .with_line_width(width)
                        .with_line_cap(lyon::tessellation::LineCap::Round)
                        .with_line_join(lyon::tessellation::LineJoin::Round),
                ));
                path.move_to(point(gpui::px(x - s), gpui::px(y - dy)));
                path.line_to(point(gpui::px(x), gpui::px(y + dy)));
                path.line_to(point(gpui::px(x + s), gpui::px(y - dy)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, c(arrow_color));
                }
            },
        )
        .absolute()
        .left_0()
        .top(px(NAME_TOP))
        .w(px(width))
        .h(px(COVER_HEIGHT - NAME_TOP));
        let name = me.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let toggle_area = div()
            .id("menu-accounts-toggle")
            .absolute()
            .left_0()
            .top(px(NAME_TOP))
            .w(px(width))
            .h(px(COVER_HEIGHT - NAME_TOP))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.accounts.update(cx, |a, cx| a.toggle_menu_accounts(cx));
            }));
        let cover = div()
            .relative()
            .flex_none()
            .h(px(COVER_HEIGHT))
            .child(toggle_area)
            .child(arrow)
            .child(
                div()
                    .id("menu-userpic")
                    .absolute()
                    .left(px(USERPIC_LEFT))
                    .top(px(USERPIC_TOP))
                    .cursor_pointer()
                    .children(active_pic)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.accounts.update(cx, |a, cx| a.toggle_menu_accounts(cx));
                    })),
            )
            .child(
                div()
                    .absolute()
                    .left(px(NAME_LEFT))
                    .top(px(NAME_TOP))
                    .w(px(width - NAME_LEFT - right_skip))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.window_bold_fg))
                    .child(one_line(name)),
            )
            .child(
                // "Set Emoji Status": the emoji status panel comes later.
                div()
                    .absolute()
                    .left(px(NAME_LEFT))
                    .top(px(STATUS_TOP))
                    .text_color(c(p.window_active_text_fg))
                    .child("Set Emoji Status"),
            )
            .children(badge.map(|b| {
                div()
                    .absolute()
                    .left(px(width - TOGGLE_POSITION - TOGGLE_SIZE * 3. - badge_w))
                    .top(px(COVER_HEIGHT - TOGGLE_POSITION - BADGE_SIZE / 2.))
                    .child(b)
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .w_full()
                    .h(px(1.))
                    .bg(c(p.shadow_fg)),
            );

        // ----- The list -----
        let mut list = div().flex().flex_col();
        let shadow = || div().flex_none().h(px(1.)).bg(c(p.shadow_fg));
        let skip = || div().flex_none().h(px(SKIP));
        if shown {
            list = list.child(skip());
            if ordered.len() > 1 {
                for (id, name, unread, muted, pic) in &ordered {
                    let (id, unread, muted) = (*id, *unread, *muted);
                    let active = id == active_id;
                    let full = ACCOUNT_SIZE + 2. * (2. * ACCOUNT_LINE + 1.);
                    let left = ACCOUNT_ICON_LEFT + (ADD_ICON - full) / 2.;
                    let ring_color = p.window_bg_active;
                    let userpic = div()
                        .absolute()
                        .left(px(left))
                        .top(px((ROW_HEIGHT - full) / 2.))
                        .size(px(full))
                        .child(
                            div()
                                .absolute()
                                .left(px(2. * ACCOUNT_LINE + 1.))
                                .top(px(2. * ACCOUNT_LINE + 1.))
                                .child(self.account_userpic(pic, ACCOUNT_SIZE)),
                        )
                        .when(active, |d| {
                            // The ring around the shown account.
                            d.child(
                                div()
                                    .absolute()
                                    .left(px(1.))
                                    .top(px(1.))
                                    .size(px(full - 2.))
                                    .rounded_full()
                                    .border(px(ACCOUNT_LINE))
                                    .border_color(c(ring_color)),
                            )
                        })
                        .into_any_element();
                    let badge = (!active && unread > 0)
                        .then(|| self.menu_badge(unread, muted, window).into_any_element());
                    list = list.child(self.menu_row(
                        format!("menu-account-{id}").into(),
                        userpic,
                        name.clone(),
                        badge,
                        RowAction::Account(id),
                        cx,
                    ));
                }
            }
            if can_add {
                let plus = div()
                    .absolute()
                    .left(px(ACCOUNT_ICON_LEFT))
                    .top(px((ROW_HEIGHT - ADD_ICON) / 2.))
                    .size(px(ADD_ICON))
                    .rounded_full()
                    .bg(c(p.window_bg_active))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(Icon::SettingsAdd, p.window_fg_active, window))
                    .into_any_element();
                list = list.child(self.menu_row(
                    "menu-add-account".into(),
                    plus,
                    "Add Account".into(),
                    None,
                    RowAction::AddAccount,
                    cx,
                ));
            }
            list = list.child(skip()).child(shadow());
        }
        list = list.child(skip());
        let hovered = |id: &str| {
            self.button_hovered
                .as_ref()
                .is_some_and(|h| h.as_ref() == id)
        };
        // The archive (`setupArchive`), with its unread chats in a grey badge.
        if state.archive_chats > 0 {
            let badge = (state.archive_unread > 0).then(|| {
                self.menu_badge(state.archive_unread, true, window)
                    .into_any_element()
            });
            list = list
                .child(self.menu_row(
                    "menu-archive".into(),
                    self.row_icon(Icon::MenuArchive, hovered("menu-archive"), window),
                    "Archived chats".into(),
                    badge,
                    RowAction::None,
                    cx,
                ))
                .child(skip())
                .child(shadow())
                .child(skip());
        }
        list = list
            .child(self.menu_row(
                "menu-profile".into(),
                self.row_icon(Icon::MenuProfile, hovered("menu-profile"), window),
                "My Profile".into(),
                None,
                RowAction::None,
                cx,
            ))
            .child(skip())
            .child(shadow())
            .child(skip());
        let items = [
            ("menu-new-group", Icon::MenuGroups, "New Group"),
            ("menu-new-channel", Icon::MenuChannel, "New Channel"),
            ("menu-contacts", Icon::MenuContacts, "Contacts"),
            ("menu-calls", Icon::MenuPhone, "Calls"),
            ("menu-saved", Icon::MenuSaved, "Saved Messages"),
            ("menu-settings", Icon::MenuSettings, "Settings"),
        ];
        for (id, glyph, text) in items {
            list = list.child(self.menu_row(
                id.into(),
                self.row_icon(glyph, hovered(id), window),
                text.into(),
                None,
                RowAction::None,
                cx,
            ));
        }
        list = list.child(self.menu_row(
            "menu-night-mode".into(),
            self.row_icon(Icon::NightMode, hovered("menu-night-mode"), window),
            "Night Mode".into(),
            Some(self.night_toggle(hovered("menu-night-mode"))),
            RowAction::NightMode,
            cx,
        ));
        // The footer: the app and its version (`mainMenuTelegramLabel`).
        let footer = div()
            .relative()
            .flex_none()
            .min_h(px(FOOTER_MIN))
            .flex_1()
            .child(
                div()
                    .absolute()
                    .left(px(FOOTER_LEFT))
                    .bottom(px(TELEGRAM_BOTTOM))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.window_sub_text_fg))
                    .child(crate::APP_NAME),
            )
            .child(
                div()
                    .absolute()
                    .left(px(FOOTER_LEFT))
                    .bottom(px(VERSION_BOTTOM))
                    .text_color(c(p.window_sub_text_fg))
                    .child(format!(
                        "Version {} x64 \u{2013} About",
                        env!("CARGO_PKG_VERSION")
                    )),
            );
        let body = div()
            .id("main-menu-scroll")
            .flex_1()
            .min_h(gpui::px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(list)
            .child(footer);

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .child(
                div()
                    .id("main-menu")
                    .w(px(width))
                    .h_full()
                    .bg(c(p.main_menu_bg))
                    .occlude()
                    .flex()
                    .flex_col()
                    .child(cover)
                    .child(body),
            )
            .child(
                div()
                    .id("main-menu-backdrop")
                    .flex_1()
                    .h_full()
                    .bg(c(p.layer_bg))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main_menu_open = false;
                        cx.notify();
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::short_count;

    #[test]
    fn counts_like_tdesktop() {
        assert_eq!(short_count(9_999), "9999");
        assert_eq!(short_count(10_000), "10K");
        assert_eq!(short_count(303_412), "303.4K");
        assert_eq!(short_count(55_804), "55.8K");
        assert_eq!(short_count(1_250_000), "1.2M");
    }
}
