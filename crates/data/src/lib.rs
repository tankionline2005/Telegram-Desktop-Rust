//! Data layer: keeps the local cache in sync with the server.
//!
//! [`Sync`] owns the update [`Engine`](tdr_updates::Engine) and the [`Storage`]. On start
//! it either loads the chat list from scratch (`updates.getState` + `messages.getDialogs`)
//! or catches up from the saved state with `updates.getDifference`. Afterwards it applies
//! live updates. Every batch is written in one transaction together with the update
//! state it leads to, so after a crash the cache and the state always agree.

pub mod convert;
pub mod describe;
pub mod drafts;

use std::time::{Duration, Instant};

use drafts::Drafts;
use tdr_api::{Client, Error as ApiError, Event};
use tdr_storage::{DialogRow, MessageRow, PeerKey, PeerKind, Storage, UpdateState};
use tdr_tl::{Deserializable, enums, functions, types};
use tdr_updates::{Action, Engine, Item, Output, State};
use tracing::{debug, info, warn};

pub use tdr_storage;

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
    /// Users went online or offline.
    pub statuses_changed: bool,
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
        self.draft_changes.extend(other.draft_changes);
        self.cloud_drafts.extend(other.cloud_drafts);
    }
}

/// How many chats to load when the cache is empty.
pub const INITIAL_DIALOGS: usize = 200;
const DIALOGS_PAGE: i32 = 100;
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
    /// Chats loaded from the top of the server's list in this session.
    dialogs_loaded: usize,
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
            dialogs_loaded: 0,
            pinned_extra: Vec::new(),
        })
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
        self.load_dialogs_page().await
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
            loaded += self.load_dialogs_page().await?;
        }
        Ok(loaded)
    }

    /// One page of the chat list after `dialogs_cursor`. The cache may hold chats from
    /// earlier sessions that are no longer in the list or have moved: every page
    /// replaces the cached chats in the range of dates it covers, so the part of the
    /// list loaded in this session is exact.
    async fn load_dialogs_page(&mut self) -> Result<usize> {
        let first = self.dialogs_cursor.is_none();
        let (offset_date, offset_id, offset_peer) = match &self.dialogs_cursor {
            Some(c) => (c.date, c.id, c.peer.clone()),
            None => (0, 0, enums::InputPeer::Empty),
        };
        let page = self
            .client
            .invoke(&functions::messages::GetDialogs {
                exclude_pinned: false,
                folder_id: None,
                offset_date,
                offset_id,
                offset_peer,
                limit: DIALOGS_PAGE,
                hash: 0,
            })
            .await?;
        let (dialogs, messages, chats, users, slice) = match page {
            enums::messages::Dialogs::Dialogs(d) => {
                (d.dialogs, d.messages, d.chats, d.users, false)
            }
            enums::messages::Dialogs::Slice(d) => (d.dialogs, d.messages, d.chats, d.users, true),
            enums::messages::Dialogs::NotModified(_) => {
                self.dialogs_complete = true;
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
            });
        }
        let complete = !slice || dialogs.len() < DIALOGS_PAGE as usize || rows.is_empty();
        // The page's range of dates among unpinned chats: (its oldest, the cursor).
        let oldest = rows.iter().filter(|r| !r.pinned).map(|r| r.top_date).min();
        let after = if complete { None } else { oldest };
        let before = (!first).then_some(offset_date);
        let prune = complete || oldest.is_some();
        let keep: Vec<PeerKey> = rows.iter().map(|r| r.peer).collect();
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
            }
            for (id, pts) in &dirty {
                w.set_channel_pts(*id, *pts)?;
            }
            if first {
                w.set_pinned_order(0, &pinned_order)?;
            }
            if prune {
                w.prune_dialogs(0, after, before, &keep)
            } else {
                Ok(0)
            }
        })?;
        if first {
            self.pinned_extra = pinned_extra;
        }
        if removed > 0 {
            debug!(removed, "stale chats dropped from the cached list");
        }
        self.dialogs_complete = complete;
        self.dialogs_loaded += rows.len();
        // Continue after the last chat of this page.
        self.dialogs_cursor = rows.last().and_then(|last| {
            let peer = convert::input_peer(last.peer, hash_of(last.peer))?;
            Some(DialogsCursor {
                date: last.top_date,
                id: last.top_message,
                peer,
            })
        });
        if self.dialogs_cursor.is_none() {
            self.dialogs_complete = true;
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
            None => self.engine.poll(Instant::now()),
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

    pub async fn member_count(&mut self, peer: PeerKey) -> Result<Option<i32>> {
        if peer.kind != PeerKind::Channel {
            return Ok(None);
        }
        let Some(access_hash) = self.store.access_hash(peer)? else {
            return Ok(None);
        };
        let enums::messages::ChatFull::ChatFull(full) = self
            .client
            .invoke(&functions::channels::GetFullChannel {
                channel: types::InputChannel {
                    channel_id: peer.id,
                    access_hash,
                }
                .into(),
            })
            .await?;
        Ok(match &full.full_chat {
            enums::ChatFull::ChannelFull(c) => c.participants_count,
            _ => None,
        })
    }

    /// Sends a text message and records it in the cache.
    pub async fn send_text(&mut self, peer: PeerKey, text: &str) -> Result<Applied> {
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
                reply_to: None,
                message: text.to_string(),
                random_id,
                reply_markup: None,
                entities: None,
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
            applied.new_messages.push(row);
            return Ok(applied);
        }
        self.feed(updates).await
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
                w.put_peer_if_missing(&tdr_storage::PeerRow {
                    key: peer,
                    access_hash: None,
                    is_min: false,
                    is_megagroup: false,
                    name: "Telegram".into(),
                    username: None,
                    raw: None,
                })?;
                let row = MessageRow {
                    peer,
                    id: w.next_local_message_id()?,
                    date,
                    from: Some(peer),
                    out: false,
                    text: x.message.clone(),
                    raw: None,
                };
                new_message(w, row, applied)?;
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
            self.store.write(|w| {
                for p in &peers {
                    w.put_peer(p)?;
                }
                w.extend_history(peer, offset, start_reached, &rows)
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
            entities: vec![],
        }))
    }

    #[test]
    fn service_notifications_become_telegram_messages() {
        let mut store = Storage::open_in_memory().unwrap();
        let mut applied = Applied::default();
        store
            .write(|w| {
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
        let tg = PeerKey::user(SERVICE_NOTIFICATIONS_ID);
        let history = store.history(tg, 10).unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].text, "New login.");
        assert!(history.messages[0].id > tdr_storage::SERVER_MAX_MESSAGE_ID);
        let list = store.dialogs(0, 10).unwrap();
        assert_eq!(list[0].dialog.peer, tg);
        assert_eq!(list[0].dialog.unread_count, 1);
        assert_eq!(list[0].peer.as_ref().unwrap().name, "Telegram");
    }
}
