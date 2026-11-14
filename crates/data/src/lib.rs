//! Data layer: keeps the local cache in sync with the server.
//!
//! [`Sync`] owns the update [`Engine`](tdr_updates::Engine) and the [`Storage`]. On start
//! it either loads the chat list from scratch (`updates.getState` + `messages.getDialogs`)
//! or catches up from the saved state with `updates.getDifference`. Afterwards it applies
//! live updates. Every batch is written in one transaction together with the update
//! state it leads to, so after a crash the cache and the state always agree.

pub mod convert;

use std::time::{Duration, Instant};

use tdr_api::{Client, Error as ApiError, Event};
use tdr_storage::{DialogRow, MessageRow, PeerKey, PeerKind, Storage, UpdateState};
use tdr_tl::{Deserializable, enums, functions, types};
use tdr_updates::{Action, Engine, Item, Output, State};
use tracing::{debug, info, warn};

pub use tdr_storage;

#[derive(Debug)]
pub enum Error {
    Api(ApiError),
    Storage(tdr_storage::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "cache: {e}"),
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
}

impl Applied {
    fn merge(&mut self, other: Applied) {
        self.new_messages.extend(other.new_messages);
        self.edited += other.edited;
        self.deleted += other.deleted;
        self.reloaded |= other.reloaded;
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
        Ok(Self {
            client,
            store,
            engine,
            self_id,
        })
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
        if !self.engine.has_state() || self.store.dialog_count()? == 0 {
            return self.full_load().await;
        }
        let out = self.engine.connection_restored();
        self.run(out).await
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
        info!(loaded, elapsed = ?started.elapsed(), "chat list loaded");
        Ok(applied)
    }

    async fn load_dialogs(&mut self, limit: usize) -> Result<usize> {
        let mut offset_date = 0;
        let mut offset_id = 0;
        let mut offset_peer = enums::InputPeer::Empty;
        let mut loaded = 0;
        while loaded < limit {
            let page = self
                .client
                .invoke(&functions::messages::GetDialogs {
                    exclude_pinned: false,
                    folder_id: None,
                    offset_date,
                    offset_id,
                    offset_peer: offset_peer.clone(),
                    limit: DIALOGS_PAGE,
                    hash: 0,
                })
                .await?;
            let (dialogs, messages, chats, users, total) = match page {
                enums::messages::Dialogs::Dialogs(d) => {
                    let n = d.dialogs.len();
                    (d.dialogs, d.messages, d.chats, d.users, n)
                }
                enums::messages::Dialogs::Slice(d) => {
                    (d.dialogs, d.messages, d.chats, d.users, d.count as usize)
                }
                enums::messages::Dialogs::NotModified(_) => break,
            };
            let message_rows: Vec<MessageRow> =
                messages.iter().filter_map(convert::message_row).collect();
            let peer_rows: Vec<_> = users
                .iter()
                .filter_map(convert::user_row)
                .chain(chats.iter().filter_map(convert::chat_row))
                .collect();
            let mut rows = Vec::new();
            for d in &dialogs {
                let enums::Dialog::Dialog(d) = d else {
                    continue;
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
                rows.push(DialogRow {
                    peer,
                    top_message: d.top_message,
                    top_date,
                    pinned: d.pinned,
                    folder_id: d.folder_id.unwrap_or(0),
                    unread_count: d.unread_count,
                    pts: d.pts,
                });
            }
            let dirty = self.engine.take_dirty_channels();
            self.store.write(|w| {
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
                Ok(())
            })?;
            loaded += rows.len();
            if dialogs.len() < DIALOGS_PAGE as usize || loaded >= total {
                break;
            }
            // Continue after the last chat of this page.
            let Some(last) = rows.last() else { break };
            offset_date = last.top_date;
            offset_id = last.top_message;
            let hash = peer_rows
                .iter()
                .find(|p| p.key == last.peer)
                .and_then(|p| p.access_hash);
            match convert::input_peer(last.peer, hash) {
                Some(p) => offset_peer = p,
                None => break,
            }
        }
        Ok(loaded)
    }

    /// Waits up to `timeout` for updates and applies them.
    pub async fn next(&mut self, timeout: Duration) -> Result<Applied> {
        let wait = match self.engine.next_deadline() {
            Some(d) => d.saturating_duration_since(Instant::now()).min(timeout),
            None => timeout,
        };
        let out = match self.client.next_event(wait).await {
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
        U::ReadHistoryInbox(x) => w.set_unread(convert::peer_key(&x.peer), x.still_unread_count)?,
        U::ReadChannelInbox(x) => {
            w.set_unread(PeerKey::channel(x.channel_id), x.still_unread_count)?
        }
        // Statuses, typing, reactions, ... are not cached yet.
        _ => {}
    }
    Ok(())
}

impl Sync {
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
