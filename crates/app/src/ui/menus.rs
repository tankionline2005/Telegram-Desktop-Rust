//! Context menus of messages and the menus of phone numbers, card numbers and dates
//! (`HistoryInner::showContextMenu`, `PhoneClickHandler`, `BankCardClickHandler`,
//! `FormattedDateClickHandler`), and the "click to copy" hint of one-line code.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, IntoElement, ParentElement, Pixels, Point, Styled,
    Window, div, point,
};
use tdr_data::tdr_storage::PeerKey;

use super::message_text::TextKey;
use super::{Fade, MainView, c};
use crate::bridge::{Author, Command};
use crate::icons::{Icon, icon};
use crate::popup_menu::{Entry, MenuAction, MenuColors, MenuEvent, PhoneRow, PopupMenu};
use crate::rich_text::{Hit, Link, Rich};
use crate::scale::{self, lg, px};
use crate::toast::ToastIcon;

/// What an open menu is about (answers from the data thread fill it in).
pub(super) enum MenuTag {
    Message,
    Phone(String),
    Card(String),
    Other,
}

pub(super) struct OpenMenu {
    pub(super) menu: PopupMenu,
    tag: MenuTag,
    /// The phone number's owner, for the userpic.
    phone_author: Option<Author>,
}

/// The highlight under one-line code and its hint beside the message. Moving to
/// another code line, even in another message, it slides there.
pub(super) struct CodeHint {
    key: TextKey,
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    moved: Instant,
    /// The text's top left in the window when it was last seen.
    origin: Point<Pixels>,
    level: Fade,
}

const CODE_HINT_SLIDE: Duration = Duration::from_millis(180);
const CODE_HINT_FADE: Duration = Duration::from_millis(150);
/// Edit is possible for two days (`edit_time_limit`), always in Saved Messages.
const EDIT_TIME_LIMIT: i64 = 172_800;

impl CodeHint {
    fn current(&self) -> Bounds<Pixels> {
        let t = (self.moved.elapsed().as_secs_f32() / CODE_HINT_SLIDE.as_secs_f32()).min(1.);
        let t = (1. - (t - 1.) * (t - 1.)).max(0.).sqrt();
        let lerp = |a: Pixels, b: Pixels| a + (b - a) * t;
        Bounds::from_corners(
            point(
                lerp(self.from.left(), self.to.left()),
                lerp(self.from.top(), self.to.top()),
            ),
            point(
                lerp(self.from.right(), self.to.right()),
                lerp(self.from.bottom(), self.to.bottom()),
            ),
        )
    }

    fn animating(&self) -> bool {
        self.moved.elapsed() < CODE_HINT_SLIDE || self.level.animating()
    }
}

impl MainView {
    fn open_menu(&mut self, entries: Vec<Entry>, at: Point<Pixels>, tag: MenuTag) {
        if entries.is_empty() {
            return;
        }
        self.text_ui.menu = Some(OpenMenu {
            menu: PopupMenu::new(entries, true, at),
            tag,
            phone_author: None,
        });
    }

    /// The menu of a message (`HistoryInner::showContextMenu`): over its text `hit`
    /// says what is under the mouse (a link, the selection).
    pub(super) fn open_message_menu(
        &mut self,
        key: TextKey,
        hit: Option<&Hit>,
        rich: Option<&Rich>,
        at: Point<Pixels>,
    ) {
        let Some(item) = self.messages.iter().find(|m| m.id == key.1) else {
            return;
        };
        if self.selected != Some(key.0) {
            return;
        }
        let chat = self.chat_item(key.0);
        let is_saved = chat.is_some_and(|d| d.is_saved);
        let kind = chat.map(|d| d.kind);
        let is_channel = kind == Some(tdr_data::describe::PeerKind::Channel);
        let is_group = kind == Some(tdr_data::describe::PeerKind::Group);
        let now = chrono::Local::now().timestamp();
        let can_reply = !is_channel || item.out;
        let can_edit = item.out
            && item.forwarded.is_none()
            && (is_saved || now - i64::from(item.date) < EDIT_TIME_LIMIT);
        // Pinning in groups and channels needs rights we do not load yet.
        let can_pin = !is_group && !is_channel;
        let can_delete = (!is_group && !is_channel) || item.out;
        let selection = self.text_selection_range(key);
        let upon_selected = match (&selection, hit) {
            (Some(s), Some(h)) => s.start <= h.index && h.index <= s.end,
            _ => false,
        };
        let link = hit
            .and_then(|h| h.link)
            .and_then(|l| rich.and_then(|r| r.link(l)).cloned());
        // `copyToClipboardContextItemText`.
        let link_copy = match &link {
            Some(Link::Url(url)) => Some(("Copy Link", url.clone())),
            Some(Link::Email(email)) => Some(("Copy Email Address", email.clone())),
            Some(Link::Mention(name)) => Some(("Copy Username", format!("@{name}"))),
            Some(Link::Hashtag(tag)) | Some(Link::Cashtag(tag)) => {
                Some(("Copy Hashtag", tag.clone()))
            }
            _ => None,
        };
        let mut e = Vec::new();
        if upon_selected {
            if can_reply {
                e.push(Entry::action(
                    "Quote & Reply",
                    Icon::MenuReply,
                    MenuAction::QuoteReply,
                ));
            }
            e.push(Entry::action(
                "Copy Selected Text",
                Icon::MenuCopy,
                MenuAction::CopySelected,
            ));
        } else if can_reply {
            e.push(Entry::action("Reply", Icon::MenuReply, MenuAction::Reply));
        }
        if can_edit {
            e.push(Entry::action("Edit", Icon::MenuEdit, MenuAction::Edit));
        }
        if can_pin {
            e.push(Entry::action("Pin", Icon::MenuPin, MenuAction::Pin));
        }
        if !upon_selected && link_copy.is_none() && !item.text.is_empty() {
            e.push(Entry::action(
                "Copy Text",
                Icon::MenuCopy,
                MenuAction::CopyText(key.0, key.1),
            ));
        }
        if let Some((label, text)) = link_copy {
            e.push(Entry::action(
                label,
                Icon::MenuCopy,
                MenuAction::Copy(text, None),
            ));
        }
        e.push(Entry::action(
            "Forward",
            Icon::MenuForward,
            MenuAction::Forward,
        ));
        if can_delete {
            e.push(Entry::action(
                "Delete",
                Icon::MenuDelete,
                MenuAction::Delete,
            ));
        }
        e.push(Entry::action(
            "Select",
            Icon::MenuSelect,
            MenuAction::Select,
        ));
        // `MaybeAddWhenEditedForwardedAction`.
        let when = item
            .forward_date
            .filter(|_| item.forwarded.is_some() || item.original.is_some())
            .map(|d| (Icon::MenuForwarded, d))
            .or_else(|| {
                item.edit_date
                    .filter(|_| item.edited)
                    .map(|d| (Icon::MenuEdited, d))
            });
        if let Some((kind, date)) = when {
            e.push(Entry::WideSeparator);
            e.push(Entry::When {
                icon: kind,
                text: crate::present::read_date(date, chrono::Local::now()).into(),
            });
        }
        self.open_menu(e, at, MenuTag::Message);
    }

    /// `PhoneClickHandler::onClick`.
    pub(super) fn open_phone_menu(&mut self, phone: String, at: Point<Pixels>) {
        let phone = phone.trim().to_string();
        let entries = vec![
            Entry::action(
                "Copy Phone Number",
                Icon::MenuCopy,
                MenuAction::Copy(phone.clone(), None),
            ),
            Entry::action("Add to contacts", Icon::MenuInvite, MenuAction::AddContact),
            Entry::WideSeparator,
            Entry::Phone(PhoneRow::Loading),
        ];
        let _ = self.commands.send(Command::ResolvePhone(phone.clone()));
        self.open_menu(entries, at, MenuTag::Phone(phone));
    }

    /// `BankCardClickHandler::onClick`.
    pub(super) fn open_card_menu(&mut self, card: String, at: Point<Pixels>) {
        let copy = MenuAction::Copy(card.clone(), Some("Card number copied to clipboard."));
        let entries = vec![
            Entry::action("Copy Card Number", Icon::MenuCopy, copy),
            Entry::WideSeparator,
            Entry::Status("Loading...".into()),
        ];
        let _ = self.commands.send(Command::BankCard(card.clone()));
        self.open_menu(entries, at, MenuTag::Card(card));
    }

    /// `FormattedDateClickHandler::onClick`.
    pub(super) fn open_date_menu(&mut self, date: i32, at: Point<Pixels>) {
        let entries = vec![
            Entry::action(
                "Copy Date",
                Icon::MenuCopy,
                MenuAction::Copy(
                    crate::present::long_date_time(date),
                    Some("Date copied to clipboard."),
                ),
            ),
            Entry::action(
                "Add to Calendar",
                Icon::MenuCalendar,
                MenuAction::AddToCalendar,
            ),
            // Forwards the message to Saved Messages, scheduled at the date.
            Entry::action(
                "Set a Reminder",
                Icon::MenuNotifications,
                MenuAction::SetReminder,
            ),
        ];
        self.open_menu(entries, at, MenuTag::Other);
    }

    pub(super) fn phone_resolved(&mut self, phone: &str, user: Option<(PeerKey, String, Author)>) {
        let Some(open) = &mut self.text_ui.menu else {
            return;
        };
        if !matches!(&open.tag, MenuTag::Phone(p) if p == phone) {
            return;
        }
        let row = match &user {
            Some((peer, name, _)) => PhoneRow::Found {
                peer: *peer,
                name: name.clone(),
            },
            None => PhoneRow::Missing,
        };
        open.phone_author = user.map(|(_, _, a)| a);
        for e in &mut open.menu.entries {
            if let Entry::Phone(r) = e {
                *r = row.clone();
            }
        }
    }

    pub(super) fn bank_card_resolved(&mut self, number: &str, data: Option<tdr_data::BankCard>) {
        let Some(open) = &mut self.text_ui.menu else {
            return;
        };
        if !matches!(&open.tag, MenuTag::Card(c) if c == number) {
            return;
        }
        open.menu.entries.retain(|e| !matches!(e, Entry::Status(_)));
        match data {
            Some(data) => {
                for (name, url) in data.links {
                    open.menu.entries.push(Entry::action(
                        &name,
                        Icon::MenuPayment,
                        MenuAction::OpenUrl(url),
                    ));
                }
                if !data.title.is_empty() {
                    open.menu.entries.push(Entry::Title {
                        text: data.title.into(),
                        action: MenuAction::Copy(
                            number.to_string(),
                            Some("Card number copied to clipboard."),
                        ),
                    });
                }
            }
            None => open.menu.entries.push(Entry::Status("Failed".into())),
        }
    }

    /// Esc: an open menu goes first.
    pub(super) fn hide_menu(&mut self) -> bool {
        match &mut self.text_ui.menu {
            Some(open) if !open.menu.hiding() => {
                open.menu.hide();
                true
            }
            _ => false,
        }
    }

    fn menu_area(window: &Window) -> Bounds<Pixels> {
        Bounds::new(point(gpui::px(0.), gpui::px(0.)), window.viewport_size())
    }

    pub(super) fn render_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.text_ui.menu.as_ref().is_some_and(|m| m.menu.gone()) {
            self.text_ui.menu = None;
        }
        let open = self.text_ui.menu.as_ref()?;
        let colors = MenuColors::from(self.palette());
        let userpic = open.phone_author.as_ref().map(|a| {
            self.userpic(
                crate::popup_menu::JOIN_AS_PHOTO,
                a.key,
                a.photo,
                &a.initials,
                a.color,
                false,
            )
        });
        let view = cx.entity().downgrade();
        Some(crate::popup_menu::render(
            &open.menu,
            &colors,
            Self::menu_area(window),
            window,
            userpic,
            move |event, window, cx| {
                let _ = view.update(cx, |this, cx| this.on_menu_event(event, window, cx));
            },
        ))
    }

    fn on_menu_event(&mut self, event: MenuEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.text_ui.menu else {
            return;
        };
        let geometry = open.menu.geometry(Self::menu_area(window), window);
        if let Some(action) = open.menu.handle(event, &geometry) {
            self.menu_action(action, window, cx);
        }
        cx.notify();
    }

    fn menu_action(&mut self, action: MenuAction, _window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::CopyText(peer, id) => {
                let text = self.message_text((peer, id));
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            MenuAction::CopySelected => {
                self.copy_selected_text(cx);
            }
            MenuAction::Copy(text, toast) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if let Some(toast) = toast {
                    self.show_toast(toast, Some(ToastIcon::Copy));
                }
            }
            MenuAction::OpenUrl(url) => cx.open_url(&url),
            // Replies, editing, pins, forwarding, deleting, selecting messages,
            // contacts, calendar export, profiles and notifications come later.
            MenuAction::Reply
            | MenuAction::QuoteReply
            | MenuAction::Edit
            | MenuAction::Pin
            | MenuAction::Forward
            | MenuAction::Delete
            | MenuAction::Select
            | MenuAction::AddContact
            | MenuAction::AddToCalendar
            | MenuAction::ViewProfile(_)
            | MenuAction::ToggleNotifications
            | MenuAction::TrayShow
            | MenuAction::TrayHide
            | MenuAction::TrayQuit
            | MenuAction::SetReminder => {}
        }
    }

    /// The mouse over a text: one-line code under it gets the highlight.
    pub(super) fn code_hover(
        &mut self,
        key: TextKey,
        code: Option<Bounds<Pixels>>,
        origin: Point<Pixels>,
    ) -> bool {
        let now = Instant::now();
        // Just copied: no hint on that line until the mouse leaves it.
        if let Some((k, r)) = self.text_ui.code_suppressed
            && k == key
        {
            if code == Some(r) {
                return false;
            }
            self.text_ui.code_suppressed = None;
        }
        match code {
            Some(rect) => match &mut self.text_ui.code_hint {
                Some(h) if h.key == key => {
                    let mut changed = h.level.to < 1.;
                    if h.to != rect {
                        h.from = h.current();
                        h.to = rect;
                        h.moved = now;
                        changed = true;
                    }
                    h.origin = origin;
                    h.level.set(1.);
                    changed
                }
                Some(h) if h.level.value() > 0. => {
                    // From a line of another message: slide over.
                    let current = h.current();
                    let from = Bounds::new(current.origin + (h.origin - origin), current.size);
                    let mut level = Fade::new(h.level.value(), CODE_HINT_FADE);
                    level.set(1.);
                    *h = CodeHint {
                        key,
                        from,
                        to: rect,
                        moved: now,
                        origin,
                        level,
                    };
                    true
                }
                _ => {
                    let mut level = Fade::new(0., CODE_HINT_FADE);
                    level.set(1.);
                    self.text_ui.code_hint = Some(CodeHint {
                        key,
                        from: rect,
                        to: rect,
                        moved: now - CODE_HINT_SLIDE,
                        origin,
                        level,
                    });
                    true
                }
            },
            None => match &mut self.text_ui.code_hint {
                Some(h) if h.key == key && h.level.to > 0. => {
                    h.level.set(0.);
                    true
                }
                _ => false,
            },
        }
    }

    /// Code was copied: its highlight and hint fade out and stay away from that line.
    pub(super) fn dismiss_code_hint(&mut self, key: TextKey) {
        if let Some(h) = &mut self.text_ui.code_hint
            && h.key == key
        {
            self.text_ui.code_suppressed = Some((key, h.to));
            h.level.set(0.);
        }
    }

    /// The highlight for this text: where and how much.
    pub(super) fn code_highlight(&self, key: TextKey) -> Option<(Bounds<Pixels>, f32)> {
        let h = self.text_ui.code_hint.as_ref().filter(|h| h.key == key)?;
        Some((h.current(), h.level.value()))
    }

    /// Drops a faded hint; true while it animates.
    pub(super) fn tick_code_hint(&mut self) -> bool {
        if self
            .text_ui
            .code_hint
            .as_ref()
            .is_some_and(|h| h.level.to == 0. && !h.level.animating())
        {
            self.text_ui.code_hint = None;
        }
        self.text_ui
            .code_hint
            .as_ref()
            .is_some_and(|h| h.animating())
    }

    /// The hint beside a message's bubble ("Click to copy" with the copy icon, in
    /// the style of service messages over the wallpaper). `text_top` is where the
    /// text starts in the bubble; `left` puts it on the bubble's left.
    pub(super) fn code_hint_pill(
        &self,
        key: TextKey,
        text_top: f32,
        bubble_w: f32,
        gap: f32,
        left: bool,
        window: &Window,
    ) -> Option<AnyElement> {
        let (rect, level) = self.code_highlight(key)?;
        if level <= 0. {
            return None;
        }
        let p = self.palette();
        let f = scale::font(13.);
        let pill_h = lg(super::SERVICE_PADDING_TOP) + f.height + lg(super::SERVICE_PADDING_BOTTOM);
        let center = text_top + f32::from(rect.top() + rect.bottom()) / 2.;
        let top = (center - pill_h / 2.).round();
        let pill = div()
            .absolute()
            .top(gpui::px(top))
            .h(gpui::px(pill_h))
            .flex()
            .items_center()
            .gap(px(4.))
            .pl(px(8.))
            .pr(px(10.))
            .rounded(gpui::px(pill_h / 2.))
            .bg(c(p.msg_service_bg))
            .text_color(c(p.msg_service_fg))
            .whitespace_nowrap()
            .opacity(level)
            .child(icon(Icon::MiniCopy, p.msg_service_fg, window))
            .child("Click to copy");
        let side = gpui::px(bubble_w + gap);
        Some(
            if left {
                pill.right(side)
            } else {
                pill.left(side)
            }
            .into_any_element(),
        )
    }
}
