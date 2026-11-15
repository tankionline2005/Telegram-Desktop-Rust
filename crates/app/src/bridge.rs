//! The data side of the app: a background thread with its own async runtime that owns
//! the network client and the cache. The UI thread never waits for the network or the
//! disk; it sends [`Command`]s and receives ready-to-draw [`Update`]s.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::RenderImage;
use tdr_data::describe::{self, PeerKind as ChatKind};
use tdr_data::tdr_storage::{FileCache, FileCacheConfig};

use chrono::Local;
use tdr_api::{Client, Config, SessionData};
use tdr_data::tdr_storage::{DialogView, MessageRow, PeerKey, PeerKind, Storage};
use tdr_data::{Applied, Sync};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::present;

#[derive(Debug, Clone)]
pub enum Command {
    OpenChat(PeerKey),
    /// The chat view was closed (Esc).
    CloseChat,
    /// The user scrolled to the top of the open chat: load older messages.
    LoadOlder(PeerKey),
    Send {
        peer: PeerKey,
        text: String,
    },
    /// The user saw the open chat's messages up to `max_id`: mark them read.
    Read {
        peer: PeerKey,
        max_id: i32,
    },
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
}

#[derive(Debug, Clone)]
pub enum Update {
    Dialogs(Vec<DialogItem>),
    History {
        peer: PeerKey,
        messages: Vec<MessageItem>,
        /// The first message of the chat is loaded: nothing older to ask for.
        complete: bool,
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
    /// A userpic is ready to draw.
    Userpic {
        peer: PeerKey,
        photo_id: i64,
        image: Arc<RenderImage>,
    },
    /// Connection state for the header ("Connecting…"), empty when online.
    Status(String),
    Fatal(String),
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
}

impl DialogItem {
    pub fn is_group(&self) -> bool {
        self.kind == ChatKind::Group
    }
}

/// The second line of a chat-list row: "Sender: " and a media caption or service text
/// in the accent color, then the text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastLine {
    pub prefix: Option<String>,
    pub accent: Option<String>,
    pub text: String,
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
        let (media, service, _) = message_content(store, m, is_channel);
        let text = row_text(m).lines().next().unwrap_or("").to_string();
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
            },
            (None, Some(label)) if text.is_empty() => LastLine {
                prefix,
                accent: Some(label),
                text: String::new(),
            },
            _ => LastLine {
                prefix,
                accent: None,
                text,
            },
        }
    });
    let last_read = d
        .top
        .as_ref()
        .filter(|m| m.out && !is_saved && !is_channel)
        .map(|m| m.id <= d.dialog.read_outbox_max_id);
    let default_mute = match kind {
        ChatKind::User | ChatKind::Bot => ctx.mute_users,
        ChatKind::Group => ctx.mute_chats,
        ChatKind::Channel => ctx.mute_broadcasts,
    };
    let mute_until = d.dialog.mute_until.map_or(default_mute, i64::from);
    let muted = mute_until > ctx.now;
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
            let key = if m.out {
                PeerKey::user(ctx.self_id)
            } else {
                m.from.unwrap_or(peer)
            };
            let author = authors
                .entry(key)
                .or_insert_with(|| author_of(store, key))
                .clone();
            let sender = (is_group && !m.out)
                .then(|| m.from.map(|from| (peer_name(store, from), author.color)))
                .flatten();
            // Calls are bubbles with their caption, not service lines.
            let call = info.service.as_ref().is_some_and(present::is_call);
            MessageItem {
                id: m.id,
                out: m.out,
                date: m.date,
                text: if call {
                    service.clone().unwrap_or_default()
                } else {
                    row_text(m)
                },
                media,
                service: service.filter(|_| !call),
                edited: info.edited,
                read: (m.out && !is_saved && !is_channel).then_some(m.id <= read_outbox),
                time: present::message_time(m.date),
                sender,
                author,
            }
        })
        .collect()
}

const DIALOGS: usize = 200;
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

fn userpic_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
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
pub fn start(
    config: Config,
    paths: Paths,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<Update>,
) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (up_tx, up_rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("data".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                if let Err(e) = run(config, paths, cmd_rx, up_tx.clone()).await {
                    error!("data thread: {e:#}");
                    let _ = up_tx.send(Update::Fatal(format!("{e:#}")));
                }
            });
        })
        .expect("spawn data thread");
    (cmd_tx, up_rx)
}

fn send_dialogs(store: &Storage, tx: &mpsc::UnboundedSender<Update>) -> anyhow::Result<()> {
    let list = store.dialogs(0, DIALOGS)?;
    let ctx = Context::load(store);
    let items = list.iter().map(|d| dialog_item(store, d, &ctx)).collect();
    let _ = tx.send(Update::Dialogs(items));
    Ok(())
}

fn send_history(
    store: &Storage,
    peer: PeerKey,
    count: usize,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    let h = store.history(peer, count)?;
    let _ = tx.send(Update::History {
        peer,
        messages: message_items(store, peer, &h.messages),
        complete: h.complete,
    });
    Ok(())
}

fn changed(a: &Applied) -> bool {
    !a.new_messages.is_empty()
        || a.edited > 0
        || a.deleted > 0
        || a.reloaded
        || !a.draft_changes.is_empty()
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
) -> anyhow::Result<()> {
    // 1. The cached chat list, before any network.
    if let Some(dir) = paths.cache.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let store = Storage::open(&paths.cache)?;
    send_dialogs(&store, &tx)?;
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
    let save = |client: &Client| {
        if let Err(e) = client.save_session(&paths.session) {
            error!("session save: {e}");
        }
    };

    // 2. Catch up with the server.
    let _ = tx.send(Update::Status("Updating…".into()));
    let mut sync = Sync::new(client.clone(), store)?;
    let started = std::time::Instant::now();
    let applied = sync.start().await?;
    info!(elapsed = ?started.elapsed(), "synced");
    save(&client);
    send_dialogs(sync.store(), &tx)?;
    send_draft_changes(&applied, &tx);
    let _ = tx.send(Update::Status(String::new()));
    // When to save drafts to the cloud. Drafts left unsaved last time (the app quit
    // first) go up right away.
    let mut cloud_save_at = sync
        .has_unsaved_drafts()
        .then(|| tokio::time::Instant::now() + SAVE_CLOUD_DRAFT_DELAY);

    // Userpics: from the file cache, or downloaded in the background (a few at once).
    let mut files = FileCache::open(&paths.files, FileCacheConfig::default())?;
    let (pics_tx, mut pics_rx) = mpsc::unbounded_channel::<(PeerKey, i64, Option<Vec<u8>>)>();
    let downloads = Arc::new(tokio::sync::Semaphore::new(4));
    let mut loading: HashSet<i64> = HashSet::new();

    // 3. Commands and live updates.
    let mut open: Option<PeerKey> = None;
    // How many messages of the open chat are shown (grows as the user scrolls up).
    let mut count = HISTORY;
    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Command::OpenChat(peer) => {
                        open = Some(peer);
                        count = open_count(sync.store(), peer);
                        send_history(sync.store(), peer, count, &tx)?;
                        match sync.history(peer, count).await {
                            Ok(_) => send_history(sync.store(), peer, count, &tx)?,
                            Err(e) => error!("history: {e}"),
                        }
                        send_dialogs(sync.store(), &tx)?;
                        // Supergroups and channels: the member count is in the full info.
                        match sync.member_count(peer).await {
                            Ok(Some(n)) => {
                                let text = members_text(sync.store(), peer, n);
                                let _ = tx.send(Update::Members { peer, text });
                            }
                            Ok(None) => {}
                            Err(e) => error!("full info: {e}"),
                        }
                    }
                    Command::Read { peer, max_id } => match sync.read_history(peer, max_id).await {
                        Ok(_) => send_dialogs(sync.store(), &tx)?,
                        Err(e) => error!("read: {e}"),
                    },
                    Command::LoadUserpic { peer, photo_id, dc_id } => {
                        let key = format!("userpic:{photo_id}:small");
                        if let Some(image) = files.get(&key)?.as_deref().and_then(userpic_image) {
                            let _ = tx.send(Update::Userpic { peer, photo_id, image });
                        } else if loading.insert(photo_id)
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
                    Command::CloseChat => open = None,
                    Command::SaveDraft { peer, text, cloud } => {
                        if sync.set_draft(peer, &text)? {
                            send_dialogs(sync.store(), &tx)?;
                        }
                        if cloud && sync.has_unsaved_drafts() && cloud_save_at.is_none() {
                            cloud_save_at = Some(tokio::time::Instant::now() + SAVE_CLOUD_DRAFT_DELAY);
                        }
                    }
                    Command::LoadOlder(peer) => {
                        if open == Some(peer) {
                            count += HISTORY;
                            if let Err(e) = sync.history(peer, count).await {
                                error!("history: {e}");
                            }
                            send_history(sync.store(), peer, count, &tx)?;
                        }
                    }
                    Command::Send { peer, text } => {
                        match sync.send_text(peer, &text).await {
                            Ok(_) => {
                                send_dialogs(sync.store(), &tx)?;
                                if open == Some(peer) {
                                    send_history(sync.store(), peer, count, &tx)?;
                                }
                            }
                            Err(e) => {
                                error!("send: {e}");
                                let _ = tx.send(Update::Status(format!("Not sent: {e}")));
                            }
                        }
                    }
                }
            }
            Some((peer, photo_id, bytes)) = pics_rx.recv() => {
                loading.remove(&photo_id);
                if let Some(bytes) = bytes {
                    files.put(&format!("userpic:{photo_id}:small"), &bytes)?;
                    if let Some(image) = userpic_image(&bytes) {
                        let _ = tx.send(Update::Userpic { peer, photo_id, image });
                    }
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
                if changed(&applied) {
                    send_dialogs(sync.store(), &tx)?;
                    send_draft_changes(&applied, &tx);
                    if let Some(peer) = open {
                        let touched = applied.new_messages.iter().any(|m| m.peer == peer)
                            || applied.edited + applied.deleted > 0
                            || applied.marks_changed;
                        if touched {
                            send_history(sync.store(), peer, count, &tx)?;
                        }
                    }
                    save(&client);
                }
            }
        }
    }
    save(&client);
    Ok(())
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
pub fn start_demo() -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<Update>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (up_tx, up_rx) = mpsc::unbounded_channel();
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
            false,
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
                last: last.clone(),
                last_read: *read,
                time: present::dialogs_date(*date, now),
                unread: *unread,
                muted: *muted,
                pinned: *pinned,
                draft: None,
                // The last few messages of each chat are unread.
                read_inbox_max_id: 1005,
                status: match kind {
                    ChatKind::Group => "906 members".into(),
                    ChatKind::Channel => "8 subscribers".into(),
                    ChatKind::Bot => "bot".into(),
                    ChatKind::User => "last seen recently".into(),
                },
            },
        )
        .collect();
    let _ = up_tx.send(Update::Dialogs(dialogs));
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
                }
            })
            .collect();
        // Older demo messages appear 20 at a time when the user scrolls up.
        let mut older_left = 60;
        let mut next_id = 2000;
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                Command::OpenChat(_)
                | Command::CloseChat
                | Command::SaveDraft { .. }
                | Command::Read { .. }
                | Command::LoadUserpic { .. } => {}
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
                            }
                        })
                        .collect();
                    older_left -= 20;
                    history.splice(0..0, page);
                }
                Command::LoadOlder(_) => {}
                Command::Send { text, .. } => {
                    next_id += 1;
                    let date = Local::now().timestamp() as i32;
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
                let _ = up_tx.send(Update::History {
                    peer: PeerKey::user(i + 1),
                    messages,
                    complete: older_left == 0,
                });
            }
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
