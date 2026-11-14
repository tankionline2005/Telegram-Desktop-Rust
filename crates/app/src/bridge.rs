//! The data side of the app: a background thread with its own async runtime that owns
//! the network client and the cache. The UI thread never waits for the network or the
//! disk; it sends [`Command`]s and receives ready-to-draw [`Update`]s.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{Local, TimeZone};
use tdr_api::{Client, Config, SessionData};
use tdr_data::tdr_storage::{DialogView, MessageRow, PeerKey, PeerKind, Storage};
use tdr_data::{Applied, Sync};
use tokio::sync::mpsc;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub enum Command {
    OpenChat(PeerKey),
    /// The user scrolled to the top of the open chat: load older messages.
    LoadOlder(PeerKey),
    Send {
        peer: PeerKey,
        text: String,
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
    /// Connection state for the header ("Connecting…"), empty when online.
    Status(String),
    Fatal(String),
}

#[derive(Debug, Clone)]
pub struct DialogItem {
    pub peer: PeerKey,
    pub name: String,
    pub initials: String,
    /// Index into the userpic colors (tdesktop's 7 peer colors).
    pub color: u8,
    pub last: String,
    pub time: String,
    pub unread: i32,
    pub pinned: bool,
    pub is_channel: bool,
    /// The chat with yourself: "Saved Messages" with the bookmark userpic.
    pub is_saved: bool,
    /// A basic group or supergroup (sender names and avatars in the feed).
    pub is_group: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageItem {
    pub id: i32,
    pub out: bool,
    pub text: String,
    pub time: String,
    /// Unix time, for grouping consecutive messages of one author.
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

fn time_label(date: i32) -> String {
    let Some(t) = Local.timestamp_opt(date as i64, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else if (now - t).num_days() < 7 {
        t.format("%a").to_string()
    } else {
        t.format("%d.%m.%y").to_string()
    }
}

fn dialog_item(d: &DialogView, self_id: i64) -> DialogItem {
    let name = d
        .peer
        .as_ref()
        .map_or_else(|| "…".to_string(), |p| p.name.clone());
    let last = d
        .top
        .as_ref()
        .map(|m| m.text.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();
    DialogItem {
        peer: d.dialog.peer,
        initials: initials(&name),
        color: color_index(d.dialog.peer.id),
        name,
        last,
        time: d
            .top
            .as_ref()
            .map(|m| time_label(m.date))
            .unwrap_or_default(),
        unread: d.dialog.unread_count,
        pinned: d.dialog.pinned,
        is_channel: d.dialog.peer.kind == PeerKind::Channel
            && !d.peer.as_ref().is_some_and(|p| p.is_megagroup),
        is_saved: d.dialog.peer == PeerKey::user(self_id),
        is_group: d.dialog.peer.kind == PeerKind::Chat
            || (d.dialog.peer.kind == PeerKind::Channel
                && d.peer.as_ref().is_some_and(|p| p.is_megagroup)),
    }
}

fn message_items(store: &Storage, peer: PeerKey, messages: &[MessageRow]) -> Vec<MessageItem> {
    let is_group = match peer.kind {
        PeerKind::Chat => true,
        PeerKind::Channel => store
            .peer(peer)
            .ok()
            .flatten()
            .is_some_and(|p| p.is_megagroup),
        PeerKind::User => false,
    };
    let self_id = store.meta_i64("self_id").ok().flatten().unwrap_or(0);
    // Oldest first for display.
    messages
        .iter()
        .rev()
        .map(|m| {
            let sender = (is_group && !m.out)
                .then_some(m.from)
                .flatten()
                .map(|from| {
                    let name = store
                        .peer(from)
                        .ok()
                        .flatten()
                        .map_or_else(|| "…".into(), |p| p.name);
                    (name, color_index(from.id))
                });
            let key = if m.out {
                Some(PeerKey::user(self_id))
            } else {
                m.from.or(Some(peer))
            };
            let name = key
                .and_then(|k| store.peer(k).ok().flatten())
                .map_or_else(String::new, |p| p.name);
            let author = Author {
                key,
                initials: initials(&name),
                color: key.map_or(0, |k| color_index(k.id)),
            };
            MessageItem {
                id: m.id,
                out: m.out,
                date: m.date,
                author,
                text: m.text.clone(),
                time: Local
                    .timestamp_opt(m.date as i64, 0)
                    .single()
                    .map(|t| t.format("%H:%M").to_string())
                    .unwrap_or_default(),
                sender,
            }
        })
        .collect()
}

const DIALOGS: usize = 200;
const HISTORY: usize = 60;

pub struct Paths {
    pub session: PathBuf,
    pub cache: PathBuf,
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
    let self_id = store.meta_i64("self_id")?.unwrap_or(0);
    let items = list
        .iter()
        .map(|d| {
            let mut item = dialog_item(d, self_id);
            if item.is_saved {
                item.name = "Saved Messages".into();
            }
            item
        })
        .collect();
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
    !a.new_messages.is_empty() || a.edited > 0 || a.deleted > 0 || a.reloaded
}

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
        Ok(d) => SessionData::from_bytes(&d)?,
        Err(_) => anyhow::bail!(
            "Not logged in. Run `tdr login` once (the console client), then start the app again."
        ),
    };
    if !session.is_authorized() {
        anyhow::bail!("Not logged in. Run `tdr login` once, then start the app again.");
    }
    let client = Client::new(config, session);
    let save = |client: &Client| {
        let _ = std::fs::write(&paths.session, client.session_bytes());
    };

    // 2. Catch up with the server.
    let _ = tx.send(Update::Status("Updating…".into()));
    let mut sync = Sync::new(client.clone(), store)?;
    let started = std::time::Instant::now();
    let applied = sync.start().await?;
    info!(elapsed = ?started.elapsed(), "synced");
    save(&client);
    send_dialogs(sync.store(), &tx)?;
    let _ = tx.send(Update::Status(String::new()));
    let _ = applied;

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
                        count = HISTORY;
                        send_history(sync.store(), peer, count, &tx)?;
                        match sync.history(peer, count).await {
                            Ok(_) => send_history(sync.store(), peer, count, &tx)?,
                            Err(e) => error!("history: {e}"),
                        }
                        send_dialogs(sync.store(), &tx)?;
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
            event = sync.wait_event(Duration::from_secs(5)) => {
                let applied = sync.process(event).await?;
                if changed(&applied) {
                    send_dialogs(sync.store(), &tx)?;
                    if let Some(peer) = open {
                        let touched = applied.new_messages.iter().any(|m| m.peer == peer)
                            || applied.edited + applied.deleted > 0;
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
        }
    } else {
        Author {
            key: Some(PeerKey::user(2)),
            initials: "BO".into(),
            color: color_index(2),
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
    let names = [
        ("Alice Johnson", "See you tomorrow at 10!", 2, true),
        (
            "Rust Developers",
            "Bob: the new borrow checker is great",
            14,
            false,
        ),
        ("Saved Messages", "build: 3.2 s → 0.9 s", 0, true),
        ("Telegram Tips", "Pin your important chats", 0, false),
        ("Mom", "Call me when you can ❤", 1, false),
        (
            "Weekend Hiking",
            "Carol: photos are in the album",
            37,
            false,
        ),
        ("Daniel", "ok", 0, false),
        ("Design Team", "Eve: new icons attached", 3, false),
    ];
    let dialogs: Vec<DialogItem> = names
        .iter()
        .enumerate()
        .map(|(i, (name, last, unread, pinned))| DialogItem {
            peer: PeerKey::user(i as i64 + 1),
            name: name.to_string(),
            initials: initials(name),
            color: color_index(i as i64 + 1),
            last: last.to_string(),
            time: format!("{}:{:02}", 9 + i, (i * 7) % 60),
            unread: *unread,
            pinned: *pinned,
            is_channel: false,
            is_saved: *name == "Saved Messages",
            is_group: *name == "Rust Developers"
                || *name == "Weekend Hiking"
                || *name == "Design Team",
        })
        .collect();
    let _ = up_tx.send(Update::Dialogs(dialogs));
    std::thread::spawn(move || {
        let mut history: Vec<MessageItem> = [
            (false, "Hey there"),
            (false, "Hi! Did you try the new client?"),
            (
                true,
                "Yes — the chat list opens from the cache in about 5 ms.",
            ),
            (false, "That's fast. And the colors?"),
            (
                true,
                "Taken straight from tdesktop's palette, day and night.",
            ),
            (false, "https://example.com/very/long/link/without/any/spaces?utm_source=demo&utm_medium=social&utm_content=link_in_bio&fbclid=PAZnRzaAVi_zNleHRuA2FlbQIxMQBwZG9mAmZkaWQWUSFgPoFLDSR14gI_z30rz-irR2s_T3NydGMGYXBwX2lkDzEyNDAyNDU3NDI4NzQxNAABp"),
            (true, "A long paragraph to check wrapping: Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur."),
            (false, "0101001101001101010110000100101101001001010011100101001101010000010000010100001101000101010100001001001100010101010100111001010100"),
            (false, "See you tomorrow at 10!"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (out, text))| MessageItem {
            id: 1000 + i as i32,
            out,
            text: text.to_string(),
            time: format!("10:{:02}", i * 3),
            date: 1_700_000_000 + i as i32 * 60,
            sender: None,
            author: demo_author(out),
        })
        .collect();
        // Older demo messages appear 20 at a time when the user scrolls up.
        let mut older_left = 60;
        let mut next_id = 2000;
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                Command::OpenChat(_) => {}
                Command::LoadOlder(_) if older_left > 0 => {
                    let first = history.first().map_or(1000, |m| m.id);
                    let page: Vec<MessageItem> = (1..=20)
                        .rev()
                        .map(|k| MessageItem {
                            id: first - k,
                            out: k % 3 == 0,
                            text: format!("Older message #{}", older_left - 20 + k),
                            time: "09:00".into(),
                            date: 1_699_990_000 - k * 60,
                            sender: None,
                            author: demo_author(k % 3 == 0),
                        })
                        .collect();
                    older_left -= 20;
                    history.splice(0..0, page);
                }
                Command::LoadOlder(_) => {}
                Command::Send { text, .. } => {
                    next_id += 1;
                    history.push(MessageItem {
                        id: next_id,
                        out: true,
                        text,
                        time: "now".into(),
                        date: 1_800_000_000,
                        sender: None,
                        author: demo_author(true),
                    });
                }
            }
            for i in 0..8 {
                let mut messages = history.clone();
                // Groups (Rust Developers, Weekend Hiking, Design Team) show sender names.
                if matches!(i + 1, 2 | 6 | 8) {
                    for m in messages.iter_mut().filter(|m| !m.out) {
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
