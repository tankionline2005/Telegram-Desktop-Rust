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
    Send { peer: PeerKey, text: String },
}

#[derive(Debug, Clone)]
pub enum Update {
    Dialogs(Vec<DialogItem>),
    History {
        peer: PeerKey,
        messages: Vec<MessageItem>,
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
}

#[derive(Debug, Clone)]
pub struct MessageItem {
    pub out: bool,
    pub text: String,
    pub time: String,
    /// Sender name and color in groups (incoming messages only).
    pub sender: Option<(String, u8)>,
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

fn dialog_item(d: &DialogView) -> DialogItem {
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
            MessageItem {
                out: m.out,
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
    let _ = tx.send(Update::Dialogs(list.iter().map(dialog_item).collect()));
    Ok(())
}

fn send_history(
    store: &Storage,
    peer: PeerKey,
    tx: &mpsc::UnboundedSender<Update>,
) -> anyhow::Result<()> {
    let h = store.history(peer, HISTORY)?;
    let _ = tx.send(Update::History {
        peer,
        messages: message_items(store, peer, &h.messages),
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
    loop {
        tokio::select! {
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Command::OpenChat(peer) => {
                        open = Some(peer);
                        send_history(sync.store(), peer, &tx)?;
                        match sync.history(peer, HISTORY).await {
                            Ok(_) => send_history(sync.store(), peer, &tx)?,
                            Err(e) => error!("history: {e}"),
                        }
                        send_dialogs(sync.store(), &tx)?;
                    }
                    Command::Send { peer, text } => {
                        match sync.send_text(peer, &text).await {
                            Ok(_) => {
                                send_dialogs(sync.store(), &tx)?;
                                if open == Some(peer) {
                                    send_history(sync.store(), peer, &tx)?;
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
                            send_history(sync.store(), peer, &tx)?;
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
        })
        .collect();
    let _ = up_tx.send(Update::Dialogs(dialogs));
    std::thread::spawn(move || {
        let mut history: Vec<MessageItem> = [
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
            (false, "See you tomorrow at 10!"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (out, text))| MessageItem {
            out,
            text: text.to_string(),
            time: format!("10:{:02}", i * 3),
            sender: None,
        })
        .collect();
        while let Some(cmd) = cmd_rx.blocking_recv() {
            match cmd {
                Command::OpenChat(peer) => {
                    let _ = up_tx.send(Update::History {
                        peer,
                        messages: history.clone(),
                    });
                }
                Command::Send { peer, text } => {
                    history.push(MessageItem {
                        out: true,
                        text,
                        time: "now".into(),
                        sender: None,
                    });
                    let _ = up_tx.send(Update::History {
                        peer,
                        messages: history.clone(),
                    });
                }
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
