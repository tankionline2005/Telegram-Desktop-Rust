//! The data side of the app: a background thread with its own async runtime that owns
//! the network client and the cache. The UI thread never waits for the network or the
//! disk; it sends [`Command`]s and receives ready-to-draw [`Update`]s.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::RenderImage;
use tdr_data::describe::{self, PeerKind as ChatKind};
use tdr_data::tdr_storage::{FileCache, FileCacheConfig};

use chrono::Local;
use tdr_api::{Client, Config, SessionData};
use tdr_data::filters::{self, ChatFilter, ChatTraits, FilterEntry, FilterIcon};
use tdr_data::tdr_storage::{DialogView, MessageRow, PeerKey, PeerKind, Storage};
use tdr_data::{Applied, Sync};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::present;
use crate::rich_text::{Kind as TextKind, TextEntity};

#[derive(Debug, Clone)]
pub enum Command {
    OpenChat(PeerKey),
    /// Open a chat where it was left (its scroll state was kept): the same part of
    /// its history as then, plus a page.
    ReopenChat(PeerKey),
    /// The chat list was scrolled near its end: show (and load) more chats.
    LoadMoreDialogs,
    /// The pinned chats were dragged into this order.
    ReorderPinned(Vec<PeerKey>),
    /// The chat view was closed (Esc).
    CloseChat,
    /// Show the chats of this folder (0: all chats).
    SelectFolder(i32),
    /// The window shows this account again (switched to it): everything it shows
    /// is sent anew, no chat open.
    Refresh,
    /// The user scrolled to the top of the open chat: load older messages.
    LoadOlder(PeerKey),
    /// Open a chat on top of the open one (the "go to original" button, a mention,
    /// a t.me link): at a message (its history around it, the message focused) or
    /// at its latest messages. Back returns.
    OpenChatAt {
        peer: PeerKey,
        msg_id: Option<i32>,
    },
    /// A `@username` or `t.me/username[/post]` link: resolve it, then the UI opens it.
    OpenUsername {
        username: String,
        post: Option<i32>,
    },
    /// A phone number's menu: who has it (`contacts.resolvePhone`).
    ResolvePhone(String),
    /// A card number's menu: its bank (`payments.getBankCardData`).
    BankCard(String),
    /// A chat shown around an older message was scrolled to its bottom: load newer.
    LoadNewer(PeerKey),
    /// Back to the chat open before the last `OpenChatAt`, with the same history.
    Back,
    Send {
        peer: PeerKey,
        text: String,
        entities: Vec<tdr_data::OutEntity>,
        reply_to: Option<tdr_data::OutReply>,
    },
    /// The user saw the open chat's messages up to `max_id`: mark them read.
    Read {
        peer: PeerKey,
        max_id: i32,
    },
    /// Custom emoji are needed (their documents and files; once per id).
    LoadCustomEmoji(Vec<i64>),
    /// The installed and featured emoji sets (the emoji panel).
    LoadEmojiSets,
    /// The emoji search's groups (`messages.getEmojiGroups`).
    LoadEmojiGroups,
    /// The server's reactions (`messages.getAvailableReactions`), once.
    LoadReactions,
    /// Our reaction to a message is chosen or taken back
    /// (`HistoryItem::toggleReaction`).
    ToggleReaction {
        peer: PeerKey,
        msg_id: i32,
        key: describe::ReactionKey,
    },
    /// Emoji keywords: which of `languages` the server has, then each one's
    /// keywords since the version kept (`versions`).
    RefreshEmojiKeywords {
        languages: Vec<String>,
        versions: Vec<(String, i32)>,
    },
    /// One set by id or link name (the set box).
    LoadEmojiSet(tdr_data::emoji_sets::SetRef),
    /// Adds or removes an emoji set.
    ToggleEmojiSet {
        set: tdr_data::emoji_sets::SetRef,
        install: bool,
    },
    /// Not in the file cache (the file thread looked): fetch and download.
    FetchCustomEmoji(Vec<i64>),
    /// A userpic is needed (once per photo).
    LoadUserpic {
        peer: PeerKey,
        photo_id: i64,
        dc_id: i32,
    },
    /// The input field of `peer` holds `text`. `cloud`: also save it to the server
    /// soon (the chat was left, or typing paused for a while).
    SaveDraft {
        peer: PeerKey,
        text: String,
        cloud: bool,
    },
    /// "Log out" (`Main::Account::logOut`): `auth.logOut`, then the account's
    /// session, cache and files go; [`Update::LoggedOut`] follows.
    LogOut,
    /// A channel's "Mute" / "Unmute" button: notifications off forever, or on.
    SetMuted {
        peer: PeerKey,
        mute: bool,
    },
    /// The archive was opened: its list comes as `Update::Dialogs` of
    /// [`ARCHIVE_LIST`] (loaded further as it scrolls), until `CloseArchive`.
    OpenArchive,
    CloseArchive,
    /// "Move to main menu" (`true`) / "Move to chat list" of the archive.
    ArchiveInMenu(bool),
    /// "Hide with Spoiler" of the own number in Settings.
    SetPhoneHidden(bool),
    /// "Leave group" / "Leave channel" / "Delete and leave" (confirmed).
    Leave(PeerKey),
    /// "Join Channel" / "Join Group" / "Apply to Join Group".
    Join(PeerKey),
    /// A post's comments (`messages.getDiscussionMessage`), shown as a thread of
    /// its discussion group over the open chat (Back returns).
    OpenComments {
        channel: PeerKey,
        post: i32,
    },
    /// A forum topic's messages, as a thread (Back returns).
    OpenThread {
        peer: PeerKey,
        root: i32,
    },
    /// A forum's topics list.
    OpenForum(PeerKey),
}

/// The list id of the archive in `Update::Dialogs` (folders have positive ids).
pub const ARCHIVE_LIST: i32 = -1;

/// The archive's row (`Data::Folder` with `kId`): the chats it shows on its
/// second line, how many more it has, and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveInfo {
    /// `SessionSettings::archiveInMainMenu`: in the main menu, not over the list.
    pub in_menu: bool,
    /// `ComposeFolderListEntryText`: the latest chats' names, unread ones marked.
    pub names: Vec<(String, bool)>,
    /// "… and N more chats".
    pub more: usize,
    /// Chats with unread messages (a grey badge: the archive is muted).
    pub unread: u32,
}

#[derive(Debug, Clone)]
pub enum Update {
    /// A custom emoji's document and file.
    CustomEmoji {
        doc: tdr_data::emoji_sets::EmojiDocument,
        bytes: Vec<u8>,
    },
    /// A custom emoji that could not be loaded.
    CustomEmojiFailed(i64),
    /// A language's emoji keywords since its kept version; `remote`: the
    /// languages the server has keywords for.
    EmojiKeywords {
        remote: Vec<String>,
        lang: String,
        version: i32,
        changes: Vec<tdr_data::emoji_keywords::KeywordChange>,
    },
    /// The emoji search's groups.
    EmojiGroups(Vec<tdr_data::emoji_sets::EmojiGroup>),
    /// The server's reactions with their animations' documents.
    AvailableReactions {
        list: Vec<tdr_data::reactions::Available>,
        top: Vec<describe::ReactionKey>,
    },
    /// The installed emoji sets (the user's order) and the featured ones.
    EmojiSets {
        installed: Vec<EmojiSet>,
        featured: Vec<EmojiSet>,
    },
    /// One set for the set box (`None`: not found).
    EmojiSetLoaded {
        set: tdr_data::emoji_sets::SetRef,
        found: Option<EmojiSet>,
    },
    /// The archive's row, `None` while the archive is empty.
    Archive(Option<ArchiveInfo>),
    /// The chat list of a folder (0: all chats); the window shows the open one's.
    Dialogs {
        folder: i32,
        list: Vec<DialogItem>,
    },
    History {
        peer: PeerKey,
        messages: Vec<MessageItem>,
        /// The first message of the chat is loaded: nothing older to ask for.
        complete: bool,
        /// Newer messages exist below these (a window around an older message).
        newer: bool,
        /// Show this message (and highlight it).
        focus: Option<i32>,
        /// A thread of the chat (a topic, a post's comments): its root.
        thread: Option<i32>,
    },
    /// How many messages a thread has (its header: "12 comments", "5 messages").
    ThreadCount {
        peer: PeerKey,
        root: i32,
        count: i32,
    },
    /// A forum's topics, for its list.
    Topics {
        peer: PeerKey,
        topics: Vec<TopicItem>,
    },
    /// The draft of `peer` was changed from another device: the input field of that
    /// chat shows `text` now (`None`: empty).
    Draft {
        peer: PeerKey,
        text: Option<String>,
    },
    /// The member count of a group or channel arrived ("906 members").
    Members {
        peer: PeerKey,
        text: String,
    },
    /// A userpic is ready to draw: decoded, and its file (the window keeps only
    /// the file of userpics not shown lately and decodes it again when needed).
    Userpic {
        peer: PeerKey,
        photo_id: i64,
        image: Arc<RenderImage>,
        bytes: Arc<[u8]>,
    },
    /// An opened chat's header data, for chats not in the loaded chat list.
    ChatInfo(Box<DialogItem>),
    /// Whether the home DC is connected (`false`: "Connecting...").
    Connected(bool),
    /// The account's own user (the main menu, the accounts list).
    Me(MeInfo),
    /// The account's unread counters (its row in other accounts' menus, the archive).
    AccountState(AccountState),
    /// The folders for the column on the left; empty: no folders (no column).
    Folders(Vec<FolderItem>),
    /// A username resolved: open this chat (at this message).
    Resolved {
        peer: PeerKey,
        msg_id: Option<i32>,
    },
    /// Show a toast with this text.
    Toast(String),
    /// Who has this phone number: `None` when it is not on Telegram.
    PhoneResolved {
        phone: String,
        user: Option<(PeerKey, String, Author)>,
    },
    /// A card's bank: `None` when it could not be found.
    BankCard {
        number: String,
        data: Option<tdr_data::BankCard>,
    },
    Fatal(String),
    /// The account logged out and its data is gone.
    LoggedOut,
}

#[derive(Debug, Clone)]
pub struct DialogItem {
    pub peer: PeerKey,
    pub name: String,
    pub initials: String,
    /// tdesktop's color index (`PeerData::colorIndex`).
    pub color: u8,
    /// Small userpic: photo id and DC.
    pub photo: Option<(i64, i32)>,
    pub kind: ChatKind,
    /// The chat with yourself: "Saved Messages" with the bookmark userpic.
    pub is_saved: bool,
    /// After the name (`Ui::PeerBadge`).
    pub badge: PeerBadge,
    pub last: LastLine,
    /// Our last message: `Some(false)` sent (✓), `Some(true)` read (✓✓).
    pub last_read: Option<bool>,
    pub time: String,
    pub unread: i32,
    /// Notifications off: the badge is grey.
    pub muted: bool,
    pub pinned: bool,
    /// The saved draft ("Draft: …" in the list).
    pub draft: Option<String>,
    /// Incoming messages up to this id are read (where the "Unread messages" bar goes).
    pub read_inbox_max_id: i32,
    /// Under the name in the chat header: "online", "906 members", "bot", ...
    pub status: String,
    /// A private chat's last-seen state: the header formats it when drawn, so "5
    /// minutes ago" stays true.
    pub lastseen: Option<describe::Lastseen>,
    /// What the bottom of the chat shows instead of the message field, if anything.
    pub compose: Compose,
    /// Muted until this time (`None`: not muted by its own setting).
    pub mute_until: Option<i32>,
    /// What the chat's "⋮" menu needs to know.
    pub rights: ChatRights,
}

/// Our place in a chat, for its menu (`Window::Filler`'s checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChatRights {
    /// A group or channel we are not in (`!amIn`).
    pub left: bool,
    pub creator: bool,
    pub admin: bool,
    /// `canDeleteMessages`.
    pub delete_messages: bool,
    /// It has a username.
    pub public: bool,
    /// `!allowsForwarding`.
    pub noforwards: bool,
    /// A channel with a discussion group (its full info): "View discussion".
    pub discussion: bool,
    /// A channel we may post to: "Create poll".
    pub can_post: bool,
    /// A group with topics: it opens its topics list.
    pub forum: bool,
}

/// The bottom of a chat (`HistoryWidget::updateControlsVisibility`): the message
/// field, "Join Channel" / "Join Group" (`isJoinChannel`: a channel or supergroup we
/// are not in) or "Mute" / "Unmute" (`isMuteUnmute`: a channel we can't post to).
/// `direct`: the "Direct Messages" button at the left (`_directMessage`, a channel
/// taking them); channels get the gift button at the right (`_giftToChannel`).
/// `gift`: the gift button at the right (`_giftToChannel`: a channel whose full
/// info says `stargifts_available`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compose {
    #[default]
    Field,
    Join {
        channel: bool,
        direct: bool,
        gift: bool,
        /// A group asking an admin's approval: "Apply to Join Group".
        request: bool,
    },
    MuteUnmute {
        direct: bool,
        gift: bool,
    },
}

impl Compose {
    pub fn of(peer: PeerKey, info: &describe::PeerInfo, full: Option<&describe::FullInfo>) -> Self {
        let channel = info.kind == ChatKind::Channel;
        let direct = info.direct_messages;
        let gift = channel && full.is_some_and(|f| f.stargifts);
        if peer.kind == PeerKind::Channel && info.left {
            Compose::Join {
                channel,
                direct,
                gift,
                request: !channel && info.join_request && !info.creator,
            }
        } else if channel && !info.can_post {
            Compose::MuteUnmute { direct, gift }
        } else {
            Compose::Field
        }
    }
}

impl DialogItem {
    pub fn is_group(&self) -> bool {
        self.kind == ChatKind::Group
    }
}

/// What `Ui::PeerBadge` draws after a name: the verified check, then the emoji
/// status (a custom emoji), or the Premium star of a user without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerBadge {
    #[default]
    None,
    Verified,
    Premium,
    /// An emoji status, after the verified check when `verified`.
    Status {
        id: i64,
        verified: bool,
    },
}

impl PeerBadge {
    pub fn of(info: &describe::PeerInfo) -> Self {
        match info.emoji_status {
            Some(id) => PeerBadge::Status {
                id,
                verified: info.verified,
            },
            None if info.verified => PeerBadge::Verified,
            None if info.premium => PeerBadge::Premium,
            None => PeerBadge::None,
        }
    }
}

/// The account's own user.
#[derive(Debug, Clone, PartialEq)]
pub struct MeInfo {
    pub peer: PeerKey,
    pub name: String,
    pub initials: String,
    pub color: u8,
    pub photo: Option<(i64, i32)>,
    pub premium: bool,
    /// "+7 900 123 45 67" (`Ui::FormatPhone`), and the username without "@".
    pub phone: Option<String>,
    pub username: Option<String>,
    /// The number is shown under a spoiler (the account's `phoneNumberHidden`).
    pub phone_hidden: bool,
}

/// Unread counters of an account (`Data::Session::unreadBadge`, the archive's
/// `chatListBadgesState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccountState {
    /// Unread messages of the main list, muted chats counted.
    pub unread: u32,
    /// All of them are in muted chats (a grey badge, no dot).
    pub unread_muted: bool,
    /// Chats in the archive, and those with unread messages.
    pub archive_chats: u32,
    pub archive_unread: u32,
}

/// The counters from the cache.
/// `Data::Session::unreadWithMentionsBadge` and `...Muted` over the main list:
/// unread messages plus chats marked unread, muted ones counted; muted only when
/// there are no unread mentions and everything is muted.
fn account_state(store: &Storage, ctx: &Context) -> AccountState {
    let mut state = AccountState::default();
    let (mut messages, mut messages_muted) = (0u32, 0u32);
    let (mut marks, mut marks_muted, mut mentions) = (0u32, 0u32, 0u32);
    for row in store.unread_counts().unwrap_or_default() {
        let unread = row.unread.max(0) as u32;
        if row.folder_id == 1 {
            state.archive_chats += 1;
            if unread > 0 || row.mark {
                state.archive_unread += 1;
            }
            continue;
        }
        let default_mute = match row.peer.kind {
            PeerKind::User => ctx.mute_users,
            PeerKind::Channel if row.broadcast => ctx.mute_broadcasts,
            _ => ctx.mute_chats,
        };
        let muted = row.mute_until.map_or(default_mute, i64::from) > ctx.now;
        messages += unread;
        mentions += row.mentions.max(0) as u32;
        if muted {
            messages_muted += unread;
        }
        // A mark counts for a chat without unread messages (`UnreadState::marks`).
        if row.mark && unread == 0 {
            marks += 1;
            if muted {
                marks_muted += 1;
            }
        }
    }
    state.unread = marks + messages;
    state.unread_muted =
        state.unread > 0 && mentions == 0 && marks_muted >= marks && messages_muted >= messages;
    state
}

/// The account's own user from the cache.
fn me_info(store: &Storage) -> Option<MeInfo> {
    let self_id = store.meta_i64("self_id").ok().flatten()?;
    let peer = PeerKey::user(self_id);
    let row = store.peer(peer).ok().flatten()?;
    let info = describe::peer_info(&row);
    let username = describe::username(&row);
    let countries = COUNTRIES.get().map(Vec::as_slice).unwrap_or_default();
    Some(MeInfo {
        peer,
        initials: initials(&row.name),
        color: info.color.unwrap_or_else(|| color_index(self_id)),
        name: row.name,
        photo: info.photo,
        premium: info.premium,
        phone: info
            .phone
            .filter(|p| !p.is_empty())
            .map(|p| tdr_data::login::format_phone(&p, countries)),
        username,
        phone_hidden: store
            .meta_i64(tdr_data::PHONE_HIDDEN_KEY)
            .ok()
            .flatten()
            .is_some_and(|v| v != 0),
    })
}

/// The countries' phone patterns, once loaded (for the own number in Settings).
static COUNTRIES: std::sync::OnceLock<Vec<tdr_data::login::Country>> = std::sync::OnceLock::new();

/// Removes what an account keeps on disk: its session, cache and files.
pub fn remove_account_files(paths: &Paths) {
    let _ = std::fs::remove_file(&paths.session);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = paths.cache.clone().into_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
    let _ = std::fs::remove_dir_all(&paths.files);
}

fn send_me(store: &Storage, tx: &mpsc::UnboundedSender<Update>) {
    if let Some(me) = me_info(store) {
        let _ = tx.send(Update::Me(me));
    }
}

/// A folder in the column on the left (`Window::FiltersMenu`).
#[derive(Debug, Clone, PartialEq)]
pub struct FolderItem {
    /// 0: "All chats".
    pub id: i32,
    pub title: String,
    pub icon: FilterIcon,
    /// Unread chats, and whether all of them are muted (a grey badge).
    pub badge: Option<(u32, bool)>,
}

/// The folders and the one the chat list shows.
struct Folders {
    entries: Vec<FilterEntry>,
    active: i32,
    /// What the column was last sent (sent again only when it changes).
    sent: Option<Vec<FolderItem>>,
    /// The counters last sent.
    state_sent: Option<AccountState>,
    /// The archive is open (its list is sent too), with this many chats shown.
    archive_open: bool,
    archive_shown: usize,
    /// The archive row last sent.
    archive_sent: Option<Option<ArchiveInfo>>,
}

impl Folders {
    fn new(entries: Vec<FilterEntry>) -> Self {
        Self {
            entries,
            active: 0,
            sent: None,
            state_sent: None,
            archive_open: false,
            archive_shown: DIALOGS,
            archive_sent: None,
        }
    }

    fn any(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, FilterEntry::Folder(_)))
    }

    fn filter(&self, id: i32) -> Option<&ChatFilter> {
        self.entries.iter().find_map(|e| match e {
            FilterEntry::Folder(f) if f.id == id => Some(f),
            _ => None,
        })
    }

    fn set(&mut self, entries: Vec<FilterEntry>) {
        self.entries = entries;
        if self.filter(self.active).is_none() {
            self.active = 0;
        }
    }
}

/// What a folder needs to know about a cached chat.
fn chat_traits(d: &DialogView, ctx: &Context) -> ChatTraits {
    let info = d.peer.as_ref().map(describe::peer_info).unwrap_or_default();
    let kind = match d.dialog.peer.kind {
        PeerKind::Chat => ChatKind::Group,
        _ => info.kind,
    };
    ChatTraits {
        peer: d.dialog.peer,
        kind,
        contact: info.contact,
        muted: muted(d, kind, ctx),
        unread: d.dialog.unread_count > 0 || d.dialog.unread_mark,
        archived: d.dialog.folder_id == 1,
    }
}

/// Notifications of the chat are off (its own setting, else the default of its kind).
fn muted(d: &DialogView, kind: ChatKind, ctx: &Context) -> bool {
    let default_mute = match kind {
        ChatKind::User | ChatKind::Bot => ctx.mute_users,
        ChatKind::Group => ctx.mute_chats,
        ChatKind::Channel => ctx.mute_broadcasts,
    };
    d.dialog.mute_until.map_or(default_mute, i64::from) > ctx.now
}

/// The time a chat is sorted by (`adjustedChatListTimeId`: a draft lifts it).
fn list_time(d: &DialogView) -> i32 {
    d.dialog
        .top_date
        .max(d.draft.as_ref().map_or(0, |(_, date)| *date))
}

/// The second line of a chat-list row: "Sender: " and a media caption or service text
/// in the accent color, then the text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastLine {
    pub prefix: Option<String>,
    pub accent: Option<String>,
    pub text: String,
    /// Byte ranges of `text` under spoilers: its spoiler entities, and the login code
    /// of a "Telegram" notification (`SpoilerLoginCode`).
    pub spoilers: Vec<std::ops::Range<usize>>,
    /// Byte ranges of `text` drawn as custom emoji.
    pub custom: Vec<(std::ops::Range<usize>, i64)>,
}

/// The custom emoji entities of a message within its first line `text`.
fn custom_in_line(
    info: &describe::MessageInfo,
    full: &str,
    text: &str,
) -> Vec<(std::ops::Range<usize>, i64)> {
    info.entities
        .iter()
        .filter_map(|e| match e.kind {
            describe::EntityKind::CustomEmoji(id) => e.byte_range(full).map(|r| (r, id)),
            _ => None,
        })
        .filter(|(r, _)| r.end <= text.len())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageItem {
    pub id: i32,
    pub out: bool,
    pub text: String,
    /// Shown instead of media that is not drawn yet ("Photo", "Voice message", ...).
    pub media: Option<String>,
    /// A service message: centered grey bubble with this text.
    pub service: Option<String>,
    pub edited: bool,
    /// Outgoing messages: read by the other side (✓✓) or only sent (✓).
    pub read: Option<bool>,
    pub time: String,
    /// Unix time, for grouping and date dividers.
    pub date: i32,
    /// Sender name and color in groups (incoming messages only).
    pub sender: Option<(String, u8)>,
    pub author: Author,
    /// "Forwarded from" line: the original sender's name and userpic.
    pub forwarded: Option<ForwardLine>,
    /// The "go to original" button's target (`saved_from_peer`, `saved_from_msg_id`).
    pub original: Option<(PeerKey, i32)>,
    /// Formatting and links of `text`, as byte ranges of it.
    pub entities: Vec<TextEntity>,
    /// When the original of a forward was sent, when it was edited (its menu
    /// shows the date).
    pub forward_date: Option<i32>,
    pub edit_date: Option<i32>,
    /// A channel post: its own bubble with the channel's name, no userpic.
    pub post: Option<Post>,
    pub reply: Option<ReplyLine>,
    /// The thread it belongs to (`reply_to_top_id`, or the replied message when
    /// that is the root): its reply to the root is not shown inside the thread.
    pub thread_root: Option<i32>,
    /// Its reactions under the text (`Reactions::InlineList`).
    pub reactions: Option<MessageReactions>,
}

/// A message's reactions as drawn (`InlineListData`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessageReactions {
    pub buttons: Vec<ReactionButton>,
    /// Saved Messages tags (`reactions_as_tags`): drawn as tags.
    pub tags: bool,
}

/// One reaction under a message: its count or who chose it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReactionButton {
    pub key: describe::ReactionKey,
    pub count: i32,
    /// Ours (`MessageReaction::my`).
    pub chosen: bool,
    /// Their userpics instead of the count (`InlineListData::recent`).
    pub userpics: Option<Vec<Author>>,
}

/// The order of the server's emoji reactions (`Reactions::list(Type::All)`),
/// for sorting reactions of equal count.
static REACTION_ORDER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// `InlineListDataFromMessage` and `InlineList::layoutButtons`: the buttons in
/// tdesktop's order, with userpics where all who reacted are known (always in
/// private chats, else at most three).
fn reaction_buttons(
    reactions: Option<&describe::Reactions>,
    private: Option<PeerKey>,
    me: PeerKey,
    author: &mut dyn FnMut(PeerKey) -> Author,
) -> Option<MessageReactions> {
    use describe::ReactionKey;
    let r = reactions?;
    let mut list: Vec<&describe::ReactionCount> = r.list.iter().collect();
    if !r.tags {
        let order = REACTION_ORDER.lock().unwrap();
        let rank = |k: &ReactionKey| match k {
            ReactionKey::Emoji(e) => order.iter().position(|o| o == e).unwrap_or(order.len()),
            _ => order.len() + 1,
        };
        list.sort_by(|a, b| {
            let without_mine =
                |c: &describe::ReactionCount| c.count - i32::from(c.chosen_order.is_some());
            (b.key == ReactionKey::Paid)
                .cmp(&(a.key == ReactionKey::Paid))
                .then(without_mine(b).cmp(&without_mine(a)))
                .then(rank(&a.key).cmp(&rank(&b.key)))
        });
    }
    let show_userpics = private.is_some() || {
        let mut sum = 0;
        list.iter().all(|c| {
            sum += c.count;
            let known = r.recent.iter().filter(|(_, k, _)| *k == c.key).count();
            sum <= 3 && known as i32 == c.count
        }) && list
            .iter()
            .all(|c| r.recent.iter().any(|(_, k, _)| *k == c.key))
    };
    let buttons = list
        .into_iter()
        .map(|c| {
            let mine = c.chosen_order.is_some();
            let userpics = (show_userpics && !r.tags).then(|| match private {
                Some(user) => {
                    let mut pics = Vec::new();
                    if !mine || c.count > 1 {
                        pics.push(author(user));
                    }
                    if mine {
                        pics.push(author(me));
                    }
                    pics
                }
                None => r
                    .recent
                    .iter()
                    .filter(|(_, k, _)| *k == c.key)
                    .map(|(p, _, _)| author(*p))
                    .collect(),
            });
            ReactionButton {
                key: c.key.clone(),
                count: c.count,
                chosen: mine,
                userpics,
            }
        })
        .collect();
    Some(MessageReactions {
        buttons,
        tags: r.tags,
    })
}

/// What a channel post shows besides its text (`Message` with `isPost`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Post {
    /// The view count before the time (`BottomInfo::_views`, short: "12.3K").
    pub views: Option<String>,
    /// The author's signature before the time ("Author, 12:30").
    pub signature: Option<String>,
    /// The comments button under the text: "3 comments", "Leave a comment".
    pub comments: Option<String>,
    /// The last commenters' userpics on it (`recent_repliers`).
    pub repliers: Vec<Author>,
    /// The discussion group its comments are in.
    pub discussion: Option<PeerKey>,
}

/// A forum topic in its list (`Dialogs::Row` of a `ForumTopic`).
#[derive(Debug, Clone, PartialEq)]
pub struct TopicItem {
    pub id: i32,
    pub title: String,
    /// `icon_color`; the "General" topic (id 1) has its "#" icon.
    pub icon_color: i32,
    pub general: bool,
    pub last: LastLine,
    pub time: String,
    pub unread: i32,
    pub muted: bool,
    pub pinned: bool,
    pub closed: bool,
}

/// The replied message over a message's text (`HistoryView::Reply`).
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyLine {
    /// Where the replied message is (a click shows it there).
    pub peer: PeerKey,
    pub msg_id: i32,
    /// Its sender's name and color (`None`: no color, the service color).
    pub name: String,
    pub color: Option<u8>,
    /// Its first line, a media caption, "Loading..." or "Deleted message".
    pub text: String,
    /// The text is a quote of it (the quote icon).
    pub quote: bool,
}

/// The "Forwarded from" line (`HistoryMessageForwarded::text`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardLine {
    pub name: String,
    /// The original sender (hidden senders have no userpic there).
    pub author: Option<Author>,
}

/// Who wrote a message: for the userpic next to it and for grouping.
#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub key: Option<PeerKey>,
    pub initials: String,
    pub color: u8,
    pub photo: Option<(i64, i32)>,
}

/// tdesktop's peer color for an id (`Data::DecideColorIndex`: `id % 7`).
pub fn color_index(id: i64) -> u8 {
    (id.rem_euclid(7)) as u8
}

fn initials(name: &str) -> String {
    let mut words = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()));
    let first = words.next();
    let second = words.next();
    match (first, second) {
        (Some(a), Some(b)) => format!("{}{}", a.to_uppercase(), b.to_uppercase()),
        (Some(a), None) => a.to_uppercase().to_string(),
        _ => String::new(),
    }
}

/// What the bridge knows about the account and the defaults.
struct Context {
    self_id: i64,
    now: i64,
    mute_users: i64,
    mute_chats: i64,
    mute_broadcasts: i64,
}

impl Context {
    fn load(store: &Storage) -> Self {
        let meta = |k: &str| store.meta_i64(k).ok().flatten().unwrap_or(0);
        Self {
            self_id: meta("self_id"),
            now: chrono::Utc::now().timestamp(),
            mute_users: meta(tdr_data::MUTE_USERS),
            mute_chats: meta(tdr_data::MUTE_CHATS),
            mute_broadcasts: meta(tdr_data::MUTE_BROADCASTS),
        }
    }
}

fn peer_name(store: &Storage, key: PeerKey) -> String {
    store
        .peer(key)
        .ok()
        .flatten()
        .map_or_else(|| "…".into(), |p| p.name)
}

/// First name for "Name: text" in the chat list (`PeerData::shortName`).
fn short_name(store: &Storage, key: PeerKey, self_id: i64) -> String {
    if key == PeerKey::user(self_id) {
        return "You".into();
    }
    match store.peer(key).ok().flatten() {
        Some(row) => describe::peer_info(&row)
            .first_name
            .filter(|n| !n.is_empty())
            .unwrap_or(row.name),
        None => "…".into(),
    }
}

/// Media caption and service text of a message.
fn message_content(
    store: &Storage,
    m: &MessageRow,
    is_channel: bool,
) -> (Option<String>, Option<String>, describe::MessageInfo) {
    let info = describe::message_info(m);
    let service = info.service.as_ref().map(|s| {
        let from_id = m.from.map(|f| f.id);
        let from = m.from.map_or_else(String::new, |f| peer_name(store, f));
        present::service_text(s, &from, from_id, m.out, is_channel, |id| {
            peer_name(store, PeerKey::user(id))
        })
    });
    let media = info.media.as_ref().map(present::media_label);
    (media, service, info)
}

/// Text of a message row: older cache rows stored placeholders for media and service
/// messages.
/// The text a message shows and its entities as byte ranges. A formatted date with
/// a format replaces its text (`BlockParser`'s replaced entities).
fn message_text(text: &str, entities: &[describe::Entity]) -> (String, Vec<TextEntity>) {
    use describe::EntityKind as E;
    let now = Local::now().timestamp() as i32;
    let mut ranged: Vec<(std::ops::Range<usize>, &E)> = entities
        .iter()
        .filter_map(|e| Some((e.byte_range(text)?, &e.kind)))
        .collect();
    ranged.sort_by_key(|(r, _)| r.start);
    // Replacements: (original range, new text), not overlapping.
    let mut replaced: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for (r, kind) in &ranged {
        if let E::FormattedDate { date, flags } = kind
            && flags.any()
            && replaced.last().is_none_or(|(last, _)| last.end <= r.start)
        {
            replaced.push((r.clone(), present::formatted_date(*date, *flags, now)));
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, shown) in &replaced {
        out.push_str(&text[at..r.start]);
        out.push_str(shown);
        at = r.end;
    }
    out.push_str(&text[at..]);
    // An offset in `text` as an offset in `out`: inside a replaced range it moves to
    // that range's start (or end).
    let map = |i: usize| -> usize {
        let mut shift: isize = 0;
        for (r, shown) in &replaced {
            if i >= r.end {
                shift += shown.len() as isize - r.len() as isize;
            } else if i > r.start {
                return (r.start as isize + shift) as usize;
            }
        }
        (i as isize + shift) as usize
    };
    let mut converted: Vec<TextEntity> = ranged
        .iter()
        .map(|(r, kind)| {
            let kind = match kind {
                E::Bold => TextKind::Bold,
                E::Italic => TextKind::Italic,
                E::Underline => TextKind::Underline,
                E::Strike => TextKind::Strike,
                E::Spoiler => TextKind::Spoiler,
                E::Code => TextKind::Code,
                E::Pre { language } => TextKind::Pre(language.clone()),
                E::Blockquote { .. } => TextKind::Blockquote,
                E::Url => TextKind::Url(text[r.clone()].to_string()),
                E::TextUrl(url) => TextKind::Url(url.clone()),
                E::Email => TextKind::Email,
                E::Mention => TextKind::Mention,
                E::MentionName(id) => TextKind::MentionName(*id),
                E::Hashtag => TextKind::Hashtag,
                E::Cashtag => TextKind::Cashtag,
                E::BotCommand => TextKind::BotCommand,
                E::Phone => TextKind::Phone,
                E::BankCard => TextKind::BankCard,
                E::CustomEmoji(id) => TextKind::CustomEmoji(*id),
                E::FormattedDate { date, .. } => TextKind::Date(*date),
            };
            TextEntity {
                range: map(r.start)..map(r.end).max(map(r.start)),
                kind,
            }
        })
        .collect();
    add_syntax(&out, &mut converted);
    (out, converted)
}

/// Code blocks with a language get their syntax colors.
fn add_syntax(out: &str, converted: &mut Vec<TextEntity>) {
    let blocks: Vec<(std::ops::Range<usize>, String)> = converted
        .iter()
        .filter_map(|e| match &e.kind {
            TextKind::Pre(language) if !language.is_empty() => {
                Some((e.range.clone(), language.clone()))
            }
            _ => None,
        })
        .collect();
    for (range, language) in blocks {
        if let Some(code) = out.get(range.clone()) {
            for (r, color) in syntax_colors(code, &language) {
                converted.push(TextEntity {
                    range: range.start + r.start..range.start + r.end,
                    kind: TextKind::Syntax(color),
                });
            }
        }
    }
}

/// tdesktop's syntax highlighting (libprisma), remembered: histories are sent
/// again on every change, the code stays the same. The highlighter (its
/// grammars and the patterns compiled so far: ~20 MB for 25 languages) is let
/// go after a minute unused and loaded again for new code (milliseconds).
fn syntax_colors(code: &str, language: &str) -> Vec<(std::ops::Range<usize>, u8)> {
    use std::sync::{LazyLock, Mutex};
    type Cache = HashMap<(String, String), Vec<(std::ops::Range<usize>, u8)>>;
    static HIGHLIGHTER: Mutex<Option<(tdr_prism::Highlighter, Instant)>> = Mutex::new(None);
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
    static RELEASER: std::sync::Once = std::sync::Once::new();
    const KEEP: Duration = Duration::from_secs(60);
    let key = (language.to_string(), code.to_string());
    if let Some(runs) = CACHE.lock().unwrap().get(&key) {
        return runs.clone();
    }
    RELEASER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("syntax-release".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(KEEP / 2);
                    let mut h = HIGHLIGHTER.lock().unwrap();
                    if h.as_ref().is_some_and(|(_, used)| used.elapsed() > KEEP) {
                        *h = None;
                    }
                }
            });
    });
    let runs = {
        let mut h = HIGHLIGHTER.lock().unwrap();
        let (highlighter, used) = h.get_or_insert_with(|| {
            (
                tdr_prism::Highlighter::load(include_bytes!(
                    "../../../assets/libprisma/grammars.dat"
                )),
                Instant::now(),
            )
        });
        *used = Instant::now();
        highlighter.highlight(code, language)
    };
    let mut cache = CACHE.lock().unwrap();
    if cache.len() > 500 {
        cache.clear();
    }
    cache.insert(key, runs.clone());
    runs
}

/// `kMinLoginCode`: codes of "Telegram" are hidden from five characters on.
const LOGIN_CODE_MIN: usize = 5;

fn row_text(m: &MessageRow) -> String {
    match m.text.as_str() {
        "[media]" | "[service message]" => String::new(),
        t => t.to_string(),
    }
}

fn dialog_item(store: &Storage, d: &DialogView, ctx: &Context) -> DialogItem {
    let peer = d.dialog.peer;
    let info = d.peer.as_ref().map(describe::peer_info).unwrap_or_default();
    let is_saved = peer == PeerKey::user(ctx.self_id);
    let name = if is_saved {
        "Saved Messages".to_string()
    } else {
        d.peer
            .as_ref()
            .map_or_else(|| "…".to_string(), |p| p.name.clone())
    };
    let kind = match peer.kind {
        PeerKind::Chat => ChatKind::Group,
        _ => info.kind,
    };
    let is_channel = kind == ChatKind::Channel;
    let last = d.top.as_ref().map(|m| {
        let (media, service, info) = message_content(store, m, is_channel);
        let full = row_text(m);
        let text = full.lines().next().unwrap_or("").to_string();
        let mut spoilers: Vec<std::ops::Range<usize>> = info
            .entities
            .iter()
            .filter(|e| e.kind == describe::EntityKind::Spoiler)
            .filter_map(|e| e.byte_range(&full))
            .filter(|r| r.start < text.len())
            .map(|r| r.start..r.end.min(text.len()))
            .collect();
        // `HistoryItem::toPreview` with `spoilerLoginCode`: a login code from
        // "Telegram" is hidden, unless an entity runs into it.
        if peer == PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID)
            && !m.out
            && let Some(code) = describe::login_code(&text, LOGIN_CODE_MIN)
            && !info
                .entities
                .iter()
                .filter_map(|e| e.byte_range(&full))
                .any(|r| r.start < code.start && r.end > code.start)
        {
            spoilers.push(code);
            spoilers.sort_by_key(|r| r.start);
        }
        let prefix = (kind == ChatKind::Group && service.is_none())
            .then(|| {
                if m.out {
                    Some("You".to_string())
                } else {
                    m.from.map(|f| short_name(store, f, ctx.self_id))
                }
            })
            .flatten();
        match (service, media) {
            (Some(s), _) => LastLine {
                prefix: None,
                accent: Some(s),
                text: String::new(),
                spoilers: Vec::new(),
                custom: Vec::new(),
            },
            (None, Some(label)) if text.is_empty() => LastLine {
                prefix,
                accent: Some(label),
                text: String::new(),
                spoilers: Vec::new(),
                custom: Vec::new(),
            },
            _ => LastLine {
                prefix,
                accent: None,
                custom: custom_in_line(&info, &full, &text),
                text,
                spoilers,
            },
        }
    });
    let last_read = d
        .top
        .as_ref()
        .filter(|m| m.out && !is_saved && !is_channel)
        .map(|m| m.id <= d.dialog.read_outbox_max_id);
    let muted = muted(d, kind, ctx);
    // The full info of a supergroup or channel, when loaded (gifts, discussion).
    let full = (peer.kind == PeerKind::Channel)
        .then(|| store.peer_full(peer).ok().flatten())
        .flatten()
        .and_then(|raw| describe::full_info(&Some(raw)));
    let now = Local::now();
    let status = if is_saved {
        String::new()
    } else if peer == PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID) {
        "service notifications".into()
    } else {
        match kind {
            ChatKind::Bot => "bot".into(),
            ChatKind::User => info
                .lastseen
                .map(|l| present::lastseen_text(l, now))
                .unwrap_or_else(|| "last seen a long time ago".into()),
            ChatKind::Group => info
                .members
                .map(|n| present::count_text(n, "member", "members"))
                .unwrap_or_default(),
            ChatKind::Channel => info
                .members
                .map(|n| present::count_text(n, "subscriber", "subscribers"))
                .unwrap_or_default(),
        }
    };
    DialogItem {
        peer,
        initials: initials(&name),
        color: info.color.unwrap_or_else(|| color_index(peer.id)),
        photo: info.photo,
        kind,
        is_saved,
        badge: if is_saved {
            PeerBadge::None
        } else {
            PeerBadge::of(&info)
        },
        name,
        last: last.unwrap_or_default(),
        last_read,
        // A newer draft shows its own time, as in tdesktop.
        time: d
            .top
            .as_ref()
            .map(|m| m.date)
            .max(d.draft.as_ref().map(|(_, date)| *date))
            .map(|date| present::dialogs_date(date, now))
            .unwrap_or_default(),
        draft: d.draft.as_ref().map(|(text, _)| text.clone()),
        unread: d.dialog.unread_count,
        muted,
        pinned: d.dialog.pinned,
        read_inbox_max_id: d.dialog.read_inbox_max_id,
        status,
        lastseen: info.lastseen.filter(|_| {
            kind == ChatKind::User
                && !is_saved
                && peer != PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID)
        }),
        compose: if is_saved {
            Compose::Field
        } else {
            Compose::of(peer, &info, full.as_ref())
        },
        mute_until: d.dialog.mute_until.filter(|_| muted),
        rights: ChatRights {
            left: peer.kind != PeerKind::User && info.left,
            creator: info.creator,
            admin: info.admin,
            delete_messages: info.delete_messages,
            public: info.public,
            noforwards: info.noforwards,
            discussion: full.as_ref().is_some_and(|f| f.linked_chat.is_some()),
            can_post: info.can_post,
            forum: info.forum,
        },
    }
}

fn author_of(store: &Storage, key: PeerKey) -> Author {
    let row = store.peer(key).ok().flatten();
    let name = row.as_ref().map_or_else(String::new, |p| p.name.clone());
    let info = row.as_ref().map(describe::peer_info).unwrap_or_default();
    Author {
        key: Some(key),
        initials: initials(&name),
        color: info.color.unwrap_or_else(|| color_index(key.id)),
        photo: info.photo,
    }
}

fn message_items(store: &Storage, peer: PeerKey, messages: &[MessageRow]) -> Vec<MessageItem> {
    let ctx = Context::load(store);
    let peer_kind = match peer.kind {
        PeerKind::Chat => ChatKind::Group,
        _ => store
            .peer(peer)
            .ok()
            .flatten()
            .map(|p| describe::peer_info(&p).kind)
            .unwrap_or_default(),
    };
    let is_group = peer_kind == ChatKind::Group;
    let is_channel = peer_kind == ChatKind::Channel;
    let is_saved = peer == PeerKey::user(ctx.self_id);
    let read_outbox = store
        .read_marks(peer)
        .ok()
        .flatten()
        .map_or(0, |(_, _, outbox)| outbox);
    let mut authors: std::collections::HashMap<PeerKey, Author> = Default::default();
    // Oldest first for display.
    messages
        .iter()
        .rev()
        .map(|m| {
            let (media, service, info) = message_content(store, m, is_channel);
            let fwd = info.forward.as_ref();
            let me = PeerKey::user(ctx.self_id);
            // `Message::hasOutLayout`: in Saved Messages everything is drawn as
            // outgoing except forwards from others; posts never are.
            let post = info.post && service.is_none();
            let out = if is_saved {
                fwd.is_none_or(|f| f.outgoing_in_saved(me))
            } else {
                m.out && !post
            };
            // `HistoryItem::displayFrom`: Saved Messages show forwards under their
            // original sender (or whom they were saved from).
            let saved_from = fwd.filter(|_| is_saved && !out).map(|f| {
                if f.forward_of_forward {
                    (f.saved_from, f.saved_from_name.clone())
                } else {
                    (f.from, f.from_name.clone())
                }
            });
            let (author, sender) = match &saved_from {
                Some((Some(key), _)) => {
                    let author = authors
                        .entry(*key)
                        .or_insert_with(|| author_of(store, *key))
                        .clone();
                    let name = peer_name(store, *key);
                    let color = author.color;
                    (author, Some((name, color)))
                }
                Some((None, name)) => {
                    let name = name.clone().unwrap_or_default();
                    let author = hidden_author(&name);
                    let color = author.color;
                    (author, Some((name, color)))
                }
                // `hasFromName` without `hasFromPhoto`: the channel's name on every
                // post (posts are never attached), its color.
                None if post => {
                    let author = authors
                        .entry(peer)
                        .or_insert_with(|| author_of(store, peer))
                        .clone();
                    let color = author.color;
                    (author, Some((peer_name(store, peer), color)))
                }
                None => {
                    let key = if m.out { me } else { m.from.unwrap_or(peer) };
                    let author = authors
                        .entry(key)
                        .or_insert_with(|| author_of(store, key))
                        .clone();
                    let sender = (is_group && !m.out)
                        .then(|| m.from.map(|from| (peer_name(store, from), author.color)))
                        .flatten();
                    (author, sender)
                }
            };
            // `displayForwardedFrom`: in Saved Messages only when it was saved from
            // someone else than its original sender.
            let forwarded = fwd
                .filter(|f| {
                    !is_saved
                        || (f.forward_of_forward
                            && (f.saved_from_name.is_some() || f.saved_from != f.from))
                })
                .map(|f| {
                    let name = f
                        .from
                        .map(|k| peer_name(store, k))
                        .or_else(|| f.from_name.clone())
                        .unwrap_or_default();
                    ForwardLine {
                        // `lng_forwarded_signed`: "Channel (Author)".
                        name: match &f.post_author {
                            Some(a) => format!("{name} ({a})"),
                            None => name,
                        },
                        author: f.from.map(|k| {
                            authors
                                .entry(k)
                                .or_insert_with(|| author_of(store, k))
                                .clone()
                        }),
                    }
                });
            // Calls are bubbles with their caption, not service lines.
            let call = info.service.as_ref().is_some_and(present::is_call);
            let (text, entities) = if call {
                (service.clone().unwrap_or_default(), Vec::new())
            } else {
                message_text(&row_text(m), &info.entities)
            };
            MessageItem {
                id: m.id,
                out,
                date: m.date,
                text,
                entities,
                forward_date: info.forward.as_ref().map(|f| f.date),
                edit_date: info.edit_date,
                post: post.then(|| Post {
                    views: info.views.map(present::count_short),
                    signature: info.post_author.clone(),
                    comments: info.comments.map(present::comments_text),
                    repliers: info
                        .recent_repliers
                        .iter()
                        .take(3)
                        .map(|&k| {
                            authors
                                .entry(k)
                                .or_insert_with(|| author_of(store, k))
                                .clone()
                        })
                        .collect(),
                    discussion: info.comments_channel.map(PeerKey::channel),
                }),
                reply: info
                    .reply
                    .as_ref()
                    .and_then(|r| reply_line(store, peer, r, &ctx)),
                thread_root: info.reply.as_ref().and_then(|r| r.top_id.or(r.msg_id)),
                reactions: reaction_buttons(
                    info.reactions.as_ref(),
                    (peer.kind == PeerKind::User && !is_saved).then_some(peer),
                    PeerKey::user(ctx.self_id),
                    &mut |k| {
                        authors
                            .entry(k)
                            .or_insert_with(|| author_of(store, k))
                            .clone()
                    },
                ),
                media,
                service: service.filter(|_| !call),
                edited: info.edited,
                // Saved Messages are never unread: their outgoing ones show ✓✓.
                read: if is_saved {
                    out.then_some(true)
                } else {
                    (m.out && !is_channel).then_some(m.id <= read_outbox)
                },
                time: present::message_time(m.date),
                sender,
                author,
                forwarded,
                original: fwd.and_then(|f| f.original),
            }
        })
        .collect()
}

/// The demo archive's row, as `archive_info` makes it.
fn demo_archive_row(archived: &[DialogItem], in_menu: bool) -> ArchiveInfo {
    let shown = ARCHIVE_NAMES.min(archived.len());
    let throw_away = shown > 1 && archived.len() == shown + 1;
    let shown = shown - usize::from(throw_away);
    ArchiveInfo {
        in_menu,
        names: archived[..shown]
            .iter()
            .map(|d| (d.name.clone(), d.unread > 0))
            .collect(),
        more: archived.len() - shown,
        unread: archived.iter().filter(|d| d.unread > 0).count() as u32,
    }
}

/// Replied messages not in the cache yet, asked for once
/// (`ApiWrap::requestMessageData`); those the server does not have are deleted.
static PENDING_REPLIES: std::sync::Mutex<Vec<(PeerKey, i32)>> = std::sync::Mutex::new(Vec::new());
static ASKED_REPLIES: std::sync::LazyLock<std::sync::Mutex<HashSet<(PeerKey, i32)>>> =
    std::sync::LazyLock::new(Default::default);
static DELETED_REPLIES: std::sync::LazyLock<std::sync::Mutex<HashSet<(PeerKey, i32)>>> =
    std::sync::LazyLock::new(Default::default);

/// The replied message's line (`Reply::update`): its sender and first line from
/// the cache; "Loading..." while it is asked for, "Deleted message" if it is gone.
fn reply_line(
    store: &Storage,
    chat: PeerKey,
    reply: &describe::ReplyTo,
    ctx: &Context,
) -> Option<ReplyLine> {
    let msg_id = reply.msg_id?;
    let peer = reply.peer.unwrap_or(chat);
    let quote = reply.quote.clone();
    let Some(row) = store.message(peer, msg_id).ok().flatten() else {
        // A reply to a chat we can't see: the sender only (`reply_from`).
        if let Some(name) = reply
            .from_name
            .clone()
            .or_else(|| reply.from.map(|k| peer_name(store, k)))
        {
            return Some(ReplyLine {
                peer,
                msg_id,
                color: reply.from.map(|k| author_of(store, k).color),
                name,
                text: quote.clone().unwrap_or_default(),
                quote: quote.is_some(),
            });
        }
        let key = (peer, msg_id);
        let deleted = DELETED_REPLIES.lock().unwrap().contains(&key);
        if !deleted && ASKED_REPLIES.lock().unwrap().insert(key) {
            PENDING_REPLIES.lock().unwrap().push(key);
        }
        return Some(ReplyLine {
            peer,
            msg_id,
            name: if deleted {
                "Deleted message".into()
            } else {
                "Loading...".into()
            },
            color: None,
            text: String::new(),
            quote: false,
        });
    };
    let me = PeerKey::user(ctx.self_id);
    let sender = if row.out && peer.kind != PeerKind::Channel {
        me
    } else {
        row.from.unwrap_or(peer)
    };
    let is_channel = store
        .peer(peer)
        .ok()
        .flatten()
        .is_some_and(|p| describe::peer_info(&p).kind == ChatKind::Channel);
    let (media, service, _) = message_content(store, &row, is_channel);
    let text = match quote.clone() {
        Some(q) => q,
        None => {
            let full = row_text(&row);
            let first = full.lines().next().unwrap_or("").to_string();
            if first.is_empty() {
                media.or(service).unwrap_or_default()
            } else {
                first
            }
        }
    };
    Some(ReplyLine {
        peer,
        msg_id,
        name: peer_name(store, sender),
        color: Some(author_of(store, sender).color),
        text,
        quote: quote.is_some(),
    })
}

/// Asks the server for the replied messages that were missing, then shows the
/// open chat again with them.
async fn resolve_replies(
    sync: &mut Sync,
    open: Option<PeerKey>,
    shown: &Shown,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    let pending: Vec<(PeerKey, i32)> = std::mem::take(&mut *PENDING_REPLIES.lock().unwrap());
    if pending.is_empty() {
        return Ok(());
    }
    let mut by_peer: HashMap<PeerKey, Vec<i32>> = HashMap::new();
    for (peer, id) in pending {
        by_peer.entry(peer).or_default().push(id);
    }
    for (peer, ids) in by_peer {
        match sync.fetch_messages(peer, &ids).await {
            Ok(found) => {
                let mut deleted = DELETED_REPLIES.lock().unwrap();
                for id in ids.iter().filter(|id| !found.contains(id)) {
                    deleted.insert((peer, *id));
                }
            }
            Err(e) => {
                error!("replied messages: {e}");
                let mut deleted = DELETED_REPLIES.lock().unwrap();
                for id in &ids {
                    deleted.insert((peer, *id));
                }
            }
        }
    }
    if let Some(peer) = open {
        send_history(sync.store(), peer, shown, None, tx)?;
    }
    Ok(())
}

/// A hidden sender ("Forwarded from" a name only): initials and a color from the
/// name (tdesktop derives a fake peer id from it).
fn hidden_author(name: &str) -> Author {
    let hash = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
    Author {
        key: None,
        initials: initials(name),
        color: (hash % 7) as u8,
        photo: None,
    }
}

/// Chats shown at first; every `LoadMoreDialogs` adds a page.
const DIALOGS: usize = 200;
const DIALOGS_STEP: usize = 100;
const HISTORY: usize = 60;

pub struct Paths {
    pub session: PathBuf,
    pub cache: PathBuf,
    /// The file cache (userpics, media).
    pub files: PathBuf,
}

/// Userpics are drawn at up to 46 px (dialogs) on screens up to 2x: decode once at this
/// size, off the UI thread.
const USERPIC_SIZE: u32 = 96;

pub(crate) fn userpic_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let mut scaled = image::imageops::resize(
        &decoded,
        USERPIC_SIZE,
        USERPIC_SIZE,
        image::imageops::FilterType::Triangle,
    );
    for px in scaled.pixels_mut() {
        px.0.swap(0, 2); // GPUI wants BGRA
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(scaled)
    ])))
}

/// How many messages to load when a chat opens: enough to reach the first unread one
/// (tdesktop opens there), within reason.
fn open_count(store: &Storage, peer: PeerKey) -> usize {
    let unread = store
        .read_marks(peer)
        .ok()
        .flatten()
        .map_or(0, |(unread, _, _)| unread.max(0) as usize);
    HISTORY.max((unread + 20).min(400))
}

fn members_text(store: &Storage, peer: PeerKey, count: i32) -> String {
    let channel = store
        .peer(peer)
        .ok()
        .flatten()
        .is_some_and(|p| describe::peer_info(&p).kind == ChatKind::Channel);
    if channel {
        present::count_text(count, "subscriber", "subscribers")
    } else {
        present::count_text(count, "member", "members")
    }
}

/// Starts the data thread.
/// An emoji set with its emoji.
#[derive(Clone, Debug)]
pub struct EmojiSet {
    pub info: tdr_data::emoji_sets::SetInfo,
    pub emoji: Vec<tdr_data::emoji_sets::EmojiDocument>,
}

fn emoji_set_of(full: &tdr_data::emoji_sets::FullSet) -> EmojiSet {
    EmojiSet {
        info: full.info.clone(),
        emoji: full
            .documents
            .iter()
            .filter_map(tdr_data::emoji_sets::emoji_document)
            .collect(),
    }
}

/// The installed sets with their emoji (fetched together) and the featured ones.
async fn load_emoji_sets(
    client: &tdr_api::Client,
) -> anyhow::Result<(Vec<EmojiSet>, Vec<EmojiSet>)> {
    use tdr_data::emoji_sets::{self, SetRef};
    let infos = emoji_sets::installed_emoji_sets(client).await?;
    let mut tasks = Vec::new();
    for info in infos {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let raw =
                emoji_sets::sticker_set(&client, &SetRef::Id(info.id, info.access_hash)).await;
            raw.ok()
                .and_then(|r| emoji_sets::full_set(&r))
                .map(|f| emoji_set_of(&f))
        }));
    }
    let mut installed = Vec::new();
    for t in tasks {
        if let Ok(Some(set)) = t.await {
            installed.push(set);
        }
    }
    let listed: Vec<_> = emoji_sets::featured_emoji_sets(client)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !installed.iter().any(|i| i.info.id == f.info.id))
        .collect();
    // Most trending sets come without their emoji (`stickerSetNoCovered`):
    // they are asked for, as tdesktop's `scheduleStickerSetRequest`, a few at
    // a time.
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = Vec::new();
    for f in listed {
        let client = client.clone();
        let limit = limit.clone();
        tasks.push(tokio::spawn(async move {
            let mut emoji: Vec<_> = f
                .documents
                .iter()
                .filter_map(emoji_sets::emoji_document)
                .collect();
            if emoji.len() < f.info.count.max(1) as usize {
                let _permit = limit.acquire().await;
                let raw =
                    emoji_sets::sticker_set(&client, &SetRef::Id(f.info.id, f.info.access_hash))
                        .await;
                if let Some(full) = raw.ok().and_then(|r| emoji_sets::full_set(&r)) {
                    emoji = emoji_set_of(&full).emoji;
                }
            }
            EmojiSet {
                info: f.info,
                emoji,
            }
        }));
    }
    let mut featured = Vec::new();
    for t in tasks {
        if let Ok(set) = t.await {
            featured.push(set);
        }
    }
    Ok((installed, featured))
}

/// Work for the userpic thread.
enum PicJob {
    /// The UI wants this userpic.
    Want {
        peer: PeerKey,
        photo_id: i64,
        dc_id: i32,
    },
    /// The UI wants these custom emoji.
    WantEmoji(Vec<i64>),
    /// A custom emoji's document and file were downloaded: store and show them.
    EmojiDownloaded { doc_blob: Vec<u8>, bytes: Vec<u8> },
    /// It was downloaded: store it and show it.
    Downloaded {
        peer: PeerKey,
        photo_id: i64,
        bytes: Vec<u8>,
    },
}

fn userpic_key(photo_id: i64) -> String {
    format!("userpic:{photo_id}:small")
}

/// Userpics come from the file cache on a thread of their own, from the first frame:
/// the data thread is busy catching up with the server at start (and once was the
/// only one reading the cache, so cached userpics waited for that). Misses go to the
/// data thread to download; downloads come back here to be stored.
fn misses_tx_emoji(misses: &mpsc::UnboundedSender<Command>, ids: Vec<i64>) -> bool {
    misses.send(Command::FetchCustomEmoji(ids)).is_ok()
}

fn spawn_userpics(
    files_dir: PathBuf,
    jobs: std::sync::mpsc::Receiver<PicJob>,
    misses: mpsc::UnboundedSender<Command>,
    tx: mpsc::UnboundedSender<Update>,
) {
    let misses_cmd = misses.clone();
    std::thread::Builder::new()
        .name("userpics".into())
        .spawn(move || {
            let mut files = match FileCache::open(&files_dir, FileCacheConfig::default()) {
                Ok(f) => f,
                Err(e) => {
                    error!("file cache: {e}");
                    return;
                }
            };
            let mut from_cache = 0usize;
            for job in jobs {
                match job {
                    PicJob::Want {
                        peer,
                        photo_id,
                        dc_id,
                    } => match files.get(&userpic_key(photo_id)) {
                        Ok(Some(bytes)) if let Some(image) = userpic_image(&bytes) => {
                            from_cache += 1;
                            if from_cache == 1 || from_cache.is_multiple_of(10) {
                                tracing::info!(
                                    elapsed = ?crate::STARTED.elapsed(),
                                    from_cache,
                                    "userpics from the file cache"
                                );
                            }
                            let _ = tx.send(Update::Userpic {
                                peer,
                                photo_id,
                                image,
                                bytes: bytes.into(),
                            });
                        }
                        _ => {
                            let _ = misses.send(Command::LoadUserpic {
                                peer,
                                photo_id,
                                dc_id,
                            });
                        }
                    },
                    PicJob::WantEmoji(ids) => {
                        let mut misses = Vec::new();
                        for id in ids {
                            let doc = files
                                .get(&format!("emoji-doc:{id}"))
                                .ok()
                                .flatten()
                                .and_then(|b| tdr_data::emoji_sets::decode_document(&b));
                            let file = doc.as_ref().and_then(|_| {
                                files.get(&format!("emoji-file:{id}")).ok().flatten()
                            });
                            match (doc, file) {
                                (Some(doc), Some(bytes)) => {
                                    let _ = tx.send(Update::CustomEmoji { doc, bytes });
                                }
                                _ => misses.push(id),
                            }
                        }
                        if !misses.is_empty() {
                            let _ = misses_tx_emoji(&misses_cmd, misses);
                        }
                    }
                    PicJob::EmojiDownloaded { doc_blob, bytes } => {
                        let Some(doc) = tdr_data::emoji_sets::decode_document(&doc_blob) else {
                            continue;
                        };
                        if let Err(e) = files.put(&format!("emoji-doc:{}", doc.id), &doc_blob) {
                            error!("file cache: {e}");
                        }
                        if let Err(e) = files.put(&format!("emoji-file:{}", doc.id), &bytes) {
                            error!("file cache: {e}");
                        }
                        let _ = tx.send(Update::CustomEmoji { doc, bytes });
                    }
                    PicJob::Downloaded {
                        peer,
                        photo_id,
                        bytes,
                    } => {
                        if let Err(e) = files.put(&userpic_key(photo_id), &bytes) {
                            error!("file cache: {e}");
                        }
                        if let Some(image) = userpic_image(&bytes) {
                            let _ = tx.send(Update::Userpic {
                                peer,
                                photo_id,
                                image,
                                bytes: bytes.into(),
                            });
                        }
                    }
                }
            }
        })
        .expect("spawn userpic thread");
}

pub fn start(
    config: Config,
    paths: Paths,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<Update>,
) {
    let (cmd_tx, mut ui_rx) = mpsc::unbounded_channel::<Command>();
    let (data_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (up_tx, up_rx) = mpsc::unbounded_channel();
    let (pics_tx, pics_rx) = std::sync::mpsc::channel::<PicJob>();
    spawn_userpics(paths.files.clone(), pics_rx, data_tx.clone(), up_tx.clone());
    // Userpic requests go to the userpic thread, everything else to the data thread.
    {
        let pics_tx = pics_tx.clone();
        std::thread::Builder::new()
            .name("commands".into())
            .spawn(move || {
                while let Some(cmd) = ui_rx.blocking_recv() {
                    match cmd {
                        Command::LoadUserpic {
                            peer,
                            photo_id,
                            dc_id,
                        } => {
                            let _ = pics_tx.send(PicJob::Want {
                                peer,
                                photo_id,
                                dc_id,
                            });
                        }
                        Command::LoadCustomEmoji(ids) => {
                            let _ = pics_tx.send(PicJob::WantEmoji(ids));
                        }
                        cmd => {
                            if data_tx.send(cmd).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("spawn command thread");
    }
    std::thread::Builder::new()
        .name("data".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                if let Err(e) = run(config, paths, cmd_rx, up_tx.clone(), pics_tx).await {
                    error!("data thread: {e:#}");
                    let _ = up_tx.send(Update::Fatal(format!("{e:#}")));
                }
            });
        })
        .expect("spawn data thread");
    (cmd_tx, up_rx)
}

/// Every cached chat, the main list and the archive (what folders pick from).
const ALL_DIALOGS: usize = 100_000;

/// The full info of an opened supergroup or channel: its member count for the
/// header, then the list and the header data again (the gift button).
async fn load_full(
    sync: &mut Sync,
    peer: PeerKey,
    shown_dialogs: usize,
    folders: &mut Folders,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    // A basic group: its participants' online state only.
    if peer.kind == PeerKind::Chat {
        let members = sync
            .store()
            .peer(peer)
            .ok()
            .flatten()
            .and_then(|p| describe::peer_info(&p).members)
            .unwrap_or(0);
        send_online(sync, peer, members, tx).await;
        return Ok(());
    }
    match sync.load_full(peer).await {
        Ok(Some(full)) => {
            if let Some(n) = full.members {
                let text = members_text(sync.store(), peer, n);
                let _ = tx.send(Update::Members { peer, text });
                let group = sync
                    .store()
                    .peer(peer)
                    .ok()
                    .flatten()
                    .is_some_and(|p| describe::peer_info(&p).kind == ChatKind::Group);
                if group && full.can_view_participants {
                    send_online(sync, peer, n, tx).await;
                }
            }
            send_dialogs(sync.store(), shown_dialogs, folders, tx)?;
            if let Some(item) = chat_info(sync.store(), peer) {
                let _ = tx.send(Update::ChatInfo(Box::new(item)));
            }
        }
        Ok(None) => {}
        Err(e) => error!("full info: {e}"),
    }
    Ok(())
}

/// The toast after a failed join (`ApiWrap::joinChannel`'s errors).
fn join_error_text(kind: &str, channel: bool) -> Option<&'static str> {
    Some(match kind {
        "INVITE_REQUEST_SENT" if channel => {
            "You will be added to the channel once its admins approve your request."
        }
        "INVITE_REQUEST_SENT" => {
            "You will be added to the group once an admin approves your request."
        }
        "CHANNEL_PRIVATE" | "CHANNEL_PUBLIC_GROUP_NA" | "USER_BANNED_IN_CHANNEL" if channel => {
            "Sorry, this channel is not accessible."
        }
        "CHANNEL_PRIVATE" | "CHANNEL_PUBLIC_GROUP_NA" | "USER_BANNED_IN_CHANNEL" => {
            "Sorry, this group is not accessible."
        }
        "USERS_TOO_MUCH" => "Sorry, this group is full.",
        // tdesktop shows `ChannelsLimitBox` here (not done yet).
        "CHANNELS_TOO_MUCH" => "Too Many Communities. Please leave some before joining a new one.",
        _ => return None,
    })
}

/// "26 members, 3 online" (`lng_chat_status_members_online`) when someone besides
/// us is online.
async fn send_online(
    sync: &mut Sync,
    peer: PeerKey,
    members: i32,
    tx: &mpsc::UnboundedSender<Update>,
) {
    match sync.online_members(peer, members).await {
        Ok(Some((members, online, only_me))) if online > 0 && !only_me => {
            let text = format!(
                "{}, {online} online",
                present::count_text(members, "member", "members")
            );
            let _ = tx.send(Update::Members { peer, text });
        }
        Ok(_) => {}
        Err(e) => error!("online: {e}"),
    }
}

/// `History::shouldBeInChatList`: not a basic group upgraded to a supergroup, and
/// not a group or channel we are not in (one opened by a link has its row, for its
/// history, but no place in the list).
fn in_chat_list(d: &DialogView) -> bool {
    d.peer.as_ref().is_none_or(|p| {
        let info = describe::peer_info(p);
        !info.migrated && (p.key.kind == PeerKind::User || !info.left)
    })
}

fn send_dialogs(
    store: &Storage,
    shown: usize,
    folders: &mut Folders,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    let ctx = Context::load(store);
    let listed = in_chat_list;
    // Folders pick from every chat: their badges, and the open folder's list.
    let everything: Vec<DialogView> = if folders.any() {
        let mut all = store.dialogs(0, ALL_DIALOGS)?;
        all.extend(store.dialogs(1, ALL_DIALOGS)?);
        all.retain(listed);
        all
    } else {
        Vec::new()
    };
    let traits: Vec<ChatTraits> = everything.iter().map(|d| chat_traits(d, &ctx)).collect();
    let items: Vec<DialogItem> = match folders.filter(folders.active) {
        None => store
            .dialogs(0, shown)?
            .iter()
            .filter(|d| listed(d))
            .map(|d| dialog_item(store, d, &ctx))
            .collect(),
        Some(filter) => {
            // The folder's pinned chats first, in its order, then by time
            // (`Dialogs::MainList` of the filter).
            let mut picked: Vec<(usize, &DialogView)> = everything
                .iter()
                .zip(&traits)
                .filter(|(_, t)| filter.contains(t))
                .map(|(d, _)| {
                    let pin = filter.pinned.iter().position(|p| *p == d.dialog.peer);
                    (pin.unwrap_or(usize::MAX), d)
                })
                .collect();
            picked.sort_by(|a, b| a.0.cmp(&b.0).then(list_time(b.1).cmp(&list_time(a.1))));
            picked
                .into_iter()
                .map(|(pin, d)| DialogItem {
                    pinned: pin != usize::MAX,
                    ..dialog_item(store, d, &ctx)
                })
                .collect()
        }
    };
    let _ = tx.send(Update::Dialogs {
        folder: folders.active,
        list: items,
    });
    // The column: "All chats" and the folders, with their unread chats.
    let column: Vec<FolderItem> = if folders.any() {
        folders
            .entries
            .iter()
            .map(|e| match e {
                FilterEntry::All => FolderItem {
                    id: 0,
                    title: "All chats".into(),
                    icon: FilterIcon::All,
                    badge: filters::unread_badge(traits.iter(), None),
                },
                FilterEntry::Folder(f) => FolderItem {
                    id: f.id,
                    title: f.title.clone(),
                    icon: f.icon(),
                    badge: filters::unread_badge(traits.iter(), Some(f)),
                },
            })
            .collect()
    } else {
        Vec::new()
    };
    if folders.sent.as_ref() != Some(&column) {
        folders.sent = Some(column.clone());
        let _ = tx.send(Update::Folders(column));
    }
    let archive = archive_info(store)?;
    if folders.archive_sent.as_ref() != Some(&archive) {
        folders.archive_sent = Some(archive.clone());
        let _ = tx.send(Update::Archive(archive));
    }
    if folders.archive_open {
        let list = store
            .dialogs(tdr_data::ARCHIVE_FOLDER, folders.archive_shown)?
            .iter()
            .filter(|d| listed(d))
            .map(|d| dialog_item(store, d, &ctx))
            .collect();
        let _ = tx.send(Update::Dialogs {
            folder: ARCHIVE_LIST,
            list,
        });
    }
    let state = account_state(store, &ctx);
    if folders.state_sent != Some(state) {
        folders.state_sent = Some(state);
        let _ = tx.send(Update::AccountState(state));
    }
    Ok(())
}

/// `Folder::kShowChatNamesCount`: the archive's row names this many chats.
const ARCHIVE_NAMES: usize = 8;
/// Archived chats looked at for the row (their unread count, the latest ones).
const ARCHIVE_SCAN: usize = 1000;

/// The archive's row from the cache (`Folder::lastHistories`,
/// `ComposeFolderListEntryText`): the 8 latest chats by their last message, the
/// last of them left out when only one more would follow ("and 2 more chats").
fn archive_info(store: &Storage) -> anyhow::Result<Option<ArchiveInfo>> {
    let mut list = store.dialogs(tdr_data::ARCHIVE_FOLDER, ARCHIVE_SCAN)?;
    list.retain(in_chat_list);
    if list.is_empty() {
        return Ok(None);
    }
    let in_menu = store
        .meta_i64(tdr_data::ARCHIVE_IN_MENU_KEY)?
        .is_some_and(|v| v != 0);
    let count = (store
        .meta_i64(tdr_data::ARCHIVE_COUNT_KEY)?
        .unwrap_or(0)
        .max(0) as usize)
        .max(list.len());
    let unread_of = |d: &DialogView| d.dialog.unread_count > 0 || d.dialog.unread_mark;
    let unread = list.iter().filter(|d| unread_of(d)).count() as u32;
    list.sort_by_key(|d| std::cmp::Reverse(list_time(d)));
    let last = &list[..list.len().min(ARCHIVE_NAMES)];
    let throw_away = last.len() > 1 && count == last.len() + 1;
    let shown = &last[..last.len() - usize::from(throw_away)];
    Ok(Some(ArchiveInfo {
        in_menu,
        names: shown
            .iter()
            .map(|d| {
                let name = d.peer.as_ref().map_or_else(String::new, |p| p.name.clone());
                (name, unread_of(d))
            })
            .collect(),
        more: count - shown.len(),
        unread,
    }))
}

/// The header data of a chat that may not be in the loaded list (a chat opened
/// from a forward): from its cached dialog, or just its peer.
fn chat_info(store: &Storage, peer: PeerKey) -> Option<DialogItem> {
    let row = store.peer(peer).ok().flatten()?;
    let view = DialogView {
        dialog: tdr_data::tdr_storage::DialogRow {
            peer,
            top_message: 0,
            top_date: 0,
            pinned: false,
            folder_id: 0,
            unread_count: 0,
            pts: None,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            mute_until: None,
            unread_mentions: 0,
            unread_mark: false,
        },
        peer: Some(row),
        top: None,
        draft: None,
    };
    Some(dialog_item(store, &view, &Context::load(store)))
}

/// What part of the open chat's history is shown.
enum Shown {
    /// The newest messages of the cached segment.
    Latest(usize),
    /// A thread (`messages.getReplies`): its loaded messages, newest first; a
    /// post's comments begin with the root (the post's copy in the group).
    Thread {
        root: i32,
        rows: Vec<MessageRow>,
        older_done: bool,
        /// The roots of a post's comments (shown first, "Discussion started").
        roots: Vec<MessageRow>,
    },
    /// A window around a message the segment does not reach (newest first), grown by
    /// scrolling until it meets the newest messages.
    Window {
        rows: Vec<MessageRow>,
        older_done: bool,
        /// Nothing newer: the window reached the chat's last message (a chat with
        /// no cached dialog to join, e.g. one the chat list does not have).
        newer_done: bool,
    },
}

/// Messages per page of a window.
const WINDOW_PAGE: i32 = 50;
/// The "Discussion started" line of a post's comments (not a real message).
pub const DISCUSSION_STARTED_ID: i32 = -100;
/// Topics asked for at once.
const TOPICS_PAGE: i32 = 100;

/// What a chat left shows when it is opened again: a thread is not kept.
fn keep_shown(shown: Shown) -> Shown {
    match shown {
        Shown::Thread { .. } => Shown::Latest(HISTORY),
        other => other,
    }
}

/// New messages of an open thread join it (they belong to it by their reply
/// header: `reply_to_top_id`, or a reply to the root itself).
fn absorb_into_thread(shown: &mut Shown, peer: PeerKey, new: &[MessageRow]) {
    let Shown::Thread { root, rows, .. } = shown else {
        return;
    };
    for m in new.iter().filter(|m| m.peer == peer) {
        let in_thread = describe::message_info(m)
            .reply
            .is_some_and(|r| r.top_id == Some(*root) || r.msg_id == Some(*root));
        if in_thread && !rows.iter().any(|r| r.id == m.id) {
            rows.insert(0, m.clone());
        }
    }
    rows.sort_by_key(|m| std::cmp::Reverse(m.id));
}

/// A forum's topics as its list shows them (`Dialogs::Ui::PaintRow` of a topic):
/// the last message's sender and text, its time, the unread count.
fn topic_items(
    store: &Storage,
    peer: PeerKey,
    topics: &[tdr_data::Topic],
    ctx: &Context,
) -> Vec<TopicItem> {
    let now = Local::now();
    topics
        .iter()
        .filter(|t| !t.hidden || t.id == 1)
        .map(|t| {
            let top = store.message(peer, t.top_message).ok().flatten();
            let last = top.as_ref().map_or_else(LastLine::default, |m| {
                let (media, service, info) = message_content(store, m, false);
                let full = row_text(m);
                let text = full.lines().next().unwrap_or("").to_string();
                let prefix = (service.is_none())
                    .then(|| {
                        if m.out {
                            Some("You".to_string())
                        } else {
                            m.from.map(|f| short_name(store, f, ctx.self_id))
                        }
                    })
                    .flatten();
                match (service, media) {
                    (Some(s), _) => LastLine {
                        prefix: None,
                        accent: Some(s),
                        text: String::new(),
                        spoilers: Vec::new(),
                        custom: Vec::new(),
                    },
                    (None, Some(label)) if text.is_empty() => LastLine {
                        prefix,
                        accent: Some(label),
                        text: String::new(),
                        spoilers: Vec::new(),
                        custom: Vec::new(),
                    },
                    _ => LastLine {
                        prefix,
                        accent: None,
                        custom: custom_in_line(&info, &full, &text),
                        text,
                        spoilers: Vec::new(),
                    },
                }
            });
            TopicItem {
                id: t.id,
                title: t.title.clone(),
                icon_color: t.icon_color,
                general: t.id == 1,
                last,
                time: top
                    .as_ref()
                    .map(|m| present::dialogs_date(m.date, now))
                    .unwrap_or_default(),
                unread: t.unread,
                muted: t.muted,
                pinned: t.pinned,
                closed: t.closed,
            }
        })
        .collect()
}

fn send_history(
    store: &Storage,
    peer: PeerKey,
    shown: &Shown,
    focus: Option<i32>,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    let update = match shown {
        Shown::Latest(count) => {
            let h = store.history(peer, *count)?;
            // Complete only once every cached message is shown, not just cached.
            let complete = h.complete && h.messages.len() < *count;
            Update::History {
                peer,
                messages: message_items(store, peer, &h.messages),
                complete,
                newer: false,
                focus,
                thread: None,
            }
        }
        Shown::Window {
            rows,
            older_done,
            newer_done,
        } => Update::History {
            peer,
            messages: message_items(store, peer, rows),
            complete: *older_done,
            newer: !*newer_done,
            focus,
            thread: None,
        },
        Shown::Thread {
            root,
            rows,
            older_done,
            roots,
        } => {
            // Edited and deleted ones as the cache has them now.
            let fresh = |r: &MessageRow| store.message(r.peer, r.id).ok().flatten();
            let rows: Vec<MessageRow> = rows.iter().filter_map(fresh).collect();
            let mut messages = Vec::new();
            if *older_done {
                let roots: Vec<MessageRow> = roots.iter().filter_map(fresh).collect();
                messages = message_items(store, peer, &roots);
                // `lng_replies_discussion_started` under the post.
                if let Some(last) = messages.last().cloned() {
                    messages.push(MessageItem {
                        id: DISCUSSION_STARTED_ID,
                        out: false,
                        text: String::new(),
                        media: None,
                        service: Some("Discussion started".into()),
                        edited: false,
                        read: None,
                        time: last.time.clone(),
                        date: last.date,
                        sender: None,
                        author: last.author.clone(),
                        forwarded: None,
                        original: None,
                        entities: Vec::new(),
                        forward_date: None,
                        edit_date: None,
                        post: None,
                        reply: None,
                        thread_root: None,
                        reactions: None,
                    });
                }
            }
            messages.extend(message_items(store, peer, &rows));
            Update::History {
                peer,
                messages,
                complete: *older_done,
                newer: false,
                focus,
                thread: Some(*root),
            }
        }
    };
    let _ = tx.send(update);
    Ok(())
}

fn changed(a: &Applied) -> bool {
    !a.new_messages.is_empty()
        || a.edited > 0
        || a.deleted > 0
        || a.reloaded
        || a.marks_changed
        || a.pinned_changed
        || a.statuses_changed
        || !a.draft_changes.is_empty()
}

/// Loads pages of the server's chat list until the `shown` first chats are confirmed
/// (or the list ends). Returns whether anything was loaded.
async fn confirm_dialogs(sync: &mut Sync, shown: usize) -> bool {
    let mut any = false;
    while sync.dialogs_loaded() < shown && !sync.dialogs_complete() {
        match sync.load_more_dialogs().await {
            Ok(n) => any |= n > 0,
            Err(e) => {
                error!("chat list: {e}");
                break;
            }
        }
    }
    any
}

/// Drafts changed from another device go to the input fields.
fn send_draft_changes(applied: &Applied, tx: &mpsc::UnboundedSender<Update>) {
    for (peer, text) in &applied.draft_changes {
        let _ = tx.send(Update::Draft {
            peer: *peer,
            text: text.clone(),
        });
    }
}

/// tdesktop's `kSaveCloudDraftTimeout`: drafts go to the server this long after the
/// chat is left (changes in the meantime are batched).
const SAVE_CLOUD_DRAFT_DELAY: Duration = Duration::from_millis(1000);

async fn run(
    config: Config,
    paths: Paths,
    mut commands: mpsc::UnboundedReceiver<Command>,
    tx: mpsc::UnboundedSender<Update>,
    pics: std::sync::mpsc::Sender<PicJob>,
) -> anyhow::Result<()> {
    // 1. The cached chat list, before any network.
    if let Some(dir) = paths.cache.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let store = Storage::open(&paths.cache)?;
    // How many chats the list shows (grows as it is scrolled down).
    let mut shown_dialogs = DIALOGS;
    let mut folders = Folders::new(tdr_data::cached_dialog_filters(&store).unwrap_or_default());
    send_dialogs(&store, shown_dialogs, &mut folders, &tx)?;
    send_me(&store, &tx);
    info!(elapsed = ?crate::STARTED.elapsed(), "data: cached chat list sent");

    let session = match std::fs::read(&paths.session) {
        Ok(d) => SessionData::from_bytes(&d).map_err(|e| {
            anyhow::anyhow!(
                "The saved session is damaged ({e}). Run `tdr login` once, then start the app again."
            )
        })?,
        Err(_) => anyhow::bail!(
            "Not logged in. Run `tdr login` once (the console client), then start the app again."
        ),
    };
    if !session.is_authorized() {
        anyhow::bail!("Not logged in. Run `tdr login` once, then start the app again.");
    }
    let client = Client::new(config, session);
    let sync_clock = |client: &Client| {
        present::set_server_time_offset(
            client.server_time() - chrono::Utc::now().timestamp() as i32,
        );
    };
    sync_clock(&client);
    let save = |client: &Client| {
        if let Err(e) = client.save_session(&paths.session) {
            error!("session save: {e}");
        }
    };

    // The "Connecting..." state follows the home DC's connection.
    {
        let mut connected = client.connected();
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let now = *connected.borrow_and_update();
                if tx.send(Update::Connected(now)).is_err() || connected.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    // 2. Catch up with the server.
    let mut sync = Sync::new(client.clone(), store)?;
    let started = std::time::Instant::now();
    let applied = sync.start().await?;
    sync_clock(&client);
    info!(elapsed = ?started.elapsed(), "synced");
    save(&client);
    send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
    send_draft_changes(&applied, &tx);
    send_me(sync.store(), &tx);
    // The folders, and more addresses of the DCs for the next connections.
    match sync.dialog_filters().await {
        Ok(list) => {
            folders.set(list);
            send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
        }
        Err(e) => error!("folders: {e}"),
    }
    if let Err(e) = client.refresh_dc_options().await {
        error!("config: {e}");
    }
    save(&client);
    if COUNTRIES.get().is_none() {
        let list = tdr_data::login::countries(&client).await;
        if !list.is_empty() {
            let _ = COUNTRIES.set(list);
            send_me(sync.store(), &tx);
        }
    }
    // The shown part of the cached list may be stale below what the start refreshed.
    if confirm_dialogs(&mut sync, shown_dialogs).await {
        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
    }
    // When to save drafts to the cloud. Drafts left unsaved last time (the app quit
    // first) go up right away.
    let mut cloud_save_at = sync
        .has_unsaved_drafts()
        .then(|| tokio::time::Instant::now() + SAVE_CLOUD_DRAFT_DELAY);

    // Userpics: from the file cache, or downloaded in the background (a few at once).
    let (pics_tx, mut pics_rx) = mpsc::unbounded_channel::<(PeerKey, i64, Option<Vec<u8>>)>();
    let downloads = Arc::new(tokio::sync::Semaphore::new(4));
    // Custom emoji files are small and many on screen at once: their own, wider
    // limit, so they never wait behind userpics (8 at a time measured best).
    let emoji_downloads = Arc::new(tokio::sync::Semaphore::new(8));
    // Documents known without `getCustomEmojiDocuments` (the reactions'
    // animations): downloaded like custom emoji when one is painted.
    let known_docs: Arc<std::sync::Mutex<HashMap<i64, tdr_data::tdr_tl::enums::Document>>> =
        Default::default();
    let mut loading: HashSet<i64> = HashSet::new();

    // 3. Commands and live updates.
    let mut open: Option<PeerKey> = None;
    // How many messages of the open chat are shown (grows as the user scrolls up).
    let mut shown = Shown::Latest(HISTORY);
    // Chats `OpenChatAt` went away from, with what they showed (`Back` returns).
    let mut back: Vec<(PeerKey, Shown)> = Vec::new();
    // What chats left this session showed (`History::scrollTopItem` keeps the
    // place in tdesktop): `ReopenChat` shows it again.
    let mut remembered: HashMap<PeerKey, Shown> = HashMap::new();
    let mut logging_out = false;
    loop {
        resolve_replies(&mut sync, open, &shown, &tx).await?;
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                if matches!(cmd, Command::LogOut) {
                    tdr_data::login::log_out(&client, LOG_OUT_TIMEOUT).await;
                    logging_out = true;
                    break;
                }
                let (cmd, restore) = match cmd {
                    Command::ReopenChat(peer) => (Command::OpenChat(peer), true),
                    cmd => (cmd, false),
                };
                match cmd {
                    Command::Refresh => {
                        if let Some(previous) = open.take() {
                            remembered.insert(
                                previous,
                                keep_shown(std::mem::replace(&mut shown, Shown::Latest(HISTORY))),
                            );
                        }
                        folders.active = 0;
                        folders.sent = None;
                        folders.state_sent = None;
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        send_me(sync.store(), &tx);
                        let _ = tx.send(Update::Connected(*client.connected().borrow()));
                    }
                    Command::SetMuted { peer, mute } => match sync.set_muted(peer, mute).await {
                        Ok(()) => send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?,
                        Err(e) => error!("mute: {e}"),
                    },
                    Command::SelectFolder(id) => {
                        folders.active = id;
                        if folders.filter(id).is_none() {
                            folders.active = 0;
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                    }
                    Command::OpenArchive => {
                        folders.archive_open = true;
                        folders.archive_shown = DIALOGS;
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        if !sync.archive_complete() {
                            match sync.load_more_archive().await {
                                Ok(_) => send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?,
                                Err(e) => error!("archive: {e}"),
                            }
                        }
                    }
                    Command::CloseArchive => folders.archive_open = false,
                    Command::OpenComments { channel, post } => {
                        match sync.discussion(channel, post).await {
                            Ok(Some((group, roots))) => {
                                let root = roots.last().map_or(0, |m| m.id);
                                if let Some(previous) = open {
                                    back.push((previous, std::mem::replace(&mut shown, Shown::Latest(HISTORY))));
                                }
                                open = Some(group);
                                if let Some(item) = chat_info(sync.store(), group) {
                                    let _ = tx.send(Update::ChatInfo(Box::new(item)));
                                }
                                let (rows, count, done) = match sync.replies_page(group, root, 0, WINDOW_PAGE).await {
                                    Ok((rows, count)) => {
                                        let done = (rows.len() as i32) < WINDOW_PAGE;
                                        (rows, count, done)
                                    }
                                    Err(e) => {
                                        error!("comments: {e}");
                                        (Vec::new(), 0, true)
                                    }
                                };
                                let _ = tx.send(Update::ThreadCount { peer: group, root, count });
                                shown = Shown::Thread {
                                    root,
                                    rows,
                                    older_done: done,
                                    roots,
                                };
                                send_history(sync.store(), group, &shown, None, &tx)?;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                error!("discussion: {e}");
                                let _ = tx.send(Update::Toast("Comments are not available.".into()));
                            }
                        }
                    }
                    Command::OpenThread { peer, root } => {
                        // From the topics list, like a chat from the chat list: no
                        // way back to the chat open before.
                        if let Some(previous) = open.take() {
                            remembered.insert(
                                previous,
                                keep_shown(std::mem::replace(&mut shown, Shown::Latest(HISTORY))),
                            );
                        }
                        back.clear();
                        open = Some(peer);
                        if let Some(item) = chat_info(sync.store(), peer) {
                            let _ = tx.send(Update::ChatInfo(Box::new(item)));
                        }
                        let (rows, count, done) = match sync.replies_page(peer, root, 0, WINDOW_PAGE).await {
                            Ok((rows, count)) => {
                                let done = (rows.len() as i32) < WINDOW_PAGE;
                                (rows, count, done)
                            }
                            Err(e) => {
                                error!("topic: {e}");
                                (Vec::new(), 0, true)
                            }
                        };
                        let _ = tx.send(Update::ThreadCount { peer, root, count });
                        shown = Shown::Thread {
                            root,
                            rows,
                            older_done: done,
                            roots: Vec::new(),
                        };
                        send_history(sync.store(), peer, &shown, None, &tx)?;
                    }
                    Command::OpenForum(peer) => match sync.forum_topics(peer, TOPICS_PAGE).await {
                        Ok(topics) => {
                            let ctx = Context::load(sync.store());
                            let topics = topic_items(sync.store(), peer, &topics, &ctx);
                            let _ = tx.send(Update::Topics { peer, topics });
                        }
                        Err(e) => error!("topics: {e}"),
                    },
                    Command::Join(peer) => {
                        match sync.join(peer).await {
                            Ok(Ok(applied)) => send_draft_changes(&applied, &tx),
                            Ok(Err(kind)) => {
                                let channel = sync
                                    .store()
                                    .peer(peer)
                                    .ok()
                                    .flatten()
                                    .is_some_and(|p| describe::peer_info(&p).kind == ChatKind::Channel);
                                if let Some(text) = join_error_text(&kind, channel) {
                                    let _ = tx.send(Update::Toast(text.into()));
                                }
                            }
                            Err(e) => error!("join: {e}"),
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        if let Some(item) = chat_info(sync.store(), peer) {
                            let _ = tx.send(Update::ChatInfo(Box::new(item)));
                        }
                        if open == Some(peer) {
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                    }
                    Command::Leave(peer) => {
                        match sync.leave(peer).await {
                            Ok(applied) => send_draft_changes(&applied, &tx),
                            Err(e) => {
                                error!("leave: {e}");
                                let _ = tx.send(Update::Toast(format!("Couldn't leave: {e}")));
                            }
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                    }
                    Command::SetPhoneHidden(hidden) => {
                        if let Err(e) = sync.set_phone_hidden(hidden) {
                            error!("phone setting: {e}");
                        }
                        send_me(sync.store(), &tx);
                    }
                    Command::ArchiveInMenu(in_menu) => {
                        if let Err(e) = sync.set_archive_in_menu(in_menu) {
                            error!("archive setting: {e}");
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                    }
                    Command::LoadMoreDialogs if folders.archive_open => {
                        folders.archive_shown += DIALOGS_STEP;
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        if !sync.archive_complete() {
                            match sync.load_more_archive().await {
                                Ok(_) => send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?,
                                Err(e) => error!("archive: {e}"),
                            }
                        }
                    }
                    Command::LoadMoreDialogs => {
                        shown_dialogs += DIALOGS_STEP;
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        if confirm_dialogs(&mut sync, shown_dialogs).await {
                            send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        }
                    }
                    Command::ReorderPinned(order) => {
                        if let Err(e) = sync.reorder_pinned(order).await {
                            error!("pinned order: {e}");
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                    }
                    Command::OpenChat(peer) => {
                        back.clear();
                        if let Some(previous) = open.filter(|p| *p != peer) {
                            remembered.insert(
                                previous,
                                keep_shown(std::mem::replace(&mut shown, Shown::Latest(HISTORY))),
                            );
                        }
                        open = Some(peer);
                        // Where it was left: the same messages (a page more, so new
                        // ones at the bottom never push the shown ones out).
                        shown = match (restore, remembered.remove(&peer)) {
                            (true, Some(Shown::Latest(count))) => Shown::Latest(count + HISTORY),
                            (true, Some(window)) => window,
                            _ => Shown::Latest(open_count(sync.store(), peer)),
                        };
                        send_history(sync.store(), peer, &shown, None, &tx)?;
                        if let Shown::Latest(count) = shown {
                            match sync.history(peer, count).await {
                                Ok(_) => send_history(sync.store(), peer, &shown, None, &tx)?,
                                Err(e) => error!("history: {e}"),
                            }
                        }
                        // A private chat's status, fresh from the server.
                        if let Err(e) = sync.refresh_user(peer).await {
                            error!("user: {e}");
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        // Supergroups and channels: the full info (members, gifts).
                        load_full(&mut sync, peer, shown_dialogs, &mut folders, &tx).await?;
                    }
                    Command::Read { peer, max_id } => match sync.read_history(peer, max_id).await {
                        Ok(_) => send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?,
                        Err(e) => error!("read: {e}"),
                    },
                    Command::LoadCustomEmoji(_) => {}
                    Command::LoadReactions => {
                        let client = client.clone();
                        let tx = tx.clone();
                        let known = known_docs.clone();
                        tokio::spawn(async move {
                            match tdr_data::reactions::available(&client).await {
                                Ok((list, raw)) => {
                                    *REACTION_ORDER.lock().unwrap() =
                                        list.iter().map(|r| r.emoticon.clone()).collect();
                                    {
                                        let mut known = known.lock().unwrap();
                                        for doc in raw {
                                            if let tdr_data::tdr_tl::enums::Document::Document(d) = &doc {
                                                known.insert(d.id, doc);
                                            }
                                        }
                                    }
                                    let top = tdr_data::reactions::top(&client)
                                        .await
                                        .map_err(|e| tracing::debug!("top reactions: {e}"))
                                        .unwrap_or_default();
                                    let _ = tx.send(Update::AvailableReactions { list, top });
                                }
                                Err(e) => tracing::debug!("available reactions: {e}"),
                            }
                        });
                    }
                    Command::LoadEmojiGroups => {
                        let client = client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            match tdr_data::emoji_sets::emoji_groups(&client).await {
                                Ok(groups) => {
                                    let _ = tx.send(Update::EmojiGroups(groups));
                                }
                                Err(e) => tracing::debug!("emoji groups: {e}"),
                            }
                        });
                    }
                    Command::RefreshEmojiKeywords { languages, versions } => {
                        let client = client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let remote = match tdr_data::emoji_keywords::languages(&client, languages).await {
                                Ok(remote) => remote,
                                Err(e) => {
                                    tracing::debug!("emoji keywords languages: {e}");
                                    return;
                                }
                            };
                            for lang in &remote {
                                let version = versions
                                    .iter()
                                    .find(|(l, _)| l == lang)
                                    .map_or(0, |(_, v)| *v);
                                match tdr_data::emoji_keywords::difference(&client, lang, version).await {
                                    Ok(d) if d.lang_code == *lang => {
                                        let _ = tx.send(Update::EmojiKeywords {
                                            remote: remote.clone(),
                                            lang: lang.clone(),
                                            version: d.version,
                                            changes: d.changes,
                                        });
                                    }
                                    Ok(_) => {}
                                    Err(e) => tracing::debug!("emoji keywords: {e}"),
                                }
                            }
                        });
                    }
                    Command::LoadEmojiSets => {
                        let client = client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            match load_emoji_sets(&client).await {
                                Ok((installed, featured)) => {
                                    let _ = tx.send(Update::EmojiSets { installed, featured });
                                }
                                Err(e) => tracing::debug!("emoji sets: {e}"),
                            }
                        });
                    }
                    Command::LoadEmojiSet(set) => {
                        let client = client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let found = tdr_data::emoji_sets::sticker_set(&client, &set)
                                .await
                                .ok()
                                .and_then(|r| tdr_data::emoji_sets::full_set(&r))
                                .map(|f| emoji_set_of(&f));
                            let _ = tx.send(Update::EmojiSetLoaded { set, found });
                        });
                    }
                    Command::ToggleEmojiSet { set, install } => {
                        let client = client.clone();
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let done = if install {
                                tdr_data::emoji_sets::install_set(&client, &set).await
                            } else {
                                tdr_data::emoji_sets::uninstall_set(&client, &set).await
                            };
                            if let Err(e) = done {
                                tracing::debug!("emoji set: {e}");
                            }
                            if let Ok((installed, featured)) = load_emoji_sets(&client).await {
                                let _ = tx.send(Update::EmojiSets { installed, featured });
                            }
                        });
                    }
                    // Not in the file cache (the file thread looked): fetch the
                    // documents (`getCustomEmojiDocuments`) and download the files.
                    Command::FetchCustomEmoji(ids) => {
                        let ids: Vec<i64> = ids.into_iter().filter(|id| loading.insert(*id)).collect();
                        let (known, ids) = {
                            let map = known_docs.lock().unwrap();
                            let known: Vec<tdr_data::tdr_tl::enums::Document> =
                                ids.iter().filter_map(|id| map.get(id).cloned()).collect();
                            let rest: Vec<i64> =
                                ids.into_iter().filter(|id| !map.contains_key(id)).collect();
                            (known, rest)
                        };
                        if !ids.is_empty() || !known.is_empty() {
                            let client = client.clone();
                            let done = pics.clone();
                            let downloads = emoji_downloads.clone();
                            let tx = tx.clone();
                            tokio::spawn(async move {
                                let mut docs = known;
                                if !ids.is_empty() {
                                    match tdr_data::emoji_sets::custom_emoji_documents(&client, &ids).await {
                                        Ok(more) => docs.extend(more),
                                        Err(e) => {
                                            tracing::debug!("custom emoji documents: {e}");
                                            for id in ids {
                                                let _ = tx.send(Update::CustomEmojiFailed(id));
                                            }
                                        }
                                    }
                                }
                                let mut tasks = Vec::new();
                                for raw in docs {
                                    let Some(doc) = tdr_data::emoji_sets::emoji_document(&raw) else {
                                        continue;
                                    };
                                    let blob = tdr_data::emoji_sets::encode(&raw);
                                    let client = client.clone();
                                    let done = done.clone();
                                    let downloads = downloads.clone();
                                    let tx = tx.clone();
                                    tasks.push(tokio::spawn(async move {
                                        let _permit = downloads.acquire().await;
                                        match tdr_data::emoji_sets::download_document(&client, &doc).await {
                                            Ok(bytes) => {
                                                let _ = done.send(PicJob::EmojiDownloaded { doc_blob: blob, bytes });
                                            }
                                            Err(e) => {
                                                tracing::debug!("custom emoji file: {e}");
                                                let _ = tx.send(Update::CustomEmojiFailed(doc.id));
                                            }
                                        }
                                    }));
                                }
                                for t in tasks {
                                    let _ = t.await;
                                }
                            });
                        }
                    }
                    // Not in the file cache (the userpic thread looked): download.
                    Command::LoadUserpic { peer, photo_id, dc_id } => {
                        if loading.insert(photo_id)
                            && let Some(input) = sync.input_peer(peer)
                        {
                            let client = client.clone();
                            let done = pics_tx.clone();
                            let downloads = downloads.clone();
                            tokio::spawn(async move {
                                let _permit = downloads.acquire().await;
                                let bytes = tdr_data::download_userpic(&client, input, photo_id, dc_id)
                                    .await
                                    .map_err(|e| tracing::debug!("userpic: {e}"))
                                    .ok();
                                let _ = done.send((peer, photo_id, bytes));
                            });
                        }
                    }
                    Command::CloseChat => {
                        if let Some(previous) = open.take() {
                            remembered.insert(
                                previous,
                                keep_shown(std::mem::replace(&mut shown, Shown::Latest(HISTORY))),
                            );
                        }
                    }
                    // Handled before the match.
                    Command::LogOut => {}
                    Command::SaveDraft { peer, text, cloud } => {
                        if sync.set_draft(peer, &text)? {
                            send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        }
                        if cloud && sync.has_unsaved_drafts() && cloud_save_at.is_none() {
                            cloud_save_at = Some(tokio::time::Instant::now() + SAVE_CLOUD_DRAFT_DELAY);
                        }
                    }
                    Command::LoadOlder(peer) if open == Some(peer) => match &mut shown {
                        Shown::Thread {
                            root,
                            rows,
                            older_done,
                            ..
                        } => {
                            if !*older_done {
                                let oldest = rows.last().map_or(0, |m| m.id);
                                match sync.replies_page(peer, *root, oldest, WINDOW_PAGE).await {
                                    Ok((page, _)) => {
                                        *older_done = page.is_empty();
                                        rows.extend(page.into_iter().filter(|m| oldest == 0 || m.id < oldest));
                                    }
                                    Err(e) => error!("thread: {e}"),
                                }
                            }
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                        Shown::Latest(count) => {
                            *count += HISTORY;
                            let count = *count;
                            if let Err(e) = sync.history(peer, count).await {
                                error!("history: {e}");
                            }
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                        Shown::Window {
                            rows, older_done, ..
                        } => {
                            if let Some(oldest) = rows.last().map(|m| m.id) {
                                match sync.history_page(peer, oldest, 0, WINDOW_PAGE).await {
                                    Ok(page) => {
                                        // Only an empty page is the start.
                                        *older_done = page.is_empty();
                                        rows.extend(page.into_iter().filter(|m| m.id < oldest));
                                    }
                                    Err(e) => error!("history: {e}"),
                                }
                            }
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                    },
                    Command::LoadOlder(_) => {}
                    Command::LoadNewer(peer) if open == Some(peer) => {
                        if let Shown::Window {
                            rows, newer_done, ..
                        } = &mut shown
                            && let Some(newest) = rows.first().map(|m| m.id)
                        {
                            let mut got_newer = false;
                            match sync.history_page(peer, newest, -WINDOW_PAGE, WINDOW_PAGE).await {
                                Ok(page) => {
                                    let mut newer: Vec<MessageRow> =
                                        page.into_iter().filter(|m| m.id > newest).collect();
                                    got_newer = !newer.is_empty();
                                    newer.append(rows);
                                    *rows = newer;
                                }
                                Err(e) => error!("history: {e}"),
                            }
                            // The window met the newest message of the cached history:
                            // it joins it and the chat is shown as usual from here. A
                            // chat without a cached dialog (not in the chat list) stays
                            // a window, complete once nothing newer comes.
                            let top = sync.store().history(peer, 1)?.messages.first().map(|m| m.id);
                            let reached = top.is_some_and(|top| rows.first().is_some_and(|m| m.id >= top));
                            if top.is_none() && !got_newer {
                                *newer_done = true;
                            }
                            if reached {
                                let window = std::mem::take(rows);
                                if let Err(e) = sync.join_window(peer, &window) {
                                    error!("history: {e}");
                                }
                                shown = Shown::Latest(window.len().max(HISTORY));
                            }
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                    }
                    Command::LoadNewer(_) | Command::ReopenChat(_) => {}
                    Command::Back => {
                        if let Some((peer, was)) = back.pop() {
                            if let Some(previous) = open {
                                remembered.insert(
                                    previous,
                                    keep_shown(std::mem::replace(&mut shown, was)),
                                );
                            } else {
                                shown = was;
                            }
                            open = Some(peer);
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                    }
                    Command::OpenChatAt { peer, msg_id } => {
                        if let Some(previous) = open.filter(|p| *p != peer) {
                            back.push((previous, std::mem::replace(&mut shown, Shown::Latest(HISTORY))));
                        }
                        open = Some(peer);
                        if let Some(item) = chat_info(sync.store(), peer) {
                            let _ = tx.send(Update::ChatInfo(Box::new(item)));
                        }
                        // A chat never seen (a channel opened by its link): its row
                        // first, since its loaded history is read through it.
                        if !sync.store().has_dialog(peer)?
                            && let Err(e) = sync.request_dialog_entry(peer).await
                        {
                            error!("dialog entry: {e}");
                        }
                        // In the cached history: shown from the newest down to it.
                        let cached_from = match msg_id {
                            Some(id) => sync.store().count_from(peer, id)?,
                            None => None,
                        };
                        if let Some(n) = cached_from {
                            shown = Shown::Latest(open_count(sync.store(), peer).max(n + 30));
                        } else if msg_id.is_none() {
                            // The latest messages: the cached ones now, then fresh.
                            shown = Shown::Latest(open_count(sync.store(), peer));
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                            if let Err(e) = sync.history(peer, open_count(sync.store(), peer)).await {
                                error!("history: {e}");
                            }
                        } else {
                            let msg_id = msg_id.unwrap_or(0);
                            match sync.history_around(peer, msg_id, WINDOW_PAGE * 2).await {
                                Ok(Some(rows)) => {
                                    shown = Shown::Window {
                                        rows,
                                        older_done: false,
                                        newer_done: false,
                                    };
                                }
                                Ok(None) => {
                                    let n = sync.store().count_from(peer, msg_id)?.unwrap_or(0);
                                    shown = Shown::Latest(HISTORY.max(n + 30));
                                }
                                Err(e) => {
                                    error!("history: {e}");
                                    shown = Shown::Latest(open_count(sync.store(), peer));
                                }
                            }
                        }
                        send_history(sync.store(), peer, &shown, msg_id, &tx)?;
                        // Its peer came with the history if it was unknown.
                        if let Some(item) = chat_info(sync.store(), peer) {
                            let _ = tx.send(Update::ChatInfo(Box::new(item)));
                        }
                        // Not in the chat list: it joins it by its last message, as
                        // tdesktop lists every chat it loaded; the list shows far
                        // enough down to have it.
                        if let Err(e) = sync.request_dialog_entry(peer).await {
                            error!("dialog entry: {e}");
                        }
                        let all = sync.store().dialogs(0, usize::MAX >> 1)?;
                        if let Some(pos) = all.iter().position(|d| d.dialog.peer == peer) {
                            shown_dialogs = shown_dialogs.max(pos + 1);
                        }
                        if let Err(e) = sync.refresh_user(peer).await {
                            error!("user: {e}");
                        }
                        send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                        // Supergroups and channels: the full info (members, gifts).
                        load_full(&mut sync, peer, shown_dialogs, &mut folders, &tx).await?;
                    }
                    Command::OpenUsername { username, post } => {
                        match sync.resolve_username(&username).await {
                            Ok(Some(peer)) => {
                                let _ = tx.send(Update::Resolved { peer, msg_id: post });
                            }
                            Ok(None) => {
                                // tdesktop says it in a box (`lng_username_not_found`).
                                let _ = tx.send(Update::Toast(format!(
                                    "Username @{username} not found."
                                )));
                            }
                            Err(e) => error!("resolve: {e}"),
                        }
                    }
                    Command::ResolvePhone(phone) => match sync.resolve_phone(&phone).await {
                        Ok(found) => {
                            let user = found.map(|peer| {
                                (peer, peer_name(sync.store(), peer), author_of(sync.store(), peer))
                            });
                            let _ = tx.send(Update::PhoneResolved { phone, user });
                        }
                        Err(e) => error!("resolve phone: {e}"),
                    },
                    Command::BankCard(number) => {
                        let data = match sync.bank_card(&number).await {
                            Ok(data) => data,
                            Err(e) => {
                                error!("bank card: {e}");
                                None
                            }
                        };
                        let _ = tx.send(Update::BankCard { number, data });
                    }
                    Command::ToggleReaction { peer, msg_id, key } => {
                        match sync.toggle_reaction(peer, msg_id, &key).await {
                            Ok(_) => {
                                if open == Some(peer) {
                                    send_history(sync.store(), peer, &shown, None, &tx)?;
                                }
                            }
                            Err(e) => {
                                error!("reaction: {e}");
                                if open == Some(peer) {
                                    send_history(sync.store(), peer, &shown, None, &tx)?;
                                }
                            }
                        }
                    }
                    Command::Send {
                        peer,
                        text,
                        entities,
                        reply_to,
                    } => {
                        match sync.send_text(peer, &text, &entities, reply_to).await {
                            Ok(applied) => {
                                absorb_into_thread(&mut shown, peer, &applied.new_messages);
                                send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                                if open == Some(peer) {
                                    send_history(sync.store(), peer, &shown, None, &tx)?;
                                }
                            }
                            Err(e) => {
                                error!("send: {e}");
                                let _ = tx.send(Update::Toast(format!("Not sent: {e}")));
                            }
                        }
                    }
                }
            }
            Some((peer, photo_id, bytes)) = pics_rx.recv() => {
                loading.remove(&photo_id);
                if let Some(bytes) = bytes {
                    let _ = pics.send(PicJob::Downloaded { peer, photo_id, bytes });
                }
            }
            _ = async {
                match cloud_save_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                cloud_save_at = None;
                if let Err(e) = sync.save_drafts_to_cloud().await {
                    error!("drafts: {e}");
                }
            }
            event = sync.wait_event(Duration::from_secs(5)) => {
                let applied = sync.process(event).await?;
                sync_clock(&client);
                if applied.filters_changed {
                    match sync.dialog_filters().await {
                        Ok(list) => folders.set(list),
                        Err(e) => error!("folders: {e}"),
                    }
                }
                if changed(&applied) || applied.filters_changed {
                    send_dialogs(sync.store(), shown_dialogs, &mut folders, &tx)?;
                    send_draft_changes(&applied, &tx);
                    if let Some(peer) = open {
                        absorb_into_thread(&mut shown, peer, &applied.new_messages);
                        let touched = applied.new_messages.iter().any(|m| m.peer == peer)
                            || applied.edited + applied.deleted > 0
                            || applied.marks_changed;
                        if touched {
                            send_history(sync.store(), peer, &shown, None, &tx)?;
                        }
                    }
                    save(&client);
                }
            }
        }
    }
    if logging_out {
        // The cache closes before its files go.
        drop(sync);
        drop(client);
        remove_account_files(&paths);
        let _ = tx.send(Update::LoggedOut);
        return Ok(());
    }
    save(&client);
    Ok(())
}

/// How long "Log out" waits for the server.
const LOG_OUT_TIMEOUT: Duration = Duration::from_secs(5);

/// Demo messages with formatting: (outgoing, text, entities).
/// The demo's custom emoji: ids and tdesktop's animations.
const DEMO_CUSTOM_EMOJI: [(i64, &str, &[u8]); 4] = [
    (
        9001,
        "👋",
        include_bytes!("../../../assets/tdesktop-animations/demo__hello_status.tgs"),
    ),
    (
        9002,
        "🎂",
        include_bytes!("../../../assets/tdesktop-animations/demo__cake.tgs"),
    ),
    (
        9003,
        "💎",
        include_bytes!("../../../assets/tdesktop-animations/demo__diamond.tgs"),
    ),
    (
        9004,
        "🤖",
        include_bytes!("../../../assets/tdesktop-animations/demo__robot.tgs"),
    ),
];

/// The demo's installed set: its custom emoji.
fn demo_emoji_sets() -> Update {
    let emoji = DEMO_CUSTOM_EMOJI
        .iter()
        .filter_map(|(id, _, _)| match demo_custom_emoji(*id) {
            Some(Update::CustomEmoji { doc, .. }) => Some(doc),
            _ => None,
        })
        .collect::<Vec<_>>();
    let info = |id: i64, title: &str, installed: bool| tdr_data::emoji_sets::SetInfo {
        id,
        access_hash: 0,
        title: title.into(),
        short_name: title.replace(' ', ""),
        count: emoji.len() as i32,
        hash: 0,
        emojis: true,
        archived: false,
        official: false,
        installed,
        text_color: false,
        creator: false,
        thumb_document_id: None,
    };
    let mut stress: Vec<_> = (0..demo_stress_count())
        .filter_map(|k| match demo_custom_emoji(DEMO_STRESS_ID + k) {
            Some(Update::CustomEmoji { doc, .. }) => Some(doc),
            _ => None,
        })
        .collect();
    stress.extend(
        demo_real_ids()
            .into_iter()
            .filter_map(|id| match demo_real_doc(id, false) {
                Some(Update::CustomEmoji { doc, .. }) => Some(doc),
                _ => None,
            }),
    );
    let mut installed = vec![EmojiSet {
        info: info(1, "tdesktop animations @tdesktop", true),
        emoji: emoji.clone(),
    }];
    if !stress.is_empty() {
        installed.push(EmojiSet {
            info: tdr_data::emoji_sets::SetInfo {
                count: stress.len() as i32,
                ..info(3, "Stress", true)
            },
            emoji: stress,
        });
    }
    Update::EmojiSets {
        installed,
        featured: vec![EmojiSet {
            info: info(2, "Featured demo", false),
            emoji,
        }],
    }
}

/// The first id of the stress set (`TDR_DEMO_EMOJI=N`: N more custom emoji
/// for memory and speed checks, the demo animations under other ids).
const DEMO_STRESS_ID: i64 = 20_000;

fn demo_stress_count() -> i64 {
    std::env::var("TDR_DEMO_EMOJI")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// `TDR_DEMO_EMOJI=real`: the custom emoji in the file cache (public sets'
/// animations, read only) as the stress set, for checks with real animations.
fn demo_real_cache() -> Option<&'static std::sync::Mutex<FileCache>> {
    static CACHE: std::sync::OnceLock<Option<std::sync::Mutex<FileCache>>> =
        std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            (std::env::var("TDR_DEMO_EMOJI").as_deref() == Ok("real"))
                .then(|| FileCache::open_read_only(crate::data_dir().join("files")).ok())
                .flatten()
                .map(std::sync::Mutex::new)
        })
        .as_ref()
}

fn demo_real_doc(id: i64, with_file: bool) -> Option<Update> {
    let mut cache = demo_real_cache()?.lock().unwrap();
    let blob = cache.get(&format!("emoji-doc:{id}")).ok()??;
    let doc = tdr_data::emoji_sets::decode_document(&blob)?;
    let bytes = if with_file {
        cache.get(&format!("emoji-file:{id}")).ok()??
    } else {
        Vec::new()
    };
    Some(Update::CustomEmoji { doc, bytes })
}

fn demo_real_ids() -> Vec<i64> {
    let Some(cache) = demo_real_cache() else {
        return Vec::new();
    };
    let keys = cache
        .lock()
        .unwrap()
        .keys("emoji-file:", 5000)
        .unwrap_or_default();
    keys.iter()
        .filter_map(|k| k.strip_prefix("emoji-file:")?.parse().ok())
        .collect()
}

fn demo_custom_emoji(id: i64) -> Option<Update> {
    if let Some(update) = demo_real_doc(id, true) {
        return Some(update);
    }
    let (_, alt, bytes) = if (DEMO_STRESS_ID..DEMO_STRESS_ID + demo_stress_count()).contains(&id) {
        &DEMO_CUSTOM_EMOJI[((id - DEMO_STRESS_ID) % DEMO_CUSTOM_EMOJI.len() as i64) as usize]
    } else {
        DEMO_CUSTOM_EMOJI.iter().find(|e| e.0 == id)?
    };
    Some(Update::CustomEmoji {
        doc: tdr_data::emoji_sets::EmojiDocument {
            id,
            access_hash: 0,
            file_reference: Vec::new(),
            dc_id: 0,
            size: bytes.len() as i64,
            format: tdr_data::emoji_sets::EmojiFormat::Tgs,
            alt: (*alt).into(),
            set: Some((1, 0)),
            text_color: false,
            free: false,
            sticker: false,
            path: None,
        },
        bytes: bytes.to_vec(),
    })
}

fn demo_formatted(date: i32) -> Vec<(bool, String, Vec<TextEntity>)> {
    use TextKind as K;
    let build = |parts: &[(&str, &[K])]| {
        let mut text = String::new();
        let mut entities = Vec::new();
        for (part, kinds) in parts {
            let start = text.len();
            text.push_str(part);
            for kind in kinds.iter() {
                entities.push(TextEntity {
                    range: start..text.len(),
                    kind: kind.clone(),
                });
            }
        }
        (text, entities)
    };
    let link = K::Url("https://telegram.org".into());
    let (a, ae) = build(&[
        ("Underlined line", &[K::Underline]),
        ("\n", &[]),
        ("Bold italic struck", &[K::Bold, K::Italic, K::Strike]),
        (" then bold", &[K::Bold]),
        ("\n", &[]),
        ("x", &[K::Spoiler]),
        (" text with ", &[]),
        ("an underlined part", &[K::Underline]),
        ("\nplain line\n", &[]),
        ("A quoted line", &[K::Blockquote]),
        ("\n", &[]),
        ("Bold line", &[K::Bold]),
        ("\ncode ", &[]),
        ("abc", &[K::Spoiler]),
        ("\n", &[]),
        ("hidden link", std::slice::from_ref(&link)),
        ("\n", &[]),
        ("a whole hidden line", &[K::Spoiler]),
        ("\n", &[]),
        ("a marked date", &[K::Date(date)]),
    ]);
    let (b, be) = build(&[
        ("Inline ", &[]),
        ("code", &[K::Code]),
        (", a mention ", &[]),
        ("@durov", &[K::Mention]),
        (", a link ", &[]),
        ("https://telegram.org", std::slice::from_ref(&link)),
        (" and a block:\n", &[]),
        (
            "fn main() {\n    println!(\"Hello\");\n}",
            &[K::Pre("rust".into())],
        ),
        ("\nAnd one without a language:\n", &[]),
        ("line one\nline two", &[K::Pre(String::new())]),
        ("\nCall ", &[]),
        ("+1 555 0100", &[K::Phone]),
        (", pay to ", &[]),
        ("4242 4242 4242 4242", &[K::BankCard]),
        (", write to ", &[]),
        ("hello@example.com", &[K::Email]),
        ("\n", &[]),
        ("one-line code", &[K::Code]),
        ("\n", &[]),
        ("another code line", &[K::Code]),
    ]);
    let mut be = be;
    add_syntax(&b, &mut be);
    // Custom emoji (`--demo` answers them with tdesktop's own animations).
    let (c, ce) = build(&[
        ("Custom emoji: ", &[]),
        ("👋", &[K::CustomEmoji(DEMO_CUSTOM_EMOJI[0].0)]),
        (" hello ", &[]),
        ("🎂", &[K::CustomEmoji(DEMO_CUSTOM_EMOJI[1].0)]),
        ("💎", &[K::CustomEmoji(DEMO_CUSTOM_EMOJI[2].0)]),
        (" and ", &[K::Bold]),
        ("🤖", &[K::CustomEmoji(DEMO_CUSTOM_EMOJI[3].0)]),
        (" next to 😀 a regular one", &[]),
    ]);
    vec![(false, a, ae), (true, b, be), (false, c, ce)]
}

/// The demo's reactions: some messages of Alice's chat (userpics), the groups
/// (counts, a custom emoji) and the channel.
fn demo_reactions(chat: i64, m: &MessageItem) -> Option<MessageReactions> {
    use describe::ReactionKey as K;
    if m.service.is_some() || chat == 3 {
        return None;
    }
    let private = chat == 1;
    let button = |key: K, count: i32, chosen: bool, pics: &[bool]| ReactionButton {
        key,
        count,
        chosen,
        userpics: private.then(|| pics.iter().map(|out| demo_author(*out)).collect()),
    };
    match m.id % 4 {
        1 => Some(MessageReactions {
            buttons: vec![
                button(K::Emoji("❤".into()), 2, true, &[false, true]),
                button(K::Emoji("👍".into()), 1, false, &[false]),
            ],
            tags: false,
        }),
        3 if !private => Some(MessageReactions {
            buttons: vec![
                button(K::Emoji("🔥".into()), 12, false, &[]),
                button(K::Custom(DEMO_CUSTOM_EMOJI[1].0), 5, true, &[]),
                button(K::Emoji("😂".into()), 3, false, &[]),
                button(K::Emoji("🎉".into()), 1_234, false, &[]),
            ],
            tags: false,
        }),
        _ => None,
    }
}

/// `toggleReaction` on the demo's list (no Premium: one of ours).
fn demo_toggle(
    reactions: Option<MessageReactions>,
    key: &describe::ReactionKey,
    private: bool,
) -> Option<MessageReactions> {
    let mut r = reactions.unwrap_or_default();
    let was = r.buttons.iter().any(|b| b.chosen && b.key == *key);
    for b in &mut r.buttons {
        if b.chosen {
            b.chosen = false;
            b.count -= 1;
            if let Some(pics) = &mut b.userpics {
                pics.retain(|a| a.key != demo_author(true).key);
            }
        }
    }
    if !was {
        match r.buttons.iter_mut().find(|b| b.key == *key) {
            Some(b) => {
                b.chosen = true;
                b.count += 1;
                if let Some(pics) = &mut b.userpics {
                    pics.push(demo_author(true));
                }
            }
            None => r.buttons.push(ReactionButton {
                key: key.clone(),
                count: 1,
                chosen: true,
                userpics: private.then(|| vec![demo_author(true)]),
            }),
        }
    }
    r.buttons.retain(|b| b.count > 0);
    (!r.buttons.is_empty()).then_some(r)
}

fn demo_author(out: bool) -> Author {
    if out {
        Author {
            key: Some(PeerKey::user(103)),
            initials: "ME".into(),
            color: color_index(103),
            photo: None,
        }
    } else {
        Author {
            key: Some(PeerKey::user(2)),
            initials: "BO".into(),
            color: color_index(2),
            photo: None,
        }
    }
}

/// Fake chats for screenshots and UI work without an account (`--demo`).
/// Fake chats for checks and screenshots. `account` (0-based) picks which demo account
/// this is (the accounts list of `--accounts`).
pub fn start_demo(
    account: usize,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<Update>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (up_tx, up_rx) = mpsc::unbounded_channel();
    // Reactions changed by clicks: (chat, message) to what it shows now.
    let mut demo_reacted: HashMap<(i64, i32), Option<MessageReactions>> = HashMap::new();
    let now = Local::now();
    let today = move |h: u32, m: u32| {
        now.date_naive()
            .and_hms_opt(h, m, 0)
            .and_then(|t| t.and_local_timezone(Local).single())
            .map_or(0, |t| t.timestamp() as i32)
    };
    let day = 86_400;
    let line = |prefix: Option<&str>, accent: Option<&str>, text: &str| LastLine {
        prefix: prefix.map(Into::into),
        accent: accent.map(Into::into),
        text: text.into(),
        spoilers: Vec::new(),
        custom: Vec::new(),
    };
    // name, kind, last line, unread, muted, pinned, our last message read
    let rows = [
        (
            "Alice Johnson",
            ChatKind::User,
            line(None, None, "See you tomorrow at 10!"),
            2,
            false,
            true,
            None,
            today(9, 0),
        ),
        (
            "Rust Developers",
            ChatKind::Group,
            line(Some("Bob"), None, "the new borrow checker is great"),
            14,
            false,
            true,
            None,
            today(8, 7),
        ),
        (
            "Saved Messages",
            ChatKind::User,
            line(None, Some("Photo"), ""),
            0,
            false,
            true,
            None,
            today(7, 14) - day,
        ),
        (
            "Telegram Tips",
            ChatKind::Channel,
            line(None, None, "Pin your important chats"),
            554,
            true,
            false,
            None,
            today(6, 21) - day,
        ),
        (
            "Mom",
            ChatKind::User,
            line(None, None, "Call me when you can ❤"),
            0,
            false,
            false,
            Some(true),
            today(5, 28) - 2 * day,
        ),
        (
            "Weekend Hiking",
            ChatKind::Group,
            line(Some("Carol"), Some("Voice message"), ""),
            37,
            true,
            false,
            None,
            today(4, 35) - 3 * day,
        ),
        (
            "Daniel",
            ChatKind::User,
            line(None, None, "ok"),
            0,
            false,
            false,
            Some(false),
            today(3, 42) - 4 * day,
        ),
        (
            "Design Team",
            ChatKind::Group,
            line(None, Some("Eve joined the group"), ""),
            3,
            false,
            false,
            None,
            today(2, 49) - 10 * day,
        ),
        (
            "Smart Help",
            ChatKind::Bot,
            line(None, None, "Hello! How can I help?"),
            0,
            false,
            false,
            None,
            today(1, 0) - 20 * day,
        ),
    ];
    let dialogs: Vec<DialogItem> = rows
        .iter()
        .enumerate()
        .map(
            |(i, (name, kind, last, unread, muted, pinned, read, date))| DialogItem {
                peer: PeerKey::user(i as i64 + 1),
                name: name.to_string(),
                initials: initials(name),
                color: color_index(i as i64 + 1),
                photo: None,
                kind: *kind,
                is_saved: *name == "Saved Messages",
                // The demo shows every badge (an emoji status of a demo
                // custom emoji).
                badge: match *name {
                    "Telegram Tips" => PeerBadge::Verified,
                    "Alice Johnson" => PeerBadge::Premium,
                    "Mom" => PeerBadge::Status {
                        id: DEMO_CUSTOM_EMOJI[1].0,
                        verified: false,
                    },
                    _ => PeerBadge::None,
                },
                last: last.clone(),
                last_read: *read,
                time: present::dialogs_date(*date, now),
                unread: *unread,
                muted: *muted,
                pinned: *pinned,
                draft: None,
                // The last few messages of each chat are unread.
                read_inbox_max_id: 1005,
                // Alice is online, for the userpic dot.
                lastseen: (i == 0).then_some(describe::Lastseen::Online { until: i32::MAX }),
                // The channel can't be posted to: "Mute" / "Unmute" with both buttons.
                compose: if *kind == ChatKind::Channel {
                    Compose::MuteUnmute {
                        direct: true,
                        gift: true,
                    }
                } else {
                    Compose::Field
                },
                mute_until: None,
                rights: ChatRights {
                    forum: *name == "Design Team",
                    ..ChatRights::default()
                },
                status: match kind {
                    ChatKind::Group => "906 members".into(),
                    ChatKind::Channel => "8 subscribers".into(),
                    ChatKind::Bot => "bot".into(),
                    ChatKind::User => "last seen recently".into(),
                },
            },
        )
        .collect();
    // Older quiet chats below, so the list scrolls (scrollbar, "scroll to top").
    let fillers = [
        "Anna", "Boris", "Clara", "Dmitry", "Elena", "Felix", "Gleb", "Hanna",
    ];
    let mut dialogs = dialogs;
    for k in 0..30 {
        let id = rows.len() as i64 + 1 + k;
        let name = format!("{} {}", fillers[k as usize % fillers.len()], k + 1);
        dialogs.push(DialogItem {
            peer: PeerKey::user(id),
            initials: initials(&name),
            name,
            color: color_index(id),
            photo: None,
            kind: ChatKind::User,
            badge: PeerBadge::None,
            is_saved: false,
            last: line(None, None, "ok"),
            last_read: None,
            time: present::dialogs_date(today(12, 0) - (30 + k as i32) * day, now),
            unread: if k == 3 { 1_234_567 } else { 0 },
            muted: false,
            pinned: false,
            draft: None,
            read_inbox_max_id: 1005,
            status: "last seen a long time ago".into(),
            lastseen: None,
            compose: Compose::Field,
            mute_until: None,
            rights: ChatRights::default(),
        });
    }
    let _ = up_tx.send(Update::Dialogs {
        folder: 0,
        list: dialogs.clone(),
    });
    // The archive: old chats, two of them unread.
    let archived: Vec<DialogItem> = [
        "Old Project",
        "Crypto News",
        "School Friends",
        "Flat Hunting",
        "Book Club",
        "Gym Buddies",
        "Travel 2024",
        "Neighbours",
        "Car Club",
        "Photo Tips",
    ]
    .iter()
    .enumerate()
    .map(|(k, name)| {
        let id = 500 + k as i64;
        DialogItem {
            peer: PeerKey::user(id),
            initials: initials(name),
            name: name.to_string(),
            color: color_index(id),
            photo: None,
            kind: if k % 3 == 1 {
                ChatKind::Channel
            } else if k % 3 == 2 {
                ChatKind::Group
            } else {
                ChatKind::User
            },
            badge: PeerBadge::None,
            is_saved: false,
            last: line(None, None, "Archived a while ago"),
            last_read: None,
            time: present::dialogs_date(today(9, 0) - (2 + k as i32) * day, now),
            unread: if k == 1 || k == 4 { 3 } else { 0 },
            muted: true,
            pinned: false,
            draft: None,
            read_inbox_max_id: 1005,
            status: "last seen a long time ago".into(),
            lastseen: None,
            compose: Compose::Field,
            mute_until: Some(i32::MAX),
            rights: ChatRights::default(),
        }
    })
    .collect();
    let mut archive_in_menu = false;
    let _ = up_tx.send(Update::Archive(Some(demo_archive_row(
        &archived,
        archive_in_menu,
    ))));
    let mut demo_folder = 0;
    // `--folders`: the folder column with a few folders.
    let demo_folders = std::env::args().any(|a| a == "--folders");
    let folder_of = |id: i32, d: &DialogItem| match id {
        1 => matches!(d.kind, ChatKind::User | ChatKind::Bot),
        2 => d.kind == ChatKind::Group,
        3 => d.kind == ChatKind::Channel,
        4 => d.unread > 0,
        5 => matches!(d.peer.id, 2 | 8),
        6 => d.kind == ChatKind::Group,
        _ => true,
    };
    let folder_items = move |list: &[DialogItem]| -> Vec<FolderItem> {
        let folders = [
            (0, "All chats", FilterIcon::All),
            (1, "Personal", FilterIcon::Private),
            (2, "Groups", FilterIcon::Groups),
            (3, "Channels", FilterIcon::Channels),
            (4, "Unread", FilterIcon::Unread),
            (5, "Work", FilterIcon::Work),
            // A long title wraps (up to three lines).
            (6, "Rust and open source projects", FilterIcon::Study),
        ];
        folders
            .iter()
            .map(|(id, title, icon)| {
                let unread: Vec<&DialogItem> = list
                    .iter()
                    .filter(|d| d.unread > 0 && folder_of(*id, d))
                    .collect();
                FolderItem {
                    id: *id,
                    title: title.to_string(),
                    icon: *icon,
                    badge: (!unread.is_empty())
                        .then(|| (unread.len() as u32, unread.iter().all(|d| d.muted))),
                }
            })
            .collect()
    };
    if demo_folders {
        let _ = up_tx.send(Update::Folders(folder_items(&dialogs)));
    }
    // The demo account's own user and counters.
    let names = ["Alex Morgan", "Jamie Lee", "Sam Taylor"];
    let mut me = MeInfo {
        peer: PeerKey::user(103 + account as i64),
        name: names[account % names.len()].into(),
        initials: initials(names[account % names.len()]),
        color: color_index(103 + account as i64),
        photo: None,
        premium: false,
        phone: Some(
            ["+1 555 010 0001", "+44 7700 900123", "+49 151 2345 6789"][account % 3].into(),
        ),
        username: (account != 1).then(|| ["alex_morgan", "", "sam_taylor"][account % 3].into()),
        phone_hidden: false,
    };
    let state = match account {
        0 => AccountState {
            unread: dialogs.iter().map(|d| d.unread.max(0) as u32).sum(),
            unread_muted: false,
            archive_chats: 130,
            archive_unread: 39,
        },
        1 => AccountState {
            unread: 303_412,
            unread_muted: false,
            ..Default::default()
        },
        _ => AccountState {
            unread: 55_804,
            unread_muted: true,
            ..Default::default()
        },
    };
    let _ = up_tx.send(Update::Me(me.clone()));
    let _ = up_tx.send(Update::AccountState(state));
    // `--connecting`: the "Connecting..." box, as if the connection were lost.
    let _ = up_tx.send(Update::Connected(
        !std::env::args().any(|a| a == "--connecting"),
    ));
    std::thread::spawn(move || {
        let text =
            |out: bool, text: &str| (out, Some(text.to_string()), None::<&str>, None::<&str>);
        let entries = [
            (false, None, None, Some("Bob Smith joined the group")),
            text(false, "Hey there"),
            text(false, "Hi! Did you try the new client?"),
            text(
                true,
                "Yes — the chat list opens from the cache in about 5 ms.",
            ),
            (false, None, Some("Photo"), None),
            text(false, "That's fast. And the colors?"),
            text(
                true,
                "Taken straight from tdesktop's palette, day and night.",
            ),
            text(
                false,
                "https://example.com/very/long/link/without/any/spaces?utm_source=demo&utm_medium=social&utm_content=link_in_bio&fbclid=PAZnRzaAVi_zNleHRuA2FlbQIxMQBwZG9mAmZkaWQWUSFgPoFLDSR14gI_z30rz-irR2s_T3NydGMGYXBwX2lkDzEyNDAyNDU3NDI4NzQxNAABp",
            ),
            text(
                true,
                "A long paragraph to check wrapping: Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.",
            ),
            (false, None, Some("Voice message"), None),
            text(
                false,
                "0101001101001101010110000100101101001001010011100101001101010000010000010100001101000101010100001001001100010101010100111001010100",
            ),
            text(false, "See you tomorrow at 10!"),
        ];
        let count = entries.len() as i32;
        // Two days ago, yesterday, today: date dividers in between.
        let start = today(10, 0) - 2 * day;
        let mut history: Vec<MessageItem> = entries
            .into_iter()
            .enumerate()
            .map(|(i, (out, body, media, service))| {
                let date = start + (i as i32 * 2 * day) / count;
                MessageItem {
                    id: 1000 + i as i32,
                    out,
                    text: body.unwrap_or_default(),
                    media: media.map(Into::into),
                    service: service.map(Into::into),
                    edited: i == 5,
                    read: out.then_some(i < 6),
                    time: present::message_time(date),
                    date,
                    sender: None,
                    author: demo_author(out),
                    forwarded: None,
                    original: None,
                    entities: Vec::new(),
                    forward_date: None,
                    edit_date: None,
                    post: None,
                    reply: None,
                    thread_root: None,
                    reactions: None,
                }
            })
            .collect();
        // Formatting: a message like the user's test one, then code and links.
        for (k, (out, text, entities)) in demo_formatted(today(10, 0)).into_iter().enumerate() {
            let date = today(10, 0) + k as i32 * 60;
            history.push(MessageItem {
                id: 1000 + count + k as i32,
                out,
                text,
                media: None,
                service: None,
                edited: false,
                read: out.then_some(true),
                time: present::message_time(date),
                date,
                sender: None,
                author: demo_author(out),
                forwarded: None,
                original: None,
                entities,
                forward_date: None,
                edit_date: None,
                post: None,
                reply: None,
                thread_root: None,
                reactions: None,
            });
        }
        // Older demo messages appear 20 at a time when the user scrolls up.
        let mut older_left = 60;
        let mut next_id = 2000;
        // A "go to original" target, sent once with the next histories.
        let mut focus: Option<(PeerKey, i32)> = None;
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                Command::Refresh => {
                    demo_folder = 0;
                    let _ = up_tx.send(Update::Archive(Some(demo_archive_row(
                        &archived,
                        archive_in_menu,
                    ))));
                    let _ = up_tx.send(Update::Dialogs {
                        folder: 0,
                        list: dialogs.clone(),
                    });
                    if demo_folders {
                        let _ = up_tx.send(Update::Folders(folder_items(&dialogs)));
                    }
                    let _ = up_tx.send(Update::Me(me.clone()));
                    let _ = up_tx.send(Update::AccountState(state));
                    let _ = up_tx.send(Update::Connected(true));
                }
                Command::SelectFolder(id) => {
                    demo_folder = id;
                    let shown: Vec<DialogItem> = dialogs
                        .iter()
                        .filter(|d| folder_of(id, d))
                        .cloned()
                        .collect();
                    // The demo answers late, as a server would: the window shows the
                    // folder at once, from what it had.
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    let _ = up_tx.send(Update::Dialogs {
                        folder: id,
                        list: shown,
                    });
                }
                Command::ReorderPinned(order) => {
                    let rank = |d: &DialogItem| order.iter().position(|p| *p == d.peer);
                    let pinned = dialogs.iter().take_while(|d| d.pinned).count();
                    dialogs[..pinned].sort_by_key(|d| rank(d).unwrap_or(usize::MAX));
                    let _ = up_tx.send(Update::Dialogs {
                        folder: demo_folder,
                        list: dialogs.clone(),
                    });
                    continue;
                }
                Command::LoadMoreDialogs
                | Command::LoadNewer(_)
                | Command::Back
                | Command::ReopenChat(_) => continue,
                Command::OpenChatAt { peer, msg_id } => focus = msg_id.map(|m| (peer, m)),
                Command::OpenUsername { username, .. } => {
                    let _ = up_tx.send(Update::Toast(format!("Username @{username} not found.")));
                    continue;
                }
                // The demo has no server: a number nobody has, a known card.
                Command::ResolvePhone(phone) => {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    let _ = up_tx.send(Update::PhoneResolved { phone, user: None });
                    continue;
                }
                Command::BankCard(number) => {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    let _ = up_tx.send(Update::BankCard {
                        number,
                        data: Some(tdr_data::BankCard {
                            title: "Demo Bank, Visa".into(),
                            links: vec![("Open in Demo Bank".into(), "https://example.com".into())],
                        }),
                    });
                    continue;
                }
                Command::LogOut => {
                    let _ = up_tx.send(Update::LoggedOut);
                    break;
                }
                Command::OpenArchive => {
                    // Late, as a server would answer.
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = up_tx.send(Update::Dialogs {
                        folder: ARCHIVE_LIST,
                        list: archived.clone(),
                    });
                    continue;
                }
                Command::CloseArchive => continue,
                Command::Join(_) => continue,
                Command::OpenForum(peer) => {
                    let topic =
                        |id: i32, title: &str, color: i32, text: &str, unread: i32| TopicItem {
                            id,
                            title: title.into(),
                            icon_color: color,
                            general: id == 1,
                            last: line(Some("Bob"), None, text),
                            time: present::dialogs_date(today(9, 0) - id * 3600, Local::now()),
                            unread,
                            muted: false,
                            pinned: id == 2,
                            closed: false,
                        };
                    let topics = vec![
                        topic(2, "Chat rules", 0xFB6F5F, "Be nice to each other", 0),
                        topic(1, "General", 0, "Hi everyone!", 12),
                        topic(7, "Mockups", 0x6FB9F0, "New screens are up", 3),
                        topic(9, "Releases", 0x8EEE98, "v0.2 is out", 0),
                        topic(11, "Ideas", 0xFFD67E, "What about dark icons?", 0),
                    ];
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let _ = up_tx.send(Update::Topics { peer, topics });
                    continue;
                }
                Command::OpenThread { peer, root } => {
                    let mut messages: Vec<MessageItem> =
                        history.iter().rev().take(6).rev().cloned().collect();
                    for m in &mut messages {
                        m.sender = (!m.out).then(|| ("Bob Smith".to_string(), m.author.color));
                    }
                    let _ = up_tx.send(Update::ThreadCount {
                        peer,
                        root,
                        count: 7,
                    });
                    let _ = up_tx.send(Update::History {
                        peer,
                        messages,
                        complete: true,
                        newer: false,
                        focus: None,
                        thread: Some(root),
                    });
                    continue;
                }
                Command::OpenComments { channel, post } => {
                    let group = PeerKey::user(2);
                    let root = 5000;
                    let original = history.iter().find(|m| m.id == post).cloned();
                    let mut messages = Vec::new();
                    if let Some(mut m) = original {
                        m.id = root;
                        m.out = false;
                        m.post = None;
                        m.sender = None;
                        m.forwarded = Some(ForwardLine {
                            name: "Telegram Tips".into(),
                            author: None,
                        });
                        messages.push(m.clone());
                        messages.push(MessageItem {
                            id: DISCUSSION_STARTED_ID,
                            text: String::new(),
                            service: Some("Discussion started".into()),
                            forwarded: None,
                            reply: None,
                            ..m
                        });
                    }
                    for (k, text) in ["Great post!", "Thanks for the tips", "Agreed with Bob"]
                        .iter()
                        .enumerate()
                    {
                        let out = k == 2;
                        let date = start + 2 * day + k as i32 * 600;
                        messages.push(MessageItem {
                            id: root + 1 + k as i32,
                            out,
                            text: text.to_string(),
                            media: None,
                            service: None,
                            edited: false,
                            read: out.then_some(true),
                            time: present::message_time(date),
                            date,
                            sender: (!out).then(|| ("Bob Smith".to_string(), color_index(2))),
                            author: demo_author(out),
                            forwarded: None,
                            original: None,
                            entities: Vec::new(),
                            forward_date: None,
                            edit_date: None,
                            post: None,
                            reply: (k == 2).then(|| ReplyLine {
                                peer: group,
                                msg_id: root + 1,
                                name: "Bob Smith".into(),
                                color: Some(color_index(2)),
                                text: "Great post!".into(),
                                quote: false,
                            }),
                            thread_root: Some(root),
                            reactions: None,
                        });
                    }
                    let _ = channel;
                    let _ = up_tx.send(Update::ThreadCount {
                        peer: group,
                        root,
                        count: 3,
                    });
                    let _ = up_tx.send(Update::History {
                        peer: group,
                        messages,
                        complete: true,
                        newer: false,
                        focus: None,
                        thread: Some(root),
                    });
                    continue;
                }
                Command::Leave(peer) => {
                    dialogs.retain(|d| d.peer != peer);
                    let _ = up_tx.send(Update::Dialogs {
                        folder: demo_folder,
                        list: dialogs
                            .iter()
                            .filter(|d| folder_of(demo_folder, d))
                            .cloned()
                            .collect(),
                    });
                    continue;
                }
                Command::SetPhoneHidden(hidden) => {
                    me.phone_hidden = hidden;
                    let _ = up_tx.send(Update::Me(me.clone()));
                    continue;
                }
                Command::ArchiveInMenu(in_menu) => {
                    archive_in_menu = in_menu;
                    let _ = up_tx.send(Update::Archive(Some(demo_archive_row(
                        &archived,
                        archive_in_menu,
                    ))));
                    continue;
                }
                Command::SetMuted { peer, mute } => {
                    if let Some(d) = dialogs.iter_mut().find(|d| d.peer == peer) {
                        d.muted = mute;
                    }
                    let _ = up_tx.send(Update::Dialogs {
                        folder: demo_folder,
                        list: dialogs
                            .iter()
                            .filter(|d| folder_of(demo_folder, d))
                            .cloned()
                            .collect(),
                    });
                    continue;
                }
                Command::OpenChat(_)
                | Command::CloseChat
                | Command::SaveDraft { .. }
                | Command::Read { .. }
                | Command::LoadUserpic { .. }
                | Command::FetchCustomEmoji(_)
                | Command::RefreshEmojiKeywords { .. }
                | Command::ToggleEmojiSet { .. } => {}
                Command::LoadEmojiSet(set) => {
                    let found = match demo_emoji_sets() {
                        Update::EmojiSets {
                            installed,
                            featured,
                        } => installed.into_iter().chain(featured).find(|s| match &set {
                            tdr_data::emoji_sets::SetRef::Id(id, _) => s.info.id == *id,
                            tdr_data::emoji_sets::SetRef::ShortName(n) => {
                                s.info.short_name.eq_ignore_ascii_case(n)
                            }
                        }),
                        _ => None,
                    };
                    let _ = up_tx.send(Update::EmojiSetLoaded { set, found });
                }
                Command::LoadEmojiSets => {
                    let _ = up_tx.send(demo_emoji_sets());
                }
                Command::LoadReactions => {
                    let list: Vec<tdr_data::reactions::Available> = [
                        "👍",
                        "👎",
                        "❤",
                        "🔥",
                        "🥰",
                        "👏",
                        "😁",
                        "🤔",
                        "🤯",
                        "😱",
                        "🤬",
                        "😢",
                        "🎉",
                        "🤩",
                        "🤮",
                        "💩",
                        "🙏",
                        "👌",
                        "🕊",
                        "🤡",
                        "🥱",
                        "🥴",
                        "😍",
                        "🐳",
                        "❤\u{200d}🔥",
                        "🌚",
                        "🌭",
                        "💯",
                        "🤣",
                        "⚡",
                        "🍌",
                        "🏆",
                        "💔",
                        "🤨",
                        "😐",
                        "🍓",
                        "🍾",
                        "💋",
                        "🖕",
                        "😈",
                        "😴",
                        "😭",
                        "🤓",
                        "👻",
                        "👨\u{200d}💻",
                        "👀",
                        "🎃",
                        "🙈",
                        "😇",
                        "😨",
                        "🤝",
                        "✍",
                        "🤗",
                        "🫡",
                        "🎅",
                        "🎄",
                        "☃",
                        "💅",
                        "🤪",
                        "🗿",
                        "🆒",
                        "💘",
                        "🙉",
                        "🦄",
                        "😘",
                        "💊",
                        "🙊",
                        "😎",
                        "👾",
                        "🤷\u{200d}♂",
                        "🤷",
                        "🤷\u{200d}♀",
                        "😡",
                        "😂",
                        "😮",
                    ]
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        // `TDR_DEMO_REACTIONS=anim`: the first ones get the demo
                        // animations as their select, appear and effect ones.
                        let doc = |k: usize| {
                            let anim = std::env::var("TDR_DEMO_REACTIONS").as_deref() == Ok("anim");
                            match demo_custom_emoji(
                                DEMO_CUSTOM_EMOJI[k % DEMO_CUSTOM_EMOJI.len()].0,
                            ) {
                                Some(Update::CustomEmoji { doc, .. }) if anim && i < 6 => Some(doc),
                                _ => None,
                            }
                        };
                        tdr_data::reactions::Available {
                            emoticon: (*e).to_string(),
                            title: String::new(),
                            premium: false,
                            inactive: false,
                            center: None,
                            select: doc(i),
                            appear: doc(i + 1),
                            activate: None,
                            effect: None,
                            around: doc(1),
                        }
                    })
                    .collect();
                    *REACTION_ORDER.lock().unwrap() =
                        list.iter().map(|r| r.emoticon.clone()).collect();
                    let top = [
                        "👍", "❤", "🔥", "🥰", "👏", "😁", "🤔", "🤯", "😱", "🎉", "🤩",
                    ]
                    .iter()
                    .map(|e| describe::ReactionKey::Emoji((*e).to_string()))
                    .collect();
                    let _ = up_tx.send(Update::AvailableReactions { list, top });
                    continue;
                }
                Command::ToggleReaction { peer, msg_id, key } => {
                    let current = demo_reacted
                        .get(&(peer.id, msg_id))
                        .cloned()
                        .unwrap_or_else(|| {
                            history
                                .iter()
                                .find(|m| m.id == msg_id)
                                .and_then(|m| demo_reactions(peer.id, m))
                        });
                    demo_reacted
                        .insert((peer.id, msg_id), demo_toggle(current, &key, peer.id == 1));
                }
                Command::LoadEmojiGroups => {
                    let group = |title: &str, icon: usize, emoticons: &[&str]| {
                        tdr_data::emoji_sets::EmojiGroup {
                            title: title.into(),
                            icon: DEMO_CUSTOM_EMOJI[icon].0,
                            emoticons: emoticons.iter().map(|e| (*e).to_string()).collect(),
                            premium: false,
                        }
                    };
                    let _ = up_tx.send(Update::EmojiGroups(vec![
                        group("Greetings", 0, &["👋", "🤝", "🙂"]),
                        group("Celebrate", 1, &["🎉", "🥳", "🎂"]),
                        group("Wow", 2, &["😮", "🤩", "💎"]),
                        group("Robots", 3, &["🤖", "👾"]),
                        // More than the pill holds: the strip scrolls and is cut.
                        group("Hearts", 1, &["❤", "💕"]),
                        group("Party", 0, &["🎉", "🎊"]),
                        group("Gems", 2, &["💎", "💍"]),
                        group("Aliens", 3, &["👽", "👾"]),
                        group("Hands", 0, &["👍", "👏"]),
                        group("Cakes", 1, &["🎂", "🍰"]),
                    ]));
                }
                Command::LoadCustomEmoji(ids) => {
                    for id in ids {
                        let _ = up_tx
                            .send(demo_custom_emoji(id).unwrap_or(Update::CustomEmojiFailed(id)));
                    }
                }
                Command::LoadOlder(_) if older_left > 0 => {
                    let first = history.first().map_or(1000, |m| m.id);
                    let first_date = history.first().map_or(start, |m| m.date);
                    let page: Vec<MessageItem> = (1..=20)
                        .rev()
                        .map(|k| {
                            let date = first_date - k * 3 * 3600;
                            MessageItem {
                                id: first - k,
                                out: k % 3 == 0,
                                text: format!("Older message #{}", older_left - 20 + k),
                                media: None,
                                service: None,
                                edited: false,
                                read: (k % 3 == 0).then_some(true),
                                time: present::message_time(date),
                                date,
                                sender: None,
                                author: demo_author(k % 3 == 0),
                                forwarded: None,
                                original: None,
                                entities: Vec::new(),
                                forward_date: None,
                                edit_date: None,
                                post: None,
                                reply: None,
                                thread_root: None,
                                reactions: None,
                            }
                        })
                        .collect();
                    older_left -= 20;
                    history.splice(0..0, page);
                }
                Command::LoadOlder(_) => {}
                Command::Send { text, entities, .. } => {
                    next_id += 1;
                    let date = Local::now().timestamp() as i32;
                    let entities: Vec<describe::Entity> = entities
                        .iter()
                        .map(|e| {
                            use describe::EntityKind as E;
                            use tdr_data::OutKind as O;
                            describe::Entity {
                                offset: e.offset,
                                length: e.length,
                                kind: match &e.kind {
                                    O::Bold => E::Bold,
                                    O::Italic => E::Italic,
                                    O::Underline => E::Underline,
                                    O::Strike => E::Strike,
                                    O::Code => E::Code,
                                    O::Pre(language) => E::Pre {
                                        language: language.clone(),
                                    },
                                    O::Blockquote => E::Blockquote { collapsed: false },
                                    O::Spoiler => E::Spoiler,
                                    O::TextUrl(url) => E::TextUrl(url.clone()),
                                    O::CustomEmoji(id) => E::CustomEmoji(*id),
                                },
                            }
                        })
                        .collect();
                    let (text, entities) = message_text(&text, &entities);
                    history.push(MessageItem {
                        id: next_id,
                        out: true,
                        text,
                        media: None,
                        service: None,
                        edited: false,
                        read: Some(false),
                        time: present::message_time(date),
                        date,
                        sender: None,
                        author: demo_author(true),
                        forwarded: None,
                        original: None,
                        entities,
                        forward_date: None,
                        edit_date: None,
                        post: None,
                        reply: None,
                        thread_root: None,
                        reactions: None,
                    });
                }
            }
            // Replies: two of the first messages answer earlier ones.
            let n = count as usize;
            for (at, to) in [
                (n.saturating_sub(1), n.saturating_sub(4)),
                (n.saturating_sub(3), n.saturating_sub(6)),
            ] {
                if at < n && to < n && at != to && history[to].service.is_none() {
                    let target = history[to].clone();
                    let name = if target.out {
                        "Alex Morgan"
                    } else {
                        "Bob Smith"
                    };
                    history[at].reply = Some(ReplyLine {
                        peer: PeerKey::user(1),
                        msg_id: target.id,
                        name: name.into(),
                        color: Some(target.author.color),
                        text: target.text.lines().next().unwrap_or("").to_string(),
                        quote: false,
                    });
                }
            }
            for i in 0..9 {
                let mut messages = history.clone();
                // Groups (Rust Developers, Weekend Hiking, Design Team) show sender names.
                if matches!(i + 1, 2 | 6 | 8) {
                    for m in messages
                        .iter_mut()
                        .filter(|m| !m.out && m.service.is_none())
                    {
                        m.sender = Some(("Bob Smith".into(), m.author.color));
                    }
                }
                // Saved Messages: incoming ones are forwards from Alice's chat, one
                // of them saved from Bob (a forward of a forward).
                if i + 1 == 3 {
                    for (k, m) in messages
                        .iter_mut()
                        .enumerate()
                        .filter(|(_, m)| !m.out && m.service.is_none())
                    {
                        m.original = Some((PeerKey::user(1), m.id));
                        m.author = demo_author(false);
                        m.sender = Some(("Bob Smith".into(), m.author.color));
                        if k % 2 == 0 {
                            m.forwarded = Some(ForwardLine {
                                name: "Alice Johnson".into(),
                                author: Some(Author {
                                    key: Some(PeerKey::user(1)),
                                    initials: "AJ".into(),
                                    color: color_index(1),
                                    photo: None,
                                }),
                            });
                        }
                    }
                }
                // The channel (Telegram Tips): posts with views, a signature and
                // comments on some.
                if dialogs
                    .get(i as usize)
                    .is_some_and(|d| d.kind == ChatKind::Channel)
                {
                    let name = dialogs[i as usize].name.clone();
                    let color = dialogs[i as usize].color;
                    for (k, m) in messages
                        .iter_mut()
                        .enumerate()
                        .filter(|(_, m)| m.service.is_none())
                    {
                        m.out = false;
                        m.read = None;
                        m.sender = Some((name.clone(), color));
                        m.author = Author {
                            key: Some(PeerKey::user(i + 1)),
                            initials: initials(&name),
                            color,
                            photo: None,
                        };
                        m.post = Some(Post {
                            views: Some(present::count_short([12, 987, 12_345, 1_234_567][k % 4])),
                            signature: (k % 5 == 1).then(|| "Alice".to_string()),
                            comments: match k % 3 {
                                0 => Some(present::comments_text(3)),
                                1 => Some(present::comments_text(0)),
                                _ => None,
                            },
                            repliers: if k % 3 == 0 {
                                vec![demo_author(false), demo_author(true)]
                            } else {
                                Vec::new()
                            },
                            discussion: Some(PeerKey::user(2)),
                        });
                    }
                }
                for m in &mut messages {
                    m.reactions = match demo_reacted.get(&(i + 1, m.id)) {
                        Some(r) => r.clone(),
                        None => demo_reactions(i + 1, m),
                    };
                }
                let _ = up_tx.send(Update::History {
                    peer: PeerKey::user(i + 1),
                    messages,
                    complete: older_left == 0,
                    newer: false,
                    focus: focus
                        .filter(|(p, _)| *p == PeerKey::user(i + 1))
                        .map(|(_, id)| id),
                    thread: None,
                });
            }
            focus = None;
        }
    });
    (cmd_tx, up_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_like_tdesktop() {
        assert_eq!(initials("Pavel Durov"), "PD");
        assert_eq!(initials("telegram"), "T");
        assert_eq!(initials("  "), "");
        assert_eq!(initials("Анна Иванова"), "АИ");
    }
}
