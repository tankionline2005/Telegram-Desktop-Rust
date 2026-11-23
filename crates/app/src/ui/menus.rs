//! Context menus of messages and the menus of phone numbers, card numbers and dates
//! (`HistoryInner::showContextMenu`, `PhoneClickHandler`, `BankCardClickHandler`,
//! `FormattedDateClickHandler`), and the "click to copy" hint of one-line code.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Focusable, IntoElement, ParentElement, Pixels,
    Point, Styled, Window, div, point,
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
    /// A selectable label's menu in Settings' cover. The name (`FlatLabel` with
    /// `setContextCopyText`): "Copy Selected Text" with a selection, else "Copy
    /// Name" (with the copy icon, unlike tdesktop: the user's choice). The phone (`popupMenuWithIcons` and its hook):
    /// the selection or "Copy Phone Number", then "Hide with Spoiler", checked
    /// while the number is hidden (`AddPhoneSpoilerMenu`).
    pub(super) fn open_label_menu(
        &mut self,
        at: Point<Pixels>,
        selected: bool,
        label: super::settings::CoverLabel,
        text: String,
    ) {
        use super::settings::CoverLabel;
        let entries = match label {
            CoverLabel::Name => vec![if selected {
                Entry::action(
                    "Copy Selected Text",
                    Icon::MenuCopy,
                    MenuAction::CopySelected,
                )
            } else {
                Entry::action("Copy Name", Icon::MenuCopy, MenuAction::Copy(text, None))
            }],
            CoverLabel::Phone { hidden } => {
                // `CopyPhoneToClipboard`: the digits without spaces and dashes.
                let digits: String = text.chars().filter(|c| *c != ' ' && *c != '-').collect();
                vec![
                    if selected {
                        Entry::action(
                            "Copy Selected Text",
                            Icon::MenuCopy,
                            MenuAction::CopySelected,
                        )
                    } else {
                        Entry::action(
                            "Copy Phone Number",
                            Icon::MenuCopy,
                            MenuAction::Copy(digits, None),
                        )
                    },
                    Entry::checked(
                        "Hide with Spoiler",
                        Icon::MenuSpoiler,
                        MenuAction::TogglePhoneSpoiler,
                        hidden,
                    ),
                ]
            }
        };
        self.open_menu(entries, at, MenuTag::Other);
    }

    /// The chat's "⋮" menu (`Window::FillDialogsEntryMenu` with `Section::History`,
    /// `Filler::fillHistoryActions`), its top right at `at`. Muting, unmuting and
    /// leaving work; the other items come later.
    pub(super) fn open_chat_menu(&mut self, at: Point<Pixels>, peer: PeerKey) {
        use tdr_data::describe::PeerKind as K;
        let Some(chat) = self.chat_item(peer).cloned() else {
            return;
        };
        let r = chat.rights;
        let user = matches!(chat.kind, K::User | K::Bot);
        let channel = chat.kind == K::Channel;
        let supergroup = peer.kind == tdr_data::tdr_storage::PeerKind::Channel && !channel;
        let basic_group = peer.kind == tdr_data::tdr_storage::PeerKind::Chat;
        let service = peer == PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID);
        let stub = |label: &str, icon: Icon| Entry::action(label, icon, MenuAction::ChatStub);
        let mut entries = Vec::new();
        // `addToggleMuteSubmenu`: not in Saved Messages.
        if !chat.is_saved {
            if chat.muted {
                let left = chat
                    .mute_until
                    .map_or(0, |until| i64::from(until) - chrono::Utc::now().timestamp());
                entries.push(
                    Entry::action(
                        "Unmute",
                        Icon::MenuUnmute,
                        MenuAction::ChatMute(peer, false),
                    )
                    .with_shortcut(&crate::present::mute_for_tiny(left)),
                );
            } else {
                // `MuteMenu::FillMuteMenu`: the recent periods are not kept yet.
                entries.push(Entry::action_submenu(
                    "Mute notifications",
                    Icon::MenuMute,
                    vec![
                        stub("Select tone", Icon::MenuSoundSelect),
                        stub("Disable sound", Icon::MenuSoundOff),
                        stub("Mute for...", Icon::MenuMuteFor),
                        Entry::attention(
                            "Mute forever",
                            Icon::MenuMute,
                            MenuAction::ChatMute(peer, true),
                        ),
                    ],
                ));
            }
            entries.push(Entry::WideSeparator);
        }
        // `addInfo`.
        if !chat.is_saved {
            entries.push(if user {
                stub("View profile", Icon::MenuProfile)
            } else if channel {
                stub("View channel info", Icon::MenuInfo)
            } else {
                stub("View group info", Icon::MenuInfo)
            });
        }
        // `addManageChat`: admins.
        if !user && r.admin && !r.left {
            entries.push(stub(
                if channel {
                    "Manage Channel"
                } else {
                    "Manage group"
                },
                Icon::MenuManage,
            ));
        }
        // `addBoostChat`.
        if peer.kind == tdr_data::tdr_storage::PeerKind::Channel {
            entries.push(stub(
                if channel {
                    "Boost Channel"
                } else {
                    "Boost Group"
                },
                Icon::MenuBoosts,
            ));
        }
        // `addCreatePoll`: groups we are in, channels we post to, bots.
        let polls = match chat.kind {
            K::Bot => true,
            K::Group => !r.left,
            K::Channel => r.can_post && !r.left,
            K::User => false,
        };
        if polls {
            entries.push(stub("Create poll", Icon::MenuCreatePoll));
        }
        // `addThemeEdit`, `addToggleNoForwards`: private chats.
        if user && !service {
            entries.push(stub("Set Wallpaper", Icon::MenuColors));
            if chat.kind == K::User && !chat.is_saved {
                entries.push(stub("Disable Sharing", Icon::MenuShareOff));
            }
        }
        // `addViewDiscussion`.
        if channel && r.discussion {
            entries.push(stub("View discussion", Icon::MenuDiscussion));
        }
        // `addExportChat` (`canExportChatHistory`): forwarding allowed and some
        // message loaded.
        let has_messages =
            self.selected == Some(peer) && self.messages.iter().any(|m| m.service.is_none());
        if !r.noforwards && has_messages {
            entries.push(stub("Export chat history", Icon::MenuExport));
        }
        // `addReport`: groups and channels we did not make.
        if !user && !r.creator {
            entries.push(stub("Report", Icon::MenuReport));
        }
        // `addClearHistory`.
        let clear = if peer.kind == tdr_data::tdr_storage::PeerKind::Channel {
            !r.left && (r.delete_messages || (supergroup && !r.public))
        } else {
            true
        };
        if clear {
            entries.push(stub("Clear history", Icon::MenuClear));
        }
        // `addDeleteChat`: private chats and basic groups.
        if user || basic_group {
            entries.push(if basic_group {
                Entry::attention(
                    "Delete and leave",
                    Icon::MenuDelete,
                    MenuAction::ChatLeave(peer),
                )
            } else {
                // Deleting a conversation comes later.
                Entry::attention("Delete chat", Icon::MenuDelete, MenuAction::ChatStub)
            });
        }
        // `addLeaveChat`: supergroups and channels we are in.
        if peer.kind == tdr_data::tdr_storage::PeerKind::Channel && !r.left {
            entries.push(Entry::attention(
                if channel {
                    "Leave channel"
                } else {
                    "Leave group"
                },
                Icon::MenuLeave,
                MenuAction::ChatLeave(peer),
            ));
        }
        self.open_popup(PopupMenu::new(entries, true, at).top_right_origin());
    }

    /// Settings' "⋮" menu (`Settings::Main::fillTopBarMenu`), its top right at `at`.
    pub(super) fn open_settings_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let accounts = self.accounts.read(cx);
        let mut entries = Vec::new();
        if accounts.slots.len() < accounts.max_accounts() {
            entries.push(Entry::action(
                "Add Account",
                Icon::MenuAddAccount,
                MenuAction::AddAccount,
            ));
        }
        entries.push(Entry::action(
            "Edit profile",
            Icon::MenuEdit,
            MenuAction::EditProfile,
        ));
        entries.push(Entry::attention(
            "Log out",
            Icon::MenuLeave,
            MenuAction::LogOut,
        ));
        self.text_ui.menu = Some(OpenMenu {
            menu: PopupMenu::new(entries, true, at).top_right_origin(),
            tag: MenuTag::Other,
            phone_author: None,
        });
    }

    /// Shows a built menu (no answers from the data thread fill it in).
    pub(super) fn open_popup(&mut self, menu: PopupMenu) {
        self.text_ui.menu = Some(OpenMenu {
            menu,
            tag: MenuTag::Other,
            phone_author: None,
        });
    }

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

    /// The message field's menu: Qt's standard menu of a text edit with lib_ui's
    /// "Formatting" submenu (`InputField::contextMenuEventInner`,
    /// `addMarkdownActions`). No "Spelling" yet: there is no spell checker.
    pub(super) fn open_field_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        use crate::text_input::Tag;
        let can_paste = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|t| !t.is_empty());
        let input = self.input.read(cx);
        let selected = input.has_selection();
        let tagged = input.selection_has_tags();
        let format = |label: &str, shortcut: &str, tag: Tag| {
            Entry::item(
                label,
                Some(shortcut),
                selected,
                MenuAction::FieldFormat(tag),
            )
        };
        let formatting = vec![
            format("Bold", "Ctrl+B", Tag::Bold),
            format("Italic", "Ctrl+I", Tag::Italic),
            format("Underline", "Ctrl+U", Tag::Underline),
            format("Strikethrough", "Ctrl+Shift+X", Tag::Strike),
            format("Quote", "Ctrl+Shift+.", Tag::Blockquote),
            format("Monospace", "Ctrl+Shift+M", Tag::Code),
            format("Spoiler", "Ctrl+Shift+P", Tag::Spoiler),
            Entry::Separator,
            Entry::item("Create link", Some("Ctrl+K"), true, MenuAction::FieldLink),
            Entry::item(
                "Date",
                Some("Ctrl+Shift+D"),
                selected,
                MenuAction::FieldDate,
            ),
            Entry::Separator,
            Entry::item(
                "Clear formatting",
                Some("Ctrl+Shift+N"),
                selected && tagged,
                MenuAction::FieldClearFormat,
            ),
        ];
        let entries = vec![
            Entry::item(
                "Undo",
                Some("Ctrl+Z"),
                input.can_undo(),
                MenuAction::FieldUndo,
            ),
            Entry::item(
                "Redo",
                Some("Ctrl+Y"),
                input.can_redo(),
                MenuAction::FieldRedo,
            ),
            Entry::Separator,
            Entry::item("Cut", Some("Ctrl+X"), selected, MenuAction::FieldCut),
            Entry::item("Copy", Some("Ctrl+C"), selected, MenuAction::FieldCopy),
            Entry::item("Paste", Some("Ctrl+V"), can_paste, MenuAction::FieldPaste),
            Entry::item("Delete", None, selected, MenuAction::FieldDelete),
            Entry::Separator,
            Entry::submenu("Formatting", true, formatting),
            Entry::Separator,
            Entry::item(
                "Select All",
                Some("Ctrl+A"),
                !input.is_empty(),
                MenuAction::FieldSelectAll,
            ),
        ];
        self.text_ui.menu = Some(OpenMenu {
            menu: PopupMenu::new(entries, false, at),
            tag: MenuTag::Other,
            phone_author: None,
        });
        cx.notify();
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
        // `canReply`: where we can write (`CanSendAnything`).
        let can_reply = item.service.is_none()
            && self
                .chat_item(key.0)
                .is_some_and(|c| c.compose == crate::bridge::Compose::Field);
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
            e.push(Entry::action(
                "Reply",
                Icon::MenuReply,
                MenuAction::Reply(key.0, key.1),
            ));
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
        // `AddEmojiPacksAction`: the sets of the message's custom emoji.
        let mut sets: Vec<(i64, i64)> = Vec::new();
        for entity in &item.entities {
            if let crate::rich_text::Kind::CustomEmoji(id) = entity.kind
                && let Some(set) = crate::custom_emoji::info(id).and_then(|i| i.set)
                && !sets.contains(&set)
            {
                sets.push(set);
            }
        }
        if let Some(&(id, access_hash)) = sets.first() {
            let text = if sets.len() == 1 {
                match self.emoji_panel.set_title(id) {
                    Some(title) => format!("This message contains emoji from {title} pack."),
                    None => "This message contains emoji from 1 pack.".to_string(),
                }
            } else {
                format!("This message contains emoji from {} packs.", sets.len())
            };
            e.push(Entry::WideSeparator);
            e.push(Entry::Title {
                text: text.into(),
                action: MenuAction::OpenEmojiSet(id, access_hash),
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
        if let Some(action) = open.menu.handle(event, Self::menu_area(window), window) {
            self.menu_action(action, window, cx);
        }
        cx.notify();
    }

    fn menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        let field = |window: &mut Window, cx: &mut Context<Self>| {
            window.focus(&input.focus_handle(cx));
            input.clone()
        };
        match action {
            MenuAction::None => {}
            MenuAction::FieldUndo => field(window, cx).update(cx, |i, cx| i.undo(cx)),
            MenuAction::FieldRedo => field(window, cx).update(cx, |i, cx| i.redo(cx)),
            MenuAction::FieldCut => field(window, cx).update(cx, |i, cx| i.cut_selection(cx)),
            MenuAction::FieldCopy => field(window, cx).update(cx, |i, cx| i.copy_selection(cx)),
            MenuAction::FieldPaste => field(window, cx).update(cx, |i, cx| i.paste_text(cx)),
            MenuAction::FieldDelete => field(window, cx).update(cx, |i, cx| i.delete_selection(cx)),
            MenuAction::FieldSelectAll => {
                field(window, cx).update(cx, |i, cx| i.select_all_text(cx))
            }
            MenuAction::FieldFormat(tag) => field(window, cx).update(cx, |i, cx| i.toggle(tag, cx)),
            MenuAction::FieldClearFormat => {
                field(window, cx).update(cx, |i, cx| i.clear_format(cx))
            }
            // Links and dates in the field come later (their boxes).
            MenuAction::FieldLink | MenuAction::FieldDate => {
                field(window, cx);
            }
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
            MenuAction::EmojiRemoveRecent(emoji) => self.emoji_remove_recent(emoji),
            MenuAction::EmojiRemoveRecentCustom(id) => self.emoji_remove_recent_custom(id),
            MenuAction::OpenEmojiSet(id, access_hash) => {
                self.open_emoji_set_box(tdr_data::emoji_sets::SetRef::Id(id, access_hash));
            }
            MenuAction::EmojiResetRecent => {
                self.layers
                    .show_layer(super::layers::LayerBox::ResetRecentEmoji);
            }
            MenuAction::AddAccount => self.add_account_clicked(window, cx),
            MenuAction::ArchiveInMenu(in_menu) => self.set_archive_in_menu(in_menu),
            MenuAction::ChatMute(peer, mute) => {
                self.set_chat_muted(peer, mute);
                let _ = self.commands.send(Command::SetMuted { peer, mute });
            }
            MenuAction::ChatLeave(peer) => {
                self.layers.show_layer(super::layers::LayerBox::Leave(peer));
            }
            MenuAction::TogglePhoneSpoiler => {
                let hidden = self
                    .accounts
                    .read(cx)
                    .active()
                    .and_then(|s| s.me.as_ref())
                    .is_some_and(|m| m.phone_hidden);
                let _ = self.commands.send(Command::SetPhoneHidden(!hidden));
                // Turned on: the number is covered at once, even if it was shown.
                self.hide_phone_spoiler();
            }
            MenuAction::LogOut => {
                let id = self.accounts.read(cx).registry.active;
                self.layers.show_layer(super::layers::LayerBox::LogOut(id));
            }
            // Replies, editing, pins, forwarding, deleting, selecting messages,
            // contacts, calendar export, profiles and notifications come later.
            MenuAction::Reply(peer, msg_id) => self.start_reply(peer, msg_id, window, cx),
            MenuAction::QuoteReply
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
            | MenuAction::SetReminder
            | MenuAction::ArchiveStub
            | MenuAction::ChatStub
            // "Edit profile" comes with the profile section.
            | MenuAction::EditProfile => {}
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
        (bubble_left, bubble_w, chat_width): (f32, f32, f32),
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
        // Beside the bubble on the free side (left of outgoing ones), else on the
        // other side; not at all when it fits on neither.
        let pill_w = lg(8.)
            + crate::icons::icon_width(Icon::MiniCopy, window)
            + lg(4.)
            + super::text_width(window, "Click to copy", 13., gpui::FontWeight::NORMAL)
            + lg(10.);
        let fits_left = bubble_left - gap - pill_w >= 0.;
        let fits_right = bubble_left + bubble_w + gap + pill_w <= chat_width;
        let on_left = match (left, fits_left, fits_right) {
            (true, true, _) | (false, true, false) => true,
            (false, _, true) | (true, false, true) => false,
            _ => return None,
        };
        let side = gpui::px(bubble_w + gap);
        Some(
            if on_left {
                pill.right(side)
            } else {
                pill.left(side)
            }
            .into_any_element(),
        )
    }
}
