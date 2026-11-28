//! Data layer: keeps the local cache in sync with the server.
//!
//! [`Sync`] owns the update [`Engine`](tdr_updates::Engine) and the [`Storage`]. On start
//! it either loads the chat list from scratch (`updates.getState` + `messages.getDialogs`)
//! or catches up from the saved state with `updates.getDifference`. Afterwards it applies
//! live updates. Every batch is written in one transaction together with the update
//! state it leads to, so after a crash the cache and the state always agree.

pub mod convert;
pub mod countries;
mod countries_fallback;
pub mod describe;
pub mod drafts;
pub mod emoji_keywords;
pub mod emoji_sets;
pub mod filters;
pub mod login;
pub mod reactions;

use std::time::{Duration, Instant};

use drafts::Drafts;
use tdr_api::{Client, Error as ApiError, Event};
use tdr_storage::{DialogRow, MessageRow, PeerKey, PeerKind, Storage, UpdateState};
use tdr_tl::{Deserializable, enums, functions, types};
use tdr_updates::{Action, Engine, Item, Output, State};
use tracing::{debug, info, warn};

pub use tdr_storage;
pub use tdr_tl;

/// Formatting of a message being sent, in UTF-16 offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutEntity {
    pub offset: i32,
    pub length: i32,
    pub kind: OutKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutKind {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Pre(String),
    Blockquote,
    Spoiler,
    TextUrl(String),
    CustomEmoji(i64),
}

fn out_entity(e: &OutEntity) -> enums::MessageEntity {
    let (offset, length) = (e.offset, e.length);
    match &e.kind {
        OutKind::Bold => types::MessageEntityBold { offset, length }.into(),
        OutKind::Italic => types::MessageEntityItalic { offset, length }.into(),
        OutKind::Underline => types::MessageEntityUnderline { offset, length }.into(),
        OutKind::Strike => types::MessageEntityStrike { offset, length }.into(),
        OutKind::Code => types::MessageEntityCode { offset, length }.into(),
        OutKind::Pre(language) => types::MessageEntityPre {
            offset,
            length,
            language: language.clone(),
        }
        .into(),
        OutKind::Blockquote => types::MessageEntityBlockquote {
            collapsed: false,
            offset,
            length,
        }
        .into(),
        OutKind::Spoiler => types::MessageEntitySpoiler { offset, length }.into(),
        OutKind::TextUrl(url) => types::MessageEntityTextUrl {
            offset,
            length,
            url: url.clone(),
        }
        .into(),
        OutKind::CustomEmoji(document_id) => types::MessageEntityCustomEmoji {
            offset,
            length,
            document_id: *document_id,
        }
        .into(),
    }
}

/// A bank card's data (`payments.bankCardData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankCard {
    pub title: String,
    /// (name, url) of places that open the card.
    pub links: Vec<(String, String)>,
}

/// Random bytes for `random_id` (the server deduplicates sends by it).
fn tdr_crypto_random() -> [u8; 8] {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("OS random number generator failed");
    b
}

#[derive(Debug)]
pub enum Error {
    Api(ApiError),
    Storage(tdr_storage::Error),
    /// Something the client does not handle yet.
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "cache: {e}"),
            Self::Unsupported(what) => write!(f, "not supported yet: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
impl From<tdr_storage::Error> for Error {
    fn from(e: tdr_storage::Error) -> Self {
        Self::Storage(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a sync step changed.
#[derive(Debug, Default)]
pub struct Applied {
    /// New messages, in arrival order.
    pub new_messages: Vec<MessageRow>,
    pub edited: usize,
    pub deleted: usize,
    /// The chat list was reloaded from scratch.
    pub reloaded: bool,
    /// Read marks or mute settings changed (check marks, badges).
    pub marks_changed: bool,
    /// Chats were pinned, unpinned or reordered.
    pub pinned_changed: bool,
    /// The folders changed (`updateDialogFilter*`): ask for them again.
    pub filters_changed: bool,
    /// Users went online or offline.
    pub statuses_changed: bool,
    /// A service notification was added (`updateServiceNotification`).
    pub service_notification: bool,
    /// Local drafts changed by the server (another device): the input field of an
    /// open chat should show the new text (`None`: cleared).
    pub draft_changes: Vec<drafts::Change>,
    /// Cloud drafts seen while applying; turned into `draft_changes` by `Sync`.
    cloud_drafts: Vec<(PeerKey, drafts::CloudDraft, bool)>,
}

impl Applied {
    fn merge(&mut self, other: Applied) {
        self.new_messages.extend(other.new_messages);
        self.edited += other.edited;
        self.deleted += other.deleted;
        self.reloaded |= other.reloaded;
        self.marks_changed |= other.marks_changed;
        self.pinned_changed |= other.pinned_changed;
        self.statuses_changed |= other.statuses_changed;
        self.service_notification |= other.service_notification;
        self.draft_changes.extend(other.draft_changes);
        self.cloud_drafts.extend(other.cloud_drafts);
    }
}

/// How many chats to load when the cache is empty.
pub const INITIAL_DIALOGS: usize = 200;
const DIALOGS_PAGE: i32 = 100;
/// A forum's topic (`forumTopic`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub id: i32,
    pub title: String,
    /// `icon_color` (`ForumTopicIcons`), and whether it has a custom emoji
    /// instead (drawn as the colored icon until custom emoji come).
    pub icon_color: i32,
    pub icon_emoji: bool,
    pub top_message: i32,
    pub unread: i32,
    pub pinned: bool,
    pub closed: bool,
    pub hidden: bool,
    pub muted: bool,
}

/// What a sent message replies to (`FullReplyTo`): the message and, in a thread
/// (a topic, a post's comments), its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutReply {
    pub msg_id: i32,
    pub top_id: Option<i32>,
}

/// The archive's folder id (`Data::Folder::kId`).
pub const ARCHIVE_FOLDER: i32 = 1;
/// `meta` keys: the archive's size on the server; the account's
/// `SessionSettings::archiveInMainMenu`.
pub const ARCHIVE_COUNT_KEY: &str = "archive_count";
pub const ARCHIVE_IN_MENU_KEY: &str = "archive_in_main_menu";
/// `SessionSettings::phoneNumberHidden`: the own number under a spoiler in Settings.
pub const PHONE_HIDDEN_KEY: &str = "phone_number_hidden";
const CHANNEL_DIFFERENCE_LIMIT: i32 = 100;

pub struct Sync {
    client: Client,
    store: Storage,
    engine: Engine,
    self_id: i64,
    drafts: Drafts,
    /// Drafts that came with the chat list, applied with the next `Applied`.
    dialog_drafts: Vec<(PeerKey, drafts::CloudDraft, bool)>,
    /// Where the next page of the server's chat list starts (`None`: at the top).
    dialogs_cursor: Option<DialogsCursor>,
    /// The server's chat list was loaded to its end in this session.
    dialogs_complete: bool,
    /// The same for the archive (folder 1).
    archive_cursor: Option<DialogsCursor>,
    archive_complete: bool,
    /// Chats loaded from the top of the server's list in this session.
    dialogs_loaded: usize,
    /// Chats the server's chat list had in this session.
    listed: std::collections::HashSet<PeerKey>,
    /// Pinned entries of the main list that are not chats we keep (the archive folder,
    /// communities), with their place among the pinned: a reorder must keep them.
    pinned_extra: Vec<(usize, enums::InputDialogPeer)>,
}

/// `messages.getDialogs` offsets: the last chat of the previous page.
#[derive(Clone)]
struct DialogsCursor {
    date: i32,
    id: i32,
    peer: enums::InputPeer,
}

fn to_engine(s: UpdateState) -> State {
    State {
        pts: s.pts,
        qts: s.qts,
        date: s.date,
        seq: s.seq,
    }
}

fn to_storage(s: State) -> UpdateState {
    UpdateState {
        pts: s.pts,
        qts: s.qts,
        date: s.date,
        seq: s.seq,
    }
}

impl Sync {
    pub fn new(client: Client, store: Storage) -> Result<Self> {
        let state = store.update_state()?.map(to_engine);
        let engine = Engine::new(state, store.channel_pts()?);
        let self_id = store.meta_i64("self_id")?.unwrap_or(0);
        let mut drafts = Drafts::new(true);
        for (peer, text, date) in store.drafts()? {
            drafts.restore_local(peer, text, date);
        }
        Ok(Self {
            client,
            store,
            engine,
            self_id,
            drafts,
            dialog_drafts: Vec::new(),
            dialogs_cursor: None,
            dialogs_complete: false,
            archive_cursor: None,
            archive_complete: false,
            dialogs_loaded: 0,
            listed: Default::default(),
            pinned_extra: Vec::new(),
        })
    }

    /// `Histories::requestDialogEntry`: a chat opened that the server's chat list has
    /// not shown in this session gets its dialog (`messages.getPeerDialogs`) and is
    /// listed by its last message. Unlike tdesktop, which forgets it on restart, it
    /// is kept in the cache: the chat exists, the list just leaves it out. Returns
    /// whether one came.
    /// `contacts.resolveUsername` (`SessionNavigation::resolveUsername`): the peer
    /// with this username, stored in the cache; `None` when nobody has it.
    pub async fn resolve_username(&mut self, username: &str) -> Result<Option<PeerKey>> {
        let request = functions::contacts::ResolveUsername {
            username: username.to_string(),
            referer: None,
        };
        let enums::contacts::ResolvedPeer::ResolvedPeer(answer) =
            match self.client.invoke(&request).await {
                Ok(answer) => answer,
                // USERNAME_NOT_OCCUPIED, USERNAME_INVALID.
                Err(ApiError::Rpc(e)) if e.code == 400 => return Ok(None),
                Err(e) => return Err(e.into()),
            };
        let rows: Vec<_> = answer
            .users
            .iter()
            .filter_map(convert::user_row)
            .chain(answer.chats.iter().filter_map(convert::chat_row))
            .collect();
        self.store.write(|w| {
            for p in &rows {
                w.put_peer(p)?;
            }
            Ok(())
        })?;
        Ok(Some(convert::peer_key(&answer.peer)))
    }

    /// `contacts.resolvePhone` (a phone number's menu): the user with this number,
    /// stored in the cache; `None` when it is not on Telegram (or hidden).
    pub async fn resolve_phone(&mut self, phone: &str) -> Result<Option<PeerKey>> {
        let request = functions::contacts::ResolvePhone {
            phone: phone.to_string(),
        };
        let enums::contacts::ResolvedPeer::ResolvedPeer(answer) =
            match self.client.invoke(&request).await {
                Ok(answer) => answer,
                Err(ApiError::Rpc(e)) if e.code == 400 => return Ok(None),
                Err(e) => return Err(e.into()),
            };
        let rows: Vec<_> = answer
            .users
            .iter()
            .filter_map(convert::user_row)
            .chain(answer.chats.iter().filter_map(convert::chat_row))
            .collect();
        self.store.write(|w| {
            for p in &rows {
                w.put_peer(p)?;
            }
            Ok(())
        })?;
        Ok(Some(convert::peer_key(&answer.peer)))
    }

    /// `payments.getBankCardData` (a card number's menu): the bank's name and
    /// links to open the card in, `None` when the server does not know it.
    pub async fn bank_card(&mut self, number: &str) -> Result<Option<BankCard>> {
        let request = functions::payments::GetBankCardData {
            number: number.to_string(),
        };
        let enums::payments::BankCardData::BankCardData(data) =
            match self.client.invoke(&request).await {
                Ok(answer) => answer,
                Err(ApiError::Rpc(e)) if e.code == 400 => return Ok(None),
                Err(e) => return Err(e.into()),
            };
        Ok(Some(BankCard {
            title: data.title,
            links: data
                .open_urls
                .into_iter()
                .map(|u| {
                    let enums::BankCardOpenUrl::BankCardOpenUrl(u) = u;
                    (u.name, u.url)
                })
                .collect(),
        }))
    }

    /// Kept chats we are not in (kept before this rule, or left since) leave the list.
    fn prune_kept_left(&mut self) -> Result<()> {
        let left: Vec<PeerKey> = self
            .store
            .kept_dialogs()?
            .into_iter()
            .filter(|&key| key.kind != PeerKind::User)
            .filter(|&key| {
                self.store
                    .peer(key)
                    .ok()
                    .flatten()
                    .is_some_and(|p| describe::peer_info(&p).left)
            })
            .collect();
        if !left.is_empty() {
            self.store.write(|w| {
                for key in &left {
                    w.delete_dialog(*key)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    pub async fn request_dialog_entry(&mut self, peer: PeerKey) -> Result<bool> {
        if self.listed.contains(&peer) {
            return Ok(false);
        }
        let Some(input) = self.input_peer(peer) else {
            return Ok(false);
        };
        let enums::messages::PeerDialogs::PeerDialogs(answer) = self
            .client
            .invoke(&functions::messages::GetPeerDialogs {
                peers: vec![enums::InputDialogPeer::InputDialogPeer(Box::new(
                    types::InputDialogPeer { peer: input },
                ))],
            })
            .await?;
        let message_rows: Vec<MessageRow> = answer
            .messages
            .iter()
            .filter_map(convert::message_row)
            .collect();
        let peer_rows: Vec<_> = answer
            .users
            .iter()
            .filter_map(convert::user_row)
            .chain(answer.chats.iter().filter_map(convert::chat_row))
            .collect();
        let rows: Vec<DialogRow> = answer
            .dialogs
            .iter()
            .filter_map(|d| match d {
                enums::Dialog::Dialog(d) => Some(d),
                _ => None,
            })
            .map(|d| {
                let key = convert::peer_key(&d.peer);
                DialogRow {
                    peer: key,
                    top_message: d.top_message,
                    top_date: message_rows
                        .iter()
                        .find(|m| m.peer == key && m.id == d.top_message)
                        .map_or(0, |m| m.date),
                    pinned: d.pinned,
                    folder_id: d.folder_id.unwrap_or(0),
                    unread_count: d.unread_count,
                    pts: d.pts,
                    read_inbox_max_id: d.read_inbox_max_id,
                    read_outbox_max_id: d.read_outbox_max_id,
                    mute_until: convert::mute_until(&d.notify_settings),
                    unread_mentions: d.unread_mentions_count,
                    unread_mark: d.unread_mark,
                }
            })
            .collect();
        if rows.is_empty() {
            // No such chat any more.
            self.store.write(|w| w.delete_dialog(peer))?;
            return Ok(false);
        }
        // A group or channel we are not in is stored too (its history is read
        // through its row) but stays out of the list (`shouldBeInChatList`, see
        // `bridge::in_chat_list`).
        self.store.write(|w| {
            for p in &peer_rows {
                w.put_peer(p)?;
            }
            for m in &message_rows {
                w.put_message(m)?;
            }
            for d in &rows {
                w.put_dialog(d)?;
                w.set_kept(d.peer, true)?;
            }
            Ok(())
        })?;
        Ok(!rows.is_empty())
    }

    /// The whole chat list has been loaded from the server in this session.
    pub fn dialogs_complete(&self) -> bool {
        self.dialogs_complete
    }

    /// How many chats from the top of the list are confirmed by the server in this
    /// session (the cache below them may be stale).
    pub fn dialogs_loaded(&self) -> usize {
        self.dialogs_loaded
    }

    /// The next page of the chat list (tdesktop loads it as the list is scrolled down).
    /// Returns how many chats came.
    pub async fn load_more_dialogs(&mut self) -> Result<usize> {
        if self.dialogs_complete {
            return Ok(0);
        }
        self.load_dialogs_page(0).await
    }

    /// "Move to main menu" / "Move to chat list" of the archive's menu: the
    /// account's `archiveInMainMenu`, kept with its cache.
    pub fn set_archive_in_menu(&mut self, in_menu: bool) -> Result<()> {
        self.store
            .write(|w| w.set_meta_i64(ARCHIVE_IN_MENU_KEY, in_menu.into()))?;
        Ok(())
    }

    /// "Hide with Spoiler" of the own phone number (`setPhoneNumberHidden`).
    pub fn set_phone_hidden(&mut self, hidden: bool) -> Result<()> {
        self.store
            .write(|w| w.set_meta_i64(PHONE_HIDDEN_KEY, hidden.into()))?;
        Ok(())
    }

    /// Loads messages by id (`ApiWrap::requestMessageData`: `channels.getMessages`
    /// or `messages.getMessages`), e.g. the replied ones, into the cache without
    /// touching its history segments. Returns the ids that exist.
    pub async fn fetch_messages(&mut self, peer: PeerKey, ids: &[i32]) -> Result<Vec<i32>> {
        let id: Vec<enums::InputMessage> = ids
            .iter()
            .map(|&id| enums::InputMessage::ID(types::InputMessageID { id }))
            .collect();
        let answer = if peer.kind == PeerKind::Channel {
            let Some(access_hash) = self.store.access_hash(peer)? else {
                return Ok(Vec::new());
            };
            self.client
                .invoke(&functions::channels::GetMessages {
                    channel: types::InputChannel {
                        channel_id: peer.id,
                        access_hash,
                    }
                    .into(),
                    id,
                })
                .await?
        } else {
            self.client
                .invoke(&functions::messages::GetMessages { id })
                .await?
        };
        let (messages, chats, users) = match answer {
            enums::messages::Messages::Messages(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::Slice(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::ChannelMessages(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::NotModified(_) => return Ok(Vec::new()),
        };
        let rows: Vec<MessageRow> = messages.iter().filter_map(convert::message_row).collect();
        let peers: Vec<_> = users
            .iter()
            .filter_map(convert::user_row)
            .chain(chats.iter().filter_map(convert::chat_row))
            .collect();
        self.store.write(|w| {
            for p in &peers {
                w.put_peer(p)?;
            }
            for m in &rows {
                w.put_message(m)?;
            }
            Ok(())
        })?;
        Ok(rows
            .iter()
            .filter(|m| m.peer == peer)
            .map(|m| m.id)
            .collect())
    }

    /// Joins a channel or supergroup (`ApiWrap::joinChannel`: `channels.joinChannel`).
    /// `Err` carries the server's error type for the window's message
    /// (`INVITE_REQUEST_SENT`, `CHANNELS_TOO_MUCH`, ...).
    pub async fn join(&mut self, peer: PeerKey) -> Result<std::result::Result<Applied, String>> {
        let Some(access_hash) = self.store.access_hash(peer)? else {
            return Ok(Err("CHANNEL_PRIVATE".into()));
        };
        let answer = self
            .client
            .invoke(&functions::channels::JoinChannel {
                channel: types::InputChannel {
                    channel_id: peer.id,
                    access_hash,
                }
                .into(),
            })
            .await;
        match answer {
            Ok(enums::messages::ChatInviteJoinResult::Ok(ok)) => {
                let mut applied = self.feed(ok.updates).await?;
                applied.pinned_changed = true;
                Ok(Ok(applied))
            }
            // A bot's web view before joining: not supported yet.
            Ok(_) => Ok(Err("WEBVIEW".into())),
            Err(ApiError::Rpc(e)) => Ok(Err(e.message.clone())),
            Err(e) => Err(e.into()),
        }
    }

    /// Leaves a group or channel ("Leave group", "Leave channel", "Delete and
    /// leave" of a basic group; `ApiWrap::deleteConversation`): `channels.leaveChannel`,
    /// or `messages.deleteChatUser` for ourselves and the history deleted.
    pub async fn leave(&mut self, peer: PeerKey) -> Result<Applied> {
        let access_hash = self.store.access_hash(peer)?;
        let updates = match peer.kind {
            PeerKind::Channel => {
                let Some(access_hash) = access_hash else {
                    return Ok(Applied::default());
                };
                self.client
                    .invoke(&functions::channels::LeaveChannel {
                        channel: types::InputChannel {
                            channel_id: peer.id,
                            access_hash,
                        }
                        .into(),
                    })
                    .await?
            }
            PeerKind::Chat => {
                let updates = self
                    .client
                    .invoke(&functions::messages::DeleteChatUser {
                        revoke_history: false,
                        chat_id: peer.id,
                        user_id: enums::InputUser::InputUserSelf,
                    })
                    .await?;
                let mut applied = self.feed(updates).await?;
                if let Some(input) = convert::input_peer(peer, None) {
                    let _ = self
                        .client
                        .invoke(&functions::messages::DeleteHistory {
                            just_clear: false,
                            revoke: false,
                            peer: input,
                            max_id: 0,
                            min_date: None,
                            max_date: None,
                        })
                        .await?;
                }
                self.store.write(|w| w.delete_dialog(peer))?;
                applied.pinned_changed = true;
                return Ok(applied);
            }
            PeerKind::User => return Ok(Applied::default()),
        };
        let mut applied = self.feed(updates).await?;
        applied.pinned_changed = true;
        Ok(applied)
    }

    /// The archive was loaded to its end in this session.
    pub fn archive_complete(&self) -> bool {
        self.archive_complete
    }

    /// The next page of the archive (folder 1; `messages.getDialogs` with its
    /// `folder_id`, as tdesktop's `requestDialogs(folder)`), from its top the first
    /// time. Returns how many chats came.
    pub async fn load_more_archive(&mut self) -> Result<usize> {
        if self.archive_complete {
            return Ok(0);
        }
        self.load_dialogs_page(ARCHIVE_FOLDER).await
    }

    /// The user dragged the pinned chats of the main list into `order`: stored, then
    /// sent as tdesktop's `ApiWrap::savePinnedOrder` does.
    pub async fn reorder_pinned(&mut self, order: Vec<PeerKey>) -> Result<()> {
        self.store.write(|w| w.set_pinned_order(0, &order))?;
        let mut input: Vec<enums::InputDialogPeer> = order
            .iter()
            .filter_map(|&peer| self.input_peer(peer))
            .map(|peer| {
                enums::InputDialogPeer::InputDialogPeer(Box::new(types::InputDialogPeer { peer }))
            })
            .collect();
        // `force` unpins whatever the order leaves out: only send it when nothing is
        // missing.
        let complete = input.len() == order.len();
        for (index, extra) in &self.pinned_extra {
            input.insert((*index).min(input.len()), extra.clone());
        }
        self.client
            .invoke(&functions::messages::ReorderPinnedDialogs {
                force: complete,
                folder_id: 0,
                order: input,
            })
            .await?;
        Ok(())
    }

    pub fn store(&self) -> &Storage {
        &self.store
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Brings the cache up to date: a full load if it is empty, otherwise the difference
    /// since the saved state.
    pub async fn start(&mut self) -> Result<Applied> {
        if self.self_id == 0 {
            let users = self
                .client
                .invoke(&functions::users::GetUsers {
                    id: vec![enums::InputUser::InputUserSelf],
                })
                .await?;
            if let Some(enums::User::User(u)) = users.first() {
                self.self_id = u.id;
                let row = convert::user_row(&users[0]);
                self.store.write(|w| {
                    w.set_meta_i64("self_id", u.id)?;
                    if let Some(r) = &row {
                        w.put_peer(r)?;
                    }
                    Ok(())
                })?;
            }
        }
        self.prune_kept_left()?;
        if self.store.meta_i64(MUTE_USERS)?.is_none() {
            self.load_notify_defaults().await?;
        }
        if !self.engine.has_state() || self.store.dialog_count()? == 0 {
            return self.full_load().await;
        }
        let out = self.engine.connection_restored();
        let mut applied = self.run(out).await?;
        // Channels are not part of the common difference: refresh the top of the chat
        // list so that channels with new posts move up (tdesktop reloads the whole list).
        let t = Instant::now();
        self.load_dialogs(DIALOGS_PAGE as usize).await?;
        debug!(elapsed = ?t.elapsed(), "chat list refreshed");
        self.apply_drafts(&mut applied)?;
        Ok(applied)
    }

    /// Default mute settings of private chats, groups and channels (chats without their
    /// own setting follow them). Later changes come as `updateNotifySettings`.
    async fn load_notify_defaults(&mut self) -> Result<()> {
        let get = |peer| functions::account::GetNotifySettings { peer };
        let (users, chats, broadcasts) = (
            get(enums::InputNotifyPeer::InputNotifyUsers),
            get(enums::InputNotifyPeer::InputNotifyChats),
            get(enums::InputNotifyPeer::InputNotifyBroadcasts),
        );
        let (users, chats, broadcasts) = tokio::join!(
            self.client.invoke(&users),
            self.client.invoke(&chats),
            self.client.invoke(&broadcasts),
        );
        let (users, chats, broadcasts) = (users?, chats?, broadcasts?);
        self.store.write(|w| {
            for (key, settings) in [
                (MUTE_USERS, &users),
                (MUTE_CHATS, &chats),
                (MUTE_BROADCASTS, &broadcasts),
            ] {
                w.set_meta_i64(key, convert::mute_until(settings).unwrap_or(0).into())?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// Loads the state and the chat list from scratch.
    async fn full_load(&mut self) -> Result<Applied> {
        let started = Instant::now();
        // State first: updates that happen while the list loads come through the
        // difference later (applying them twice is harmless).
        let enums::updates::State::State(state) =
            self.client.invoke(&functions::updates::GetState {}).await?;
        let state = State::from(&*state);
        self.store.write(|w| w.clear_dialogs())?;
        let loaded = self.load_dialogs(INITIAL_DIALOGS).await?;
        // The archive's first page: its row in the list (names, unread chats).
        self.archive_cursor = None;
        self.archive_complete = false;
        if let Err(e) = self.load_dialogs_page(ARCHIVE_FOLDER).await {
            debug!("archive: {e}");
        }
        let out = self.engine.set_state(state);
        let mut applied = self.commit(out)?;
        applied.reloaded = true;
        self.apply_drafts(&mut applied)?;
        info!(loaded, elapsed = ?started.elapsed(), "chat list loaded");
        Ok(applied)
    }

    /// Loads the chat list from the top until `limit` chats came (or its end).
    async fn load_dialogs(&mut self, limit: usize) -> Result<usize> {
        self.dialogs_cursor = None;
        self.dialogs_complete = false;
        self.dialogs_loaded = 0;
        let mut loaded = 0;
        while loaded < limit && !self.dialogs_complete {
            loaded += self.load_dialogs_page(0).await?;
        }
        Ok(loaded)
    }

    /// One page of the chat list after `dialogs_cursor`. The cache may hold chats from
    /// earlier sessions that are no longer in the list or have moved: every page
    /// replaces the cached chats in the range of dates it covers, so the part of the
    /// list loaded in this session is exact.
    async fn load_dialogs_page(&mut self, folder: i32) -> Result<usize> {
        let archive = folder == ARCHIVE_FOLDER;
        let cursor = if archive {
            &self.archive_cursor
        } else {
            &self.dialogs_cursor
        };
        let first = cursor.is_none();
        let (offset_date, offset_id, offset_peer) = match cursor {
            Some(c) => (c.date, c.id, c.peer.clone()),
            None => (0, 0, enums::InputPeer::Empty),
        };
        let page = self
            .client
            .invoke(&functions::messages::GetDialogs {
                exclude_pinned: false,
                folder_id: archive.then_some(folder),
                offset_date,
                offset_id,
                offset_peer,
                limit: DIALOGS_PAGE,
                hash: 0,
            })
            .await?;
        let (dialogs, messages, chats, users, slice, total) = match page {
            enums::messages::Dialogs::Dialogs(d) => {
                let total = d.dialogs.len() as i64;
                (d.dialogs, d.messages, d.chats, d.users, false, total)
            }
            enums::messages::Dialogs::Slice(d) => {
                let total = i64::from(d.count);
                (d.dialogs, d.messages, d.chats, d.users, true, total)
            }
            enums::messages::Dialogs::NotModified(_) => {
                if archive {
                    self.archive_complete = true;
                } else {
                    self.dialogs_complete = true;
                }
                return Ok(0);
            }
        };
        let message_rows: Vec<MessageRow> =
            messages.iter().filter_map(convert::message_row).collect();
        let peer_rows: Vec<_> = users
            .iter()
            .filter_map(convert::user_row)
            .chain(chats.iter().filter_map(convert::chat_row))
            .collect();
        let hash_of = |key: PeerKey| {
            peer_rows
                .iter()
                .find(|p| p.key == key)
                .and_then(|p| p.access_hash)
        };
        let mut rows = Vec::new();
        // The first page starts with every pinned entry, in order.
        let mut pinned_order = Vec::new();
        let mut pinned_extra = Vec::new();
        for d in &dialogs {
            let d = match d {
                enums::Dialog::Dialog(d) => d,
                enums::Dialog::Folder(f) => {
                    if f.pinned {
                        let enums::Folder::Folder(folder) = &f.folder;
                        pinned_extra.push((
                            pinned_order.len() + pinned_extra.len(),
                            enums::InputDialogPeer::Folder(types::InputDialogPeerFolder {
                                folder_id: folder.id,
                            }),
                        ));
                    }
                    continue;
                }
                enums::Dialog::Community(c) => {
                    if c.pinned
                        && let Some(access_hash) = hash_of(PeerKey::channel(c.community_id))
                    {
                        pinned_extra.push((
                            pinned_order.len() + pinned_extra.len(),
                            enums::InputDialogPeer::Community(Box::new(
                                types::InputDialogPeerCommunity {
                                    community: enums::InputChannel::InputChannel(
                                        types::InputChannel {
                                            channel_id: c.community_id,
                                            access_hash,
                                        },
                                    ),
                                },
                            )),
                        ));
                    }
                    continue;
                }
            };
            let peer = convert::peer_key(&d.peer);
            let top_date = message_rows
                .iter()
                .find(|m| m.peer == peer && m.id == d.top_message)
                .map_or(0, |m| m.date);
            if peer.kind == PeerKind::Channel
                && let Some(pts) = d.pts
            {
                self.engine.set_channel_pts(peer.id, pts);
            }
            if let Some(draft) = &d.draft {
                self.dialog_drafts
                    .push((peer, convert::cloud_draft(draft), true));
            }
            if d.pinned {
                pinned_order.push(peer);
            }
            rows.push(DialogRow {
                peer,
                top_message: d.top_message,
                top_date,
                pinned: d.pinned,
                folder_id: d.folder_id.unwrap_or(0),
                unread_count: d.unread_count,
                pts: d.pts,
                read_inbox_max_id: d.read_inbox_max_id,
                read_outbox_max_id: d.read_outbox_max_id,
                mute_until: convert::mute_until(&d.notify_settings),
                unread_mentions: d.unread_mentions_count,
                unread_mark: d.unread_mark,
            });
        }
        let complete = !slice || dialogs.len() < DIALOGS_PAGE as usize || rows.is_empty();
        // The page's range of dates among unpinned chats: (its oldest, the cursor).
        let oldest = rows.iter().filter(|r| !r.pinned).map(|r| r.top_date).min();
        let after = if complete { None } else { oldest };
        let before = (!first).then_some(offset_date);
        let prune = complete || oldest.is_some();
        let keep: Vec<PeerKey> = rows.iter().map(|r| r.peer).collect();
        self.listed.extend(keep.iter().copied());
        let dirty = self.engine.take_dirty_channels();
        let removed = self.store.write(|w| {
            for p in &peer_rows {
                w.put_peer(p)?;
            }
            for m in &message_rows {
                w.put_message(m)?;
            }
            for d in &rows {
                w.put_dialog(d)?;
                // Listed after all: an ordinary chat again.
                w.set_kept(d.peer, false)?;
            }
            for (id, pts) in &dirty {
                w.set_channel_pts(*id, *pts)?;
            }
            if first {
                w.set_pinned_order(folder, &pinned_order)?;
            }
            // How many chats the archive has (`chatsList()->fullSize()`): "and N
            // more chats" on its row.
            if archive {
                w.set_meta_i64(ARCHIVE_COUNT_KEY, total)?;
            }
            if prune {
                w.prune_dialogs(folder, after, before, &keep)
            } else {
                Ok(0)
            }
        })?;
        if first && !archive {
            self.pinned_extra = pinned_extra;
        }
        if removed > 0 {
            debug!(removed, "stale chats dropped from the cached list");
        }
        // Continue after the last chat of this page.
        let next = rows.last().and_then(|last| {
            let peer = convert::input_peer(last.peer, hash_of(last.peer))?;
            Some(DialogsCursor {
                date: last.top_date,
                id: last.top_message,
                peer,
            })
        });
        let complete = complete || next.is_none();
        if archive {
            self.archive_complete = complete;
            self.archive_cursor = next;
        } else {
            self.dialogs_complete = complete;
            self.dialogs_loaded += rows.len();
            self.dialogs_cursor = next;
        }
        Ok(rows.len())
    }

    /// Waits up to `timeout` for updates and applies them.
    pub async fn next(&mut self, timeout: Duration) -> Result<Applied> {
        let event = self.wait_event(timeout).await;
        self.process(event).await
    }

    /// Waits for the next event (or the next gap deadline). Cancel-safe: use it inside
    /// `select!` and pass the result to [`Sync::process`], which must not be cancelled.
    pub async fn wait_event(&self, timeout: Duration) -> Option<Event> {
        let wait = match self.engine.next_deadline() {
            Some(d) => d.saturating_duration_since(Instant::now()).min(timeout),
            None => timeout,
        };
        self.client.next_event(wait).await
    }

    /// Applies an event from [`Sync::wait_event`] (`None`: a timeout).
    pub async fn process(&mut self, event: Option<Event>) -> Result<Applied> {
        let out = match event {
            Some(Event::Update(bytes)) => match enums::Updates::from_bytes(&bytes) {
                Ok(u) => self.engine.feed(u, Instant::now()),
                Err(e) => {
                    debug!("not an Updates object: {e}");
                    Output::default()
                }
            },
            Some(Event::NewSession | Event::Reconnected) => self.engine.connection_restored(),
            Some(Event::ConnectionLost) | None => self.engine.poll(Instant::now()),
        };
        self.run(out).await
    }

    /// Feeds `Updates` returned by a request (e.g. `messages.sendMessage`).
    pub async fn feed(&mut self, updates: enums::Updates) -> Result<Applied> {
        let out = self.engine.feed(updates, Instant::now());
        self.run(out).await
    }

    /// The field of `peer` holds `text` (typed, or the chat was left): keep it as the
    /// local draft. Returns whether it changed; [`Sync::save_drafts_to_cloud`] uploads.
    pub fn set_draft(&mut self, peer: PeerKey, text: &str) -> Result<bool> {
        let now = self.client.server_time();
        if !self.drafts.set_local(peer, text, now) {
            return Ok(false);
        }
        let draft = self.drafts.local(peer).map(|d| (d.text.clone(), d.date));
        self.store
            .write(|w| w.set_draft(peer, draft.as_ref().map(|(t, d)| (t.as_str(), *d))))?;
        Ok(true)
    }

    /// Local drafts not yet saved to the cloud.
    pub fn has_unsaved_drafts(&self) -> bool {
        !self.drafts.pending_cloud_saves().is_empty()
    }

    /// `ApiWrap::saveDraftsToCloud`: `messages.saveDraft` for every changed draft.
    pub async fn save_drafts_to_cloud(&mut self) -> Result<()> {
        for peer in self.drafts.pending_cloud_saves() {
            let access_hash = self.store.access_hash(peer)?;
            let Some(input) = convert::input_peer(peer, access_hash) else {
                self.drafts.start_save(peer, 0);
                self.drafts.finish_save(peer, 0);
                continue;
            };
            let message = self.drafts.start_save(peer, self.client.server_time());
            let result = self
                .client
                .invoke(&functions::messages::SaveDraft {
                    no_webpage: false,
                    invert_media: false,
                    reply_to: None,
                    peer: input,
                    message,
                    entities: None,
                    media: None,
                    effect: None,
                    suggested_post: None,
                    rich_message: None,
                })
                .await;
            self.drafts.finish_save(peer, self.client.server_time());
            if let Err(e) = result {
                debug!(?peer, "saveDraft failed: {e}");
            }
        }
        Ok(())
    }

    /// Turns the cloud drafts collected while applying into local draft changes, and
    /// clears drafts that were sent from another device.
    fn apply_drafts(&mut self, applied: &mut Applied) -> Result<()> {
        let mut changes = Vec::new();
        let mut cloud = std::mem::take(&mut self.dialog_drafts);
        cloud.append(&mut applied.cloud_drafts);
        for (peer, draft, from_dialog) in cloud {
            changes.extend(self.drafts.cloud_update(peer, draft, from_dialog));
        }
        for m in applied.new_messages.iter().filter(|m| m.out) {
            changes.extend(self.drafts.outgoing_message(m.peer, &m.text));
        }
        if changes.is_empty() {
            return Ok(());
        }
        let drafts = &self.drafts;
        self.store.write(|w| {
            for (peer, _) in &changes {
                let local = drafts.local(*peer);
                w.set_draft(*peer, local.map(|d| (d.text.as_str(), d.date)))?;
            }
            Ok(())
        })?;
        applied.draft_changes.extend(changes);
        Ok(())
    }

    /// The `InputPeer` of a cached chat (needs its access hash).
    pub fn input_peer(&self, peer: PeerKey) -> Option<enums::InputPeer> {
        let hash = self.store.access_hash(peer).ok().flatten();
        // The made-up "Telegram" account has no access hash: tdesktop sends 0.
        let hash = hash.or((peer == PeerKey::user(SERVICE_NOTIFICATIONS_ID)).then_some(0));
        convert::input_peer(peer, hash)
    }

    /// Marks incoming messages up to `max_id` as read (the user saw them), as tdesktop's
    /// `Histories::readInboxTill`: `messages.readHistory` (its pts goes through the
    /// update engine like any `updateReadHistoryInbox`) or `channels.readHistory`.
    pub async fn read_history(&mut self, peer: PeerKey, max_id: i32) -> Result<Applied> {
        let access_hash = self.store.access_hash(peer)?;
        if peer.kind == PeerKind::Channel {
            let Some(access_hash) = access_hash else {
                return Ok(Applied::default());
            };
            self.client
                .invoke(&functions::channels::ReadHistory {
                    channel: types::InputChannel {
                        channel_id: peer.id,
                        access_hash,
                    }
                    .into(),
                    max_id,
                })
                .await?;
            self.store.write(|w| w.set_read_inbox(peer, max_id, 0))?;
            return Ok(Applied {
                marks_changed: true,
                ..Default::default()
            });
        }
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok(Applied::default());
        };
        let enums::messages::AffectedMessages::AffectedMessages(affected) = self
            .client
            .invoke(&functions::messages::ReadHistory {
                peer: input,
                max_id,
            })
            .await?;
        let update = types::UpdateReadHistoryInbox {
            folder_id: None,
            peer: convert::peer_to_tl(peer),
            top_msg_id: None,
            max_id,
            still_unread_count: 0,
            pts: affected.pts,
            pts_count: affected.pts_count,
        };
        self.feed(
            types::UpdateShort {
                update: update.into(),
                date: self.client.server_time(),
            }
            .into(),
        )
        .await
    }

    /// Mutes a chat forever or unmutes it (the channel's "Mute" / "Unmute" button,
    /// `Data::NotifySettings::update` with `MuteValue{ .forever }` / `.unmute`). The
    /// chat's other settings (previews, sound) are sent back as they are, as tdesktop
    /// sends its whole `NotifyPeerSettingsValue`.
    pub async fn set_muted(&mut self, peer: PeerKey, mute: bool) -> Result<()> {
        let Some(input) = self.input_peer(peer) else {
            return Ok(());
        };
        let notify: enums::InputNotifyPeer = types::InputNotifyPeer { peer: input }.into();
        let enums::PeerNotifySettings::PeerNotifySettings(now) = self
            .client
            .invoke(&functions::account::GetNotifySettings {
                peer: notify.clone(),
            })
            .await?;
        let mute_until = if mute { i32::MAX } else { 0 };
        let settings = types::InputPeerNotifySettings {
            show_previews: now.show_previews,
            silent: now.silent,
            mute_until: Some(mute_until),
            sound: now.other_sound,
            stories_muted: now.stories_muted,
            stories_hide_sender: now.stories_hide_sender,
            stories_sound: now.stories_other_sound,
        };
        self.client
            .invoke(&functions::account::UpdateNotifySettings {
                peer: notify,
                settings: settings.into(),
            })
            .await?;
        self.store
            .write(|w| w.set_mute_until(peer, Some(mute_until)))?;
        Ok(())
    }

    /// Members of a group or subscribers of a channel (`*.getFullChannel`; basic groups
    /// carry the number in their object).
    /// Fresh user info (status, photo) for an opened private chat, as tdesktop asks for
    /// it when a profile is shown.
    pub async fn refresh_user(&mut self, peer: PeerKey) -> Result<bool> {
        if peer.kind != PeerKind::User {
            return Ok(false);
        }
        let Some(enums::InputPeer::User(u)) = self.input_peer(peer) else {
            return Ok(false);
        };
        let users = self
            .client
            .invoke(&functions::users::GetUsers {
                id: vec![enums::InputUser::InputUser(types::InputUser {
                    user_id: u.user_id,
                    access_hash: u.access_hash,
                })],
            })
            .await?;
        let rows: Vec<_> = users.iter().filter_map(convert::user_row).collect();
        self.store.write(|w| {
            for r in &rows {
                w.put_peer(r)?;
            }
            Ok(())
        })?;
        Ok(!rows.is_empty())
    }

    /// Members of a group and how many of them are online now
    /// (`TopBarWidget::updateOnlineDisplay`): a basic group's participants
    /// (`messages.getFullChat`), a supergroup's last participants
    /// (`channels.getParticipants`, recent) while it has no more than
    /// `chatSizeMax` members. `(members, online, only we are online)`.
    pub async fn online_members(
        &mut self,
        peer: PeerKey,
        members: i32,
    ) -> Result<Option<(i32, i32, bool)>> {
        const CHAT_SIZE_MAX: i32 = 200;
        let users: Vec<enums::User> = match peer.kind {
            PeerKind::Chat => {
                let enums::messages::ChatFull::ChatFull(full) = self
                    .client
                    .invoke(&functions::messages::GetFullChat { chat_id: peer.id })
                    .await?;
                let count = match &full.full_chat {
                    enums::ChatFull::ChatFull(c) => match &c.participants {
                        enums::ChatParticipants::ChatParticipants(p) => p.participants.len() as i32,
                        _ => 0,
                    },
                    _ => 0,
                };
                if count == 0 {
                    return Ok(None);
                }
                return Ok(Some(self.count_online(count, full.users)?));
            }
            PeerKind::Channel if members > 0 && members <= CHAT_SIZE_MAX => {
                let Some(access_hash) = self.store.access_hash(peer)? else {
                    return Ok(None);
                };
                let answer = self
                    .client
                    .invoke(&functions::channels::GetParticipants {
                        channel: types::InputChannel {
                            channel_id: peer.id,
                            access_hash,
                        }
                        .into(),
                        filter: enums::ChannelParticipantsFilter::ChannelParticipantsRecent,
                        offset: 0,
                        limit: CHAT_SIZE_MAX,
                        hash: 0,
                    })
                    .await?;
                match answer {
                    enums::channels::ChannelParticipants::ChannelParticipants(p) => p.users,
                    _ => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(self.count_online(members, users)?))
    }

    fn count_online(&mut self, members: i32, users: Vec<enums::User>) -> Result<(i32, i32, bool)> {
        let now = self.client.server_time();
        let rows: Vec<_> = users.iter().filter_map(convert::user_row).collect();
        self.store.write(|w| {
            for r in &rows {
                w.put_peer(r)?;
            }
            Ok(())
        })?;
        let mut online = 0;
        let mut only_me = true;
        for u in &users {
            if let enums::User::User(u) = u
                && let Some(enums::UserStatus::Online(s)) = &u.status
                && s.expires > now
            {
                online += 1;
                if u.id != self.self_id {
                    only_me = false;
                }
            }
        }
        Ok((members, online, only_me))
    }

    /// The full info of a supergroup or channel (`ApiWrap::requestFullPeer` when it
    /// is opened: `channels.getFullChannel`): stored with the peers that came with
    /// it (the channel itself, its discussion group). Returns it.
    pub async fn load_full(&mut self, peer: PeerKey) -> Result<Option<describe::FullInfo>> {
        let enums::messages::ChatFull::ChatFull(full) = match peer.kind {
            PeerKind::Channel => {
                let Some(access_hash) = self.store.access_hash(peer)? else {
                    return Ok(None);
                };
                self.client
                    .invoke(&functions::channels::GetFullChannel {
                        channel: types::InputChannel {
                            channel_id: peer.id,
                            access_hash,
                        }
                        .into(),
                    })
                    .await?
            }
            // A basic group (`messages.getFullChat`): its reactions.
            PeerKind::Chat => {
                self.client
                    .invoke(&functions::messages::GetFullChat { chat_id: peer.id })
                    .await?
            }
            _ => return Ok(None),
        };
        let peer_rows: Vec<_> = full
            .users
            .iter()
            .filter_map(convert::user_row)
            .chain(full.chats.iter().filter_map(convert::chat_row))
            .collect();
        let raw = convert::raw(&full.full_chat);
        self.store.write(|w| {
            for p in &peer_rows {
                w.put_peer(p)?;
            }
            if let Some(raw) = &raw {
                w.put_peer_full(peer, raw)?;
            }
            Ok(())
        })?;
        Ok(describe::full_info(&raw))
    }

    /// Sends a text message and records it in the cache.
    /// `HistoryItem::toggleReaction`: our reaction `key` to a message is chosen
    /// (in place of ours without Premium, beside them with it, up to three) or
    /// taken back. The message shows it at once (`reactionsWithLocal`), then
    /// `messages.sendReaction` sends all ours.
    pub async fn toggle_reaction(
        &mut self,
        peer: PeerKey,
        msg_id: i32,
        key: &describe::ReactionKey,
    ) -> Result<Applied> {
        let premium = self
            .store
            .peer(PeerKey::user(self.self_id))?
            .map(|p| describe::peer_info(&p).premium)
            .unwrap_or(false);
        let Some(row) = self.store.message(peer, msg_id)? else {
            return Ok(Applied::default());
        };
        let Some((layer, bytes)) = &row.raw else {
            return Ok(Applied::default());
        };
        if *layer != tdr_tl::LAYER {
            return Ok(Applied::default());
        }
        let Ok(mut message) = <enums::Message as tdr_tl::Deserializable>::from_bytes(
            &tdr_tl::Bytes::copy_from_slice(bytes),
        ) else {
            return Ok(Applied::default());
        };
        let slot = match &mut message {
            enums::Message::Message(m) => &mut m.reactions,
            enums::Message::Service(m) => &mut m.reactions,
            enums::Message::Empty(_) => return Ok(Applied::default()),
        };
        let mut r = match slot.take() {
            Some(enums::MessageReactions::MessageReactions(r)) => *r,
            None => types::MessageReactions {
                min: false,
                can_see_list: false,
                reactions_as_tags: false,
                results: Vec::new(),
                recent_reactions: None,
                top_reactors: None,
            },
        };
        let tl = key.tl();
        // Ours, in the order we chose them.
        let mut chosen: Vec<(i32, enums::Reaction)> = r
            .results
            .iter()
            .filter_map(|c| {
                let enums::ReactionCount::ReactionCount(c) = c;
                c.chosen_order.map(|o| (o, c.reaction.clone()))
            })
            .collect();
        chosen.sort_by_key(|(o, _)| *o);
        let mut chosen: Vec<enums::Reaction> = chosen.into_iter().map(|(_, r)| r).collect();
        let adding = !chosen.contains(&tl);
        if adding {
            let limit = if premium { 3 } else { 1 };
            chosen.push(tl.clone());
            while chosen.len() > limit {
                chosen.remove(0);
            }
        } else {
            chosen.retain(|c| *c != tl);
        }
        // The local copy: counts, our order and our recent entry.
        let me: enums::Peer = types::PeerUser {
            user_id: self.self_id,
        }
        .into();
        for c in &mut r.results {
            let enums::ReactionCount::ReactionCount(c) = c;
            let now_mine = chosen.contains(&c.reaction);
            if c.chosen_order.is_some() && !now_mine {
                c.count -= 1;
                c.chosen_order = None;
            }
        }
        for (order, reaction) in chosen.iter().enumerate() {
            let order = order as i32 + 1;
            match r.results.iter_mut().find_map(|c| {
                let enums::ReactionCount::ReactionCount(c) = c;
                (c.reaction == *reaction).then_some(c)
            }) {
                Some(c) => {
                    if c.chosen_order.is_none() {
                        c.count += 1;
                    }
                    c.chosen_order = Some(order);
                }
                None => r.results.push(
                    types::ReactionCount {
                        chosen_order: Some(order),
                        reaction: reaction.clone(),
                        count: 1,
                    }
                    .into(),
                ),
            }
        }
        r.results.retain(|c| {
            let enums::ReactionCount::ReactionCount(c) = c;
            c.count > 0
        });
        if let Some(recent) = &mut r.recent_reactions {
            recent.retain(|p| {
                let enums::MessagePeerReaction::MessagePeerReaction(p) = p;
                !(p.peer_id == me && !chosen.contains(&p.reaction))
            });
            if adding {
                recent.insert(
                    0,
                    types::MessagePeerReaction {
                        big: false,
                        unread: false,
                        my: true,
                        peer_id: me.clone(),
                        date: row.date,
                        reaction: tl.clone(),
                    }
                    .into(),
                );
            }
        }
        *slot = (!r.results.is_empty()).then(|| r.into());
        let mut applied = Applied::default();
        if let Some(local) = convert::message_row(&message) {
            self.store.write(|w| w.put_message(&local))?;
            applied.edited += 1;
        }
        let access_hash = self.store.access_hash(peer)?;
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok(applied);
        };
        let keys: Vec<describe::ReactionKey> = chosen
            .iter()
            .filter_map(describe::ReactionKey::of)
            .collect();
        let updates = reactions::send(&self.client, input, msg_id, &keys, adding).await?;
        let mut more = self.feed(updates).await?;
        more.edited += applied.edited;
        Ok(more)
    }

    pub async fn send_text(
        &mut self,
        peer: PeerKey,
        text: &str,
        entities: &[OutEntity],
        reply_to: Option<OutReply>,
    ) -> Result<Applied> {
        // Sent with `clear_draft`: the server drops the cloud draft, we drop ours.
        self.drafts.message_sent(peer);
        self.store.write(|w| w.set_draft(peer, None))?;
        let access_hash = self.store.access_hash(peer)?;
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok(Applied::default());
        };
        let random_id = i64::from_le_bytes(tdr_crypto_random());
        let updates = self
            .client
            .invoke(&functions::messages::SendMessage {
                no_webpage: false,
                silent: false,
                background: false,
                clear_draft: true,
                noforwards: false,
                update_stickersets_order: false,
                invert_media: false,
                allow_paid_floodskip: false,
                peer: input,
                reply_to: reply_to.map(|r| {
                    types::InputReplyToMessage {
                        reply_to_msg_id: r.msg_id,
                        top_msg_id: r.top_id,
                        reply_to_peer_id: None,
                        quote_text: None,
                        quote_entities: None,
                        quote_offset: None,
                        monoforum_peer_id: None,
                        todo_item_id: None,
                        poll_option: None,
                        quote_rich: None,
                    }
                    .into()
                }),
                message: text.to_string(),
                random_id,
                reply_markup: None,
                entities: (!entities.is_empty()).then(|| entities.iter().map(out_entity).collect()),
                schedule_date: None,
                schedule_repeat_period: None,
                send_as: None,
                quick_reply_shortcut: None,
                effect: None,
                allow_paid_stars: None,
                suggested_post: None,
                rich_message: None,
            })
            .await?;
        // In private chats the server answers with a compact form that has no text:
        // build the message row ourselves.
        if let enums::Updates::UpdateShortSentMessage(sent) = &updates {
            let row = MessageRow {
                peer,
                id: sent.id,
                date: sent.date,
                from: Some(PeerKey::user(self.self_id)),
                out: true,
                text: text.to_string(),
                raw: None,
            };
            let mut applied = self.feed(updates.clone()).await?;
            self.store.write(|w| {
                w.put_message(&row)?;
                w.message_arrived(&row)
            })?;
            // The short answer has no message object: a formatted one is fetched, so
            // it shows its formatting (the row keeps the TL object).
            let mut row = row;
            if sent.entities.as_ref().is_some_and(|e| !e.is_empty())
                && let Ok(Some(full)) = self.fetch_message(peer, sent.id).await
            {
                row = full;
            }
            applied.new_messages.push(row);
            return Ok(applied);
        }
        self.feed(updates).await
    }

    /// The folders (`messages.getDialogFilters`), "All chats" where it stands; kept
    /// in the cache (the TL object with its layer) for the next start.
    pub async fn dialog_filters(&mut self) -> Result<Vec<filters::FilterEntry>> {
        let answer = self
            .client
            .invoke(&functions::messages::GetDialogFilters {})
            .await?;
        let mut blob = tdr_tl::LAYER.to_le_bytes().to_vec();
        blob.extend_from_slice(&tdr_tl::Serializable::to_bytes(&answer));
        self.store.write(|w| w.set_meta(FILTERS_META, &blob))?;
        Ok(filter_entries(&answer))
    }
}

const FILTERS_META: &str = "dialog_filters";

/// The folders kept in the cache by [`Sync::dialog_filters`] (`None`: never loaded, or
/// cached by another layer).
pub fn cached_dialog_filters(store: &Storage) -> Option<Vec<filters::FilterEntry>> {
    let blob = store.meta(FILTERS_META).ok().flatten()?;
    let layer = i32::from_le_bytes(blob.get(..4)?.try_into().ok()?);
    if layer != tdr_tl::LAYER {
        return None;
    }
    let answer =
        enums::messages::DialogFilters::from_bytes(&tdr_tl::Bytes::copy_from_slice(&blob[4..]))
            .ok()?;
    Some(filter_entries(&answer))
}

fn filter_entries(answer: &enums::messages::DialogFilters) -> Vec<filters::FilterEntry> {
    let enums::messages::DialogFilters::DialogFilters(answer) = answer;
    {
        let mut list: Vec<filters::FilterEntry> = answer
            .filters
            .iter()
            .map(|f| match filters::ChatFilter::from_tl(f) {
                Some(f) => filters::FilterEntry::Folder(f),
                None => filters::FilterEntry::All,
            })
            .collect();
        if !list.contains(&filters::FilterEntry::All) {
            list.insert(0, filters::FilterEntry::All);
        }
        list
    }
}

impl Sync {
    /// One message of a private chat or a basic group (`messages.getMessages`),
    /// stored in the cache.
    async fn fetch_message(&mut self, peer: PeerKey, id: i32) -> Result<Option<MessageRow>> {
        if peer.kind == PeerKind::Channel {
            return Ok(None);
        }
        let answer = self
            .client
            .invoke(&functions::messages::GetMessages {
                id: vec![enums::InputMessage::ID(types::InputMessageID { id })],
            })
            .await?;
        let messages = match answer {
            enums::messages::Messages::Messages(m) => m.messages,
            enums::messages::Messages::Slice(m) => m.messages,
            enums::messages::Messages::ChannelMessages(m) => m.messages,
            enums::messages::Messages::NotModified(_) => Vec::new(),
        };
        let Some(row) = messages.iter().filter_map(convert::message_row).next() else {
            return Ok(None);
        };
        self.store.write(|w| w.put_message(&row))?;
        Ok(Some(row))
    }

    /// Commits `out`, then performs its actions (and the ones they lead to).
    async fn run(&mut self, out: Output) -> Result<Applied> {
        let mut actions = out.actions.clone();
        let mut applied = self.commit(out)?;
        while let Some(action) = (!actions.is_empty()).then(|| actions.remove(0)) {
            let next = match action {
                Action::GetDifference => self.get_difference().await?,
                Action::GetChannelDifference { channel_id, pts } => {
                    self.get_channel_difference(channel_id, pts).await?
                }
            };
            for a in &next.actions {
                if !actions.contains(a) {
                    actions.push(a.clone());
                }
            }
            applied.merge(self.commit(next)?);
        }
        // `Session::serviceNotification`: a chat not in the list yet is asked for
        // (`requestDialogEntry`), which brings the real "Telegram" account.
        let service = PeerKey::user(SERVICE_NOTIFICATIONS_ID);
        if applied.service_notification
            && !self.store.has_dialog(service)?
            && let Err(e) = self.request_dialog_entry(service).await
        {
            debug!("service notifications chat: {e}");
        }
        // After `differenceTooLong` the cached chat list is stale: reload it.
        if self.reload_if_stale().await? {
            applied.reloaded = true;
        }
        self.apply_drafts(&mut applied)?;
        Ok(applied)
    }

    async fn get_difference(&mut self) -> Result<Output> {
        let s = self.engine.state();
        let request = functions::updates::GetDifference {
            pts: s.pts,
            pts_limit: None,
            pts_total_limit: None,
            date: s.date,
            qts: s.qts,
            qts_limit: None,
        };
        match self.client.invoke(&request).await {
            Ok(diff) => Ok(self.engine.apply_difference(diff, Instant::now())),
            Err(e) => {
                self.engine.difference_failed(Instant::now());
                Err(e.into())
            }
        }
    }

    async fn get_channel_difference(&mut self, channel_id: i64, pts: i32) -> Result<Output> {
        let Some(access_hash) = self.store.access_hash(PeerKey::channel(channel_id))? else {
            debug!(channel_id, "no access hash, not tracking this channel");
            self.engine.forget_channel(channel_id);
            return Ok(Output::default());
        };
        let request = functions::updates::GetChannelDifference {
            force: false,
            channel: types::InputChannel {
                channel_id,
                access_hash,
            }
            .into(),
            filter: enums::ChannelMessagesFilter::Empty,
            pts,
            limit: CHANNEL_DIFFERENCE_LIMIT,
        };
        match self.client.invoke(&request).await {
            Ok(diff) => Ok(self
                .engine
                .apply_channel_difference(channel_id, diff, Instant::now())),
            Err(ApiError::Rpc(e)) if e.code == 400 || e.code == 403 || e.code == 406 => {
                // CHANNEL_PRIVATE, CHANNEL_INVALID, ...: we cannot follow it any more.
                warn!(channel_id, "{e}; not tracking this channel");
                self.engine.forget_channel(channel_id);
                Ok(Output::default())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Writes `out.items` and the resulting state in one transaction.
    fn commit(&mut self, out: Output) -> Result<Applied> {
        let state = to_storage(self.engine.state());
        let dirty = self.engine.take_dirty_channels();
        let self_id = self.self_id;
        let mut applied = Applied::default();
        let mut reset_common = false;
        self.store.write(|w| {
            for item in &out.items {
                apply_item(w, item, self_id, &mut applied, &mut reset_common)?;
            }
            w.set_update_state(state)?;
            for (id, pts) in &dirty {
                w.set_channel_pts(*id, *pts)?;
            }
            Ok(())
        })?;
        if reset_common {
            // Persisted, so an interrupted reload is retried on the next start.
            self.store.write(|w| w.set_meta_i64("dialogs_stale", 1))?;
        }
        Ok(applied)
    }
}

/// Downloads a peer's small userpic (`inputPeerPhotoFileLocation`, from the DC that
/// stores it). Independent of `Sync`, so several can run at once.
pub async fn download_userpic(
    client: &Client,
    peer: enums::InputPeer,
    photo_id: i64,
    dc_id: i32,
) -> Result<Vec<u8>> {
    // upload.getFile: the limit must divide 1 MiB and offsets must be multiples of it.
    const PART: i32 = 512 * 1024;
    let location: enums::InputFileLocation = types::InputPeerPhotoFileLocation {
        big: false,
        peer,
        photo_id,
    }
    .into();
    let mut data = Vec::new();
    loop {
        let request = functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: location.clone(),
            offset: data.len() as i64,
            limit: PART,
        };
        let enums::upload::File::File(part) = client.invoke_in(dc_id, &request).await? else {
            return Err(Error::Unsupported("CDN file"));
        };
        let done = part.bytes.len() < PART as usize;
        data.extend_from_slice(&part.bytes);
        if done {
            return Ok(data);
        }
    }
}

/// Default `mute_until` per chat type (`account.getNotifySettings`), in `meta`.
pub const MUTE_USERS: &str = "mute_users";
pub const MUTE_CHATS: &str = "mute_chats";
pub const MUTE_BROADCASTS: &str = "mute_broadcasts";

/// The "Telegram" account that posts service notifications (`kServiceNotificationsId`).
pub const SERVICE_NOTIFICATIONS_ID: i64 = 777000;

fn unix_now() -> i32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i32)
}

fn new_message(
    w: &tdr_storage::Writer<'_>,
    row: MessageRow,
    applied: &mut Applied,
) -> tdr_storage::Result<()> {
    w.put_message(&row)?;
    w.message_arrived(&row)?;
    if convert::unread_mention(&row) {
        w.mention_arrived(row.peer)?;
    }
    applied.new_messages.push(row);
    Ok(())
}

fn apply_item(
    w: &tdr_storage::Writer<'_>,
    item: &Item,
    self_id: i64,
    applied: &mut Applied,
    reset_common: &mut bool,
) -> tdr_storage::Result<()> {
    match item {
        Item::Users(users) => {
            for row in users.iter().filter_map(convert::user_row) {
                w.put_peer(&row)?;
            }
        }
        Item::Chats(chats) => {
            for row in chats.iter().filter_map(convert::chat_row) {
                w.put_peer(&row)?;
            }
        }
        Item::NewMessages(messages) => {
            for row in messages.iter().filter_map(convert::message_row) {
                new_message(w, row, applied)?;
            }
        }
        Item::ShortMessage(m) => new_message(w, convert::short_message_row(m, self_id), applied)?,
        Item::ShortChatMessage(m) => new_message(w, convert::short_chat_message_row(m), applied)?,
        Item::ResetCommon => *reset_common = true,
        Item::ResetChannel {
            channel_id,
            dialog,
            messages,
        } => {
            let peer = PeerKey::channel(*channel_id);
            w.delete_all_messages(peer)?;
            for row in messages.iter().filter_map(convert::message_row) {
                w.put_message(&row)?;
            }
            if let enums::Dialog::Dialog(d) = dialog {
                let top_date = messages
                    .iter()
                    .filter_map(convert::message_row)
                    .find(|m| m.id == d.top_message)
                    .map_or(0, |m| m.date);
                w.put_dialog(&DialogRow {
                    peer,
                    top_message: d.top_message,
                    top_date,
                    pinned: d.pinned,
                    folder_id: d.folder_id.unwrap_or(0),
                    unread_count: d.unread_count,
                    pts: d.pts,
                    read_inbox_max_id: d.read_inbox_max_id,
                    read_outbox_max_id: d.read_outbox_max_id,
                    mute_until: convert::mute_until(&d.notify_settings),
                    unread_mentions: d.unread_mentions_count,
                    unread_mark: d.unread_mark,
                })?;
            }
        }
        Item::Update(u) => apply_update(w, u, applied)?,
    }
    Ok(())
}

fn apply_update(
    w: &tdr_storage::Writer<'_>,
    u: &enums::Update,
    applied: &mut Applied,
) -> tdr_storage::Result<()> {
    use enums::Update as U;
    match u {
        U::NewMessage(x) => {
            if let Some(row) = convert::message_row(&x.message) {
                new_message(w, row, applied)?;
            }
        }
        U::NewChannelMessage(x) => {
            if let Some(row) = convert::message_row(&x.message) {
                new_message(w, row, applied)?;
            }
        }
        U::EditMessage(x) => {
            if let Some(row) = convert::message_row(&x.message) {
                w.put_message(&row)?;
                applied.edited += 1;
            }
        }
        U::EditChannelMessage(x) => {
            if let Some(row) = convert::message_row(&x.message) {
                w.put_message(&row)?;
                applied.edited += 1;
            }
        }
        // `HistoryItem::updateReactions`: the cached message gets them (a `min`
        // list keeps which we chose).
        U::MessageReactions(x) => {
            let peer = convert::peer_key(&x.peer);
            if let Some(row) = w.message(peer, x.msg_id)?
                && let Some((layer, bytes)) = &row.raw
                && *layer == tdr_tl::LAYER
                && let Ok(mut message) = <enums::Message as tdr_tl::Deserializable>::from_bytes(
                    &tdr_tl::Bytes::copy_from_slice(bytes),
                )
            {
                let changed = match &mut message {
                    enums::Message::Message(m) => {
                        m.reactions = Some(describe::reactions(&x.reactions, m.reactions.as_ref()));
                        true
                    }
                    enums::Message::Service(m) => {
                        m.reactions = Some(describe::reactions(&x.reactions, m.reactions.as_ref()));
                        true
                    }
                    enums::Message::Empty(_) => false,
                };
                if changed && let Some(row) = convert::message_row(&message) {
                    w.put_message(&row)?;
                    applied.edited += 1;
                }
            }
        }
        U::DeleteMessages(x) => {
            w.delete_common_messages(&x.messages)?;
            applied.deleted += x.messages.len();
        }
        U::DeleteChannelMessages(x) => {
            w.delete_messages(PeerKey::channel(x.channel_id), &x.messages)?;
            applied.deleted += x.messages.len();
        }
        // Login alerts and other notices: the server sends them as updates, the client
        // shows them as messages from "Telegram" (`Session::serviceNotification`).
        // Popups, forced log-outs and withdrawal notices are not messages.
        U::ServiceNotification(x)
            if !x.popup
                && !x.r#type.starts_with("AUTH_KEY_DROP_")
                && !x.r#type.starts_with("API_WITHDRAWAL_FEATURE_DISABLED_") =>
        {
            let peer = PeerKey::user(SERVICE_NOTIFICATIONS_ID);
            let date = x.inbox_date.unwrap_or_else(unix_now);
            if !w.has_local_message(peer, date, &x.message)? {
                // `Session::serviceNotification`: the account made up while it is
                // not loaded (no object, or no name).
                if !w.peer_loaded(peer)?
                    && let Some(row) =
                        convert::user_row(&convert::service_user(SERVICE_NOTIFICATIONS_ID))
                {
                    w.put_peer(&row)?;
                }
                let id = w.next_local_message_id()?;
                if let Some(row) = convert::service_message(SERVICE_NOTIFICATIONS_ID, id, date, x) {
                    new_message(w, row, applied)?;
                    applied.service_notification = true;
                }
            }
        }
        U::DraftMessage(x) if x.top_msg_id.is_none() && x.saved_peer_id.is_none() => {
            applied.cloud_drafts.push((
                convert::peer_key(&x.peer),
                convert::cloud_draft(&x.draft),
                false,
            ));
        }
        U::ReadHistoryInbox(x) if x.top_msg_id.is_none() => {
            w.set_read_inbox(convert::peer_key(&x.peer), x.max_id, x.still_unread_count)?;
            applied.marks_changed = true;
        }
        U::ReadChannelInbox(x) => {
            w.set_read_inbox(
                PeerKey::channel(x.channel_id),
                x.max_id,
                x.still_unread_count,
            )?;
            applied.marks_changed = true;
        }
        U::ReadHistoryOutbox(x) => {
            w.set_read_outbox(convert::peer_key(&x.peer), x.max_id)?;
            applied.marks_changed = true;
        }
        U::ReadChannelOutbox(x) => {
            w.set_read_outbox(PeerKey::channel(x.channel_id), x.max_id)?;
            applied.marks_changed = true;
        }
        U::NotifySettings(x) => {
            let mute = convert::mute_until(&x.notify_settings);
            match &x.peer {
                enums::NotifyPeer::NotifyPeer(p) => {
                    w.set_mute_until(convert::peer_key(&p.peer), mute)?;
                }
                enums::NotifyPeer::NotifyUsers => {
                    w.set_meta_i64(MUTE_USERS, mute.unwrap_or(0).into())?
                }
                enums::NotifyPeer::NotifyChats => {
                    w.set_meta_i64(MUTE_CHATS, mute.unwrap_or(0).into())?
                }
                enums::NotifyPeer::NotifyBroadcasts => {
                    w.set_meta_i64(MUTE_BROADCASTS, mute.unwrap_or(0).into())?
                }
                _ => {}
            }
            applied.marks_changed = true;
        }
        U::UserStatus(x) => {
            if set_user_status(w, x.user_id, &x.status)? {
                applied.statuses_changed = true;
            }
        }
        // Chats moved into the archive or out of it (`Session::processFolderPeers`):
        // a moved chat is no longer pinned there.
        U::FolderPeers(x) => {
            for f in &x.folder_peers {
                let enums::FolderPeer::FolderPeer(f) = f;
                w.set_dialog_folder(convert::peer_key(&f.peer), f.folder_id)?;
            }
            applied.pinned_changed = true;
        }
        U::DialogPinned(x) if x.folder_id.unwrap_or(0) == 0 => {
            if let enums::DialogPeer::DialogPeer(d) = &x.peer {
                w.set_pinned(convert::peer_key(&d.peer), x.pinned)?;
                applied.pinned_changed = true;
            }
        }
        U::PinnedDialogs(x) if x.folder_id.unwrap_or(0) == 0 => {
            // Without the order the next chat list refresh brings it.
            if let Some(order) = &x.order {
                let peers: Vec<PeerKey> = order
                    .iter()
                    .filter_map(|p| match p {
                        enums::DialogPeer::DialogPeer(d) => Some(convert::peer_key(&d.peer)),
                        _ => None,
                    })
                    .collect();
                w.set_pinned_order(0, &peers)?;
                applied.pinned_changed = true;
            }
        }
        U::DialogUnreadMark(x) if x.saved_peer_id.is_none() => {
            if let enums::DialogPeer::DialogPeer(d) = &x.peer {
                w.set_unread_mark(convert::peer_key(&d.peer), x.unread)?;
                applied.marks_changed = true;
            }
        }
        U::DialogFilter(_) | U::DialogFilterOrder(_) | U::DialogFilters => {
            applied.filters_changed = true;
        }
        // Statuses, typing, reactions, ... are not cached yet.
        _ => {}
    }
    Ok(())
}

/// `updateUserStatus`: the cached user gets the new status (tdesktop's
/// `UserData::setLastseen`). Returns whether a cached user changed.
fn set_user_status(
    w: &tdr_storage::Writer<'_>,
    user_id: i64,
    status: &enums::UserStatus,
) -> tdr_storage::Result<bool> {
    let Some(row) = w.peer(PeerKey::user(user_id))? else {
        return Ok(false);
    };
    let Some(mut user) = describe::user(&row) else {
        return Ok(false);
    };
    let enums::User::User(u) = &mut user else {
        return Ok(false);
    };
    u.status = Some(status.clone());
    match convert::user_row(&user) {
        Some(mut updated) => {
            // Keep what the stored row knew (a status update carries no more).
            updated.is_min = row.is_min;
            w.put_peer(&updated)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// See [`Sync::history`].
#[derive(Debug)]
pub struct History {
    /// Newest first.
    pub messages: Vec<MessageRow>,
    /// How many came from the cache without a request.
    pub from_cache: usize,
    /// How many were loaded from the server.
    pub fetched: usize,
}

const HISTORY_PAGE: usize = 100;

impl Sync {
    /// The last `limit` messages of a chat: the cached segment first, older messages
    /// loaded from the server only if the cache does not have enough. For a channel its
    /// difference is fetched first (as tdesktop does when a channel is opened), since
    /// channel updates do not come through the common difference.
    /// One page of history around `offset_id` (`messages.getHistory` with
    /// `add_offset`), stored as plain messages (outside the cached segment). Newest
    /// first.
    pub async fn history_page(
        &mut self,
        peer: PeerKey,
        offset_id: i32,
        add_offset: i32,
        limit: i32,
    ) -> Result<Vec<MessageRow>> {
        let access_hash = self.store.access_hash(peer)?;
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok(Vec::new());
        };
        let answer = self
            .client
            .invoke(&functions::messages::GetHistory {
                peer: input,
                offset_id,
                offset_date: 0,
                add_offset,
                limit,
                max_id: 0,
                min_id: 0,
                hash: 0,
            })
            .await?;
        let (messages, chats, users) = match answer {
            enums::messages::Messages::Messages(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::Slice(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::ChannelMessages(m) => (m.messages, m.chats, m.users),
            enums::messages::Messages::NotModified(_) => return Ok(Vec::new()),
        };
        let mut rows: Vec<MessageRow> = messages.iter().filter_map(convert::message_row).collect();
        rows.sort_by_key(|m| std::cmp::Reverse(m.id));
        let peers: Vec<_> = users
            .iter()
            .filter_map(convert::user_row)
            .chain(chats.iter().filter_map(convert::chat_row))
            .collect();
        self.store.write(|w| {
            for p in &peers {
                w.put_peer(p)?;
            }
            for m in &rows {
                w.put_message(m)?;
            }
            Ok(())
        })?;
        Ok(rows)
    }

    /// A page of a thread (a topic, a post's comments; `messages.getReplies`)
    /// older than `offset_id` (0: its newest), newest first, with the thread's
    /// message count. The messages go to the cache.
    pub async fn replies_page(
        &mut self,
        peer: PeerKey,
        root: i32,
        offset_id: i32,
        limit: i32,
    ) -> Result<(Vec<MessageRow>, i32)> {
        let access_hash = self.store.access_hash(peer)?;
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok((Vec::new(), 0));
        };
        let answer = self
            .client
            .invoke(&functions::messages::GetReplies {
                peer: input,
                msg_id: root,
                offset_id,
                offset_date: 0,
                add_offset: 0,
                limit,
                max_id: 0,
                min_id: 0,
                hash: 0,
            })
            .await?;
        let (messages, chats, users, count) = match answer {
            enums::messages::Messages::Messages(m) => {
                let count = m.messages.len() as i32;
                (m.messages, m.chats, m.users, count)
            }
            enums::messages::Messages::Slice(m) => (m.messages, m.chats, m.users, m.count),
            enums::messages::Messages::ChannelMessages(m) => {
                (m.messages, m.chats, m.users, m.count)
            }
            enums::messages::Messages::NotModified(_) => return Ok((Vec::new(), 0)),
        };
        let mut rows: Vec<MessageRow> = messages.iter().filter_map(convert::message_row).collect();
        rows.sort_by_key(|m| std::cmp::Reverse(m.id));
        self.store_loaded(&rows, &users, &chats)?;
        Ok((rows, count))
    }

    /// A post's discussion (`messages.getDiscussionMessage`): the group and the
    /// root message there (the post's copy), newest first (an album has several).
    pub async fn discussion(
        &mut self,
        channel: PeerKey,
        post: i32,
    ) -> Result<Option<(PeerKey, Vec<MessageRow>)>> {
        let access_hash = self.store.access_hash(channel)?;
        let Some(input) = convert::input_peer(channel, access_hash) else {
            return Ok(None);
        };
        let enums::messages::DiscussionMessage::DiscussionMessage(d) = self
            .client
            .invoke(&functions::messages::GetDiscussionMessage {
                peer: input,
                msg_id: post,
            })
            .await?;
        let mut rows: Vec<MessageRow> =
            d.messages.iter().filter_map(convert::message_row).collect();
        rows.sort_by_key(|m| std::cmp::Reverse(m.id));
        self.store_loaded(&rows, &d.users, &d.chats)?;
        Ok(rows.first().map(|m| m.peer).map(|group| (group, rows)))
    }

    /// A forum's topics (`messages.getForumTopics`), as the server orders them
    /// (pinned first, then by their last message), with their last messages
    /// stored.
    pub async fn forum_topics(&mut self, peer: PeerKey, limit: i32) -> Result<Vec<Topic>> {
        let access_hash = self.store.access_hash(peer)?;
        let Some(input) = convert::input_peer(peer, access_hash) else {
            return Ok(Vec::new());
        };
        let enums::messages::ForumTopics::ForumTopics(t) = self
            .client
            .invoke(&functions::messages::GetForumTopics {
                peer: input,
                q: None,
                offset_date: 0,
                offset_id: 0,
                offset_topic: 0,
                limit,
            })
            .await?;
        let rows: Vec<MessageRow> = t.messages.iter().filter_map(convert::message_row).collect();
        self.store_loaded(&rows, &t.users, &t.chats)?;
        Ok(t.topics
            .iter()
            .filter_map(|topic| match topic {
                enums::ForumTopic::ForumTopic(f) => Some(Topic {
                    id: f.id,
                    title: f.title.clone(),
                    icon_color: f.icon_color,
                    icon_emoji: f.icon_emoji_id.is_some(),
                    top_message: f.top_message,
                    unread: f.unread_count,
                    pinned: f.pinned,
                    closed: f.closed,
                    hidden: f.hidden,
                    muted: convert::mute_until(&f.notify_settings)
                        .is_some_and(|u| u > self.client.server_time()),
                }),
                enums::ForumTopic::Deleted(_) => None,
            })
            .collect())
    }

    /// Stores what a request brought: its messages (no history segments change)
    /// and peers.
    fn store_loaded(
        &mut self,
        rows: &[MessageRow],
        users: &[enums::User],
        chats: &[enums::Chat],
    ) -> Result<()> {
        let peers: Vec<_> = users
            .iter()
            .filter_map(convert::user_row)
            .chain(chats.iter().filter_map(convert::chat_row))
            .collect();
        self.store.write(|w| {
            for p in &peers {
                w.put_peer(p)?;
            }
            for m in rows {
                w.put_message(m)?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// A window of history around `id` for a message the cached segment does not
    /// reach (tdesktop's jump to a message): `None` when the segment has it.
    pub async fn history_around(
        &mut self,
        peer: PeerKey,
        id: i32,
        limit: i32,
    ) -> Result<Option<Vec<MessageRow>>> {
        if self.store.count_from(peer, id)?.is_some() {
            return Ok(None);
        }
        Ok(Some(self.history_page(peer, id, -limit / 2, limit).await?))
    }

    /// A window (newest first) that reached the newest messages joins the cached
    /// segment: the part below the segment continues it downwards.
    pub fn join_window(&mut self, peer: PeerKey, window: &[MessageRow]) -> Result<()> {
        let Some(from) = self.store.cached_from(peer)? else {
            return Ok(());
        };
        let older: Vec<MessageRow> = window.iter().filter(|m| m.id < from).cloned().collect();
        if older.is_empty() {
            return Ok(());
        }
        // Not fewer than asked for: this is not the start of the chat.
        self.store
            .write(|w| w.extend_history(peer, from, false, &older))?;
        Ok(())
    }

    pub async fn history(&mut self, peer: PeerKey, limit: usize) -> Result<History> {
        if peer.kind == PeerKind::Channel
            && let Some(pts) = self.engine.channel_pts(peer.id)
        {
            let out = Output {
                items: vec![],
                actions: vec![Action::GetChannelDifference {
                    channel_id: peer.id,
                    pts,
                }],
            };
            self.run(out).await?;
        }
        let cached = self.store.history(peer, limit)?;
        let from_cache = cached.messages.len();
        let mut fetched = 0;
        let mut have = cached.messages.len();
        let mut offset = cached.cached_from.unwrap_or(0);
        let mut complete = cached.complete;
        while have < limit && !complete {
            let access_hash = self.store.access_hash(peer)?;
            let Some(input) = convert::input_peer(peer, access_hash) else {
                break;
            };
            let page = (limit - have).min(HISTORY_PAGE);
            let answer = self
                .client
                .invoke(&functions::messages::GetHistory {
                    peer: input,
                    offset_id: offset,
                    offset_date: 0,
                    add_offset: 0,
                    limit: page as i32,
                    max_id: 0,
                    min_id: 0,
                    hash: 0,
                })
                .await?;
            // `messages.messages` is the whole history at once; otherwise only an
            // empty page says the start was reached (a short one may not).
            let (messages, chats, users, whole) = match answer {
                enums::messages::Messages::Messages(m) => (m.messages, m.chats, m.users, true),
                enums::messages::Messages::Slice(m) => (m.messages, m.chats, m.users, false),
                enums::messages::Messages::ChannelMessages(m) => {
                    (m.messages, m.chats, m.users, false)
                }
                enums::messages::Messages::NotModified(_) => break,
            };
            let start_reached = messages.is_empty() || (whole && messages.len() < page);
            let rows: Vec<MessageRow> = messages.iter().filter_map(convert::message_row).collect();
            let peers: Vec<_> = users
                .iter()
                .filter_map(convert::user_row)
                .chain(chats.iter().filter_map(convert::chat_row))
                .collect();
            // A chat without a row (a channel looked at, whose dialog the server
            // did not give): its newest message makes one, the history hangs on it.
            let newest = rows.iter().max_by_key(|m| m.id);
            let made_row = match newest {
                Some(top) if offset == 0 && !self.store.has_dialog(peer)? => Some(DialogRow {
                    peer,
                    top_message: top.id,
                    top_date: top.date,
                    pinned: false,
                    folder_id: 0,
                    unread_count: 0,
                    pts: None,
                    read_inbox_max_id: top.id,
                    read_outbox_max_id: 0,
                    mute_until: None,
                    unread_mentions: 0,
                    unread_mark: false,
                }),
                _ => None,
            };
            let extend_from = made_row.as_ref().map_or(offset, |r| r.top_message);
            self.store.write(|w| {
                for p in &peers {
                    w.put_peer(p)?;
                }
                if let Some(row) = &made_row {
                    w.put_dialog(row)?;
                }
                w.extend_history(peer, extend_from, start_reached, &rows)
            })?;
            fetched += rows.len();
            have += rows.len();
            complete = start_reached;
            match rows.iter().map(|m| m.id).min() {
                Some(oldest) => offset = oldest,
                None => break,
            }
        }
        let messages = self.store.history(peer, limit)?.messages;
        Ok(History {
            messages,
            from_cache,
            fetched,
        })
    }

    /// Reloads the chat list if a `differenceTooLong` made it stale.
    pub async fn reload_if_stale(&mut self) -> Result<bool> {
        if self.store.meta_i64("dialogs_stale")? != Some(1) {
            return Ok(false);
        }
        self.store.write(|w| w.clear_dialogs())?;
        self.load_dialogs(INITIAL_DIALOGS).await?;
        self.store.write(|w| w.set_meta_i64("dialogs_stale", 0))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(popup: bool, kind: &str, date: i32) -> enums::Update {
        enums::Update::ServiceNotification(Box::new(types::UpdateServiceNotification {
            popup,
            invert_media: false,
            inbox_date: Some(date),
            r#type: kind.into(),
            message: "New login.".into(),
            media: enums::MessageMedia::Empty,
            entities: vec![enums::MessageEntity::Bold(types::MessageEntityBold {
                offset: 0,
                length: 10,
            })],
        }))
    }

    #[test]
    fn service_notifications_become_telegram_messages() {
        let mut store = Storage::open_in_memory().unwrap();
        let mut applied = Applied::default();
        let tg = PeerKey::user(SERVICE_NOTIFICATIONS_ID);
        store
            .write(|w| {
                // A nameless row of the account (seen before without its object) is
                // not "loaded": the made-up account replaces it.
                w.put_peer(&tdr_storage::PeerRow {
                    key: tg,
                    access_hash: None,
                    is_min: true,
                    is_megagroup: false,
                    name: String::new(),
                    username: None,
                    raw: None,
                })?;
                apply_update(w, &notification(false, "auth123_1", 5000), &mut applied)?;
                // The same one again (difference and live): no duplicate.
                apply_update(w, &notification(false, "auth123_1", 5000), &mut applied)?;
                // Popups and forced log-outs are not messages.
                apply_update(w, &notification(true, "x", 5001), &mut applied)?;
                apply_update(
                    w,
                    &notification(false, "AUTH_KEY_DROP_DUPLICATE", 5002),
                    &mut applied,
                )
            })
            .unwrap();
        assert_eq!(applied.new_messages.len(), 1);
        assert!(applied.service_notification);
        let history = store.history(tg, 10).unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].text, "New login.");
        assert!(history.messages[0].id > tdr_storage::SERVER_MAX_MESSAGE_ID);
        let list = store.dialogs(0, 10).unwrap();
        assert_eq!(list[0].dialog.peer, tg);
        assert_eq!(list[0].dialog.unread_count, 1);
        assert_eq!(list[0].peer.as_ref().unwrap().name, "Telegram");
        // As `Session::serviceNotification` makes them: the account verified with
        // its number, the message with its entities.
        let info = describe::peer_info(list[0].peer.as_ref().unwrap());
        assert!(info.verified);
        assert_eq!(info.phone.as_deref(), Some("42777"));
        match &history.messages[0].raw {
            Some((_, bytes)) => {
                use tdr_tl::Deserializable;
                let m = enums::Message::from_bytes(&tdr_tl::Bytes::copy_from_slice(bytes)).unwrap();
                let enums::Message::Message(m) = m else {
                    panic!("not a message")
                };
                assert_eq!(m.entities.map(|e| e.len()), Some(1));
            }
            None => panic!("no object"),
        }
    }
}
