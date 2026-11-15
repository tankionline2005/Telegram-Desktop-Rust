//! Local cache on SQLite.
//!
//! tdesktop keeps no chat list, peers or messages on disk: every start waits for
//! `messages.getDialogs`. This cache lets the chat list appear before the network
//! answers and lets the update state continue exactly where it stopped.
//!
//! Rows are our own (`PeerRow`, `MessageRow`, `DialogRow`): the columns needed for
//! lists and lookups, plus the original TL object as a blob tagged with its layer. A new
//! upstream layer therefore never breaks the cache: blobs of an older layer are simply
//! ignored until refreshed.
//!
//! Writes go through [`Storage::write`], one transaction per batch, together with the
//! update state they correspond to.

pub mod files;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub use files::{FileCache, FileCacheConfig, FileCacheStats};
pub use rusqlite::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Schema version (`PRAGMA user_version`); bump with a migration in [`migrate`].
const SCHEMA_VERSION: i32 = 4;

/// Message ids above this are made by the client (tdesktop's `ServerMaxMsgId`), e.g.
/// service notifications that the server sends as updates rather than messages.
pub const SERVER_MAX_MESSAGE_ID: i32 = 0x3FFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerKind {
    User = 0,
    Chat = 1,
    Channel = 2,
}

impl PeerKind {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Chat,
            2 => Self::Channel,
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerKey {
    pub kind: PeerKind,
    pub id: i64,
}

impl PeerKey {
    pub fn user(id: i64) -> Self {
        Self {
            kind: PeerKind::User,
            id,
        }
    }
    pub fn chat(id: i64) -> Self {
        Self {
            kind: PeerKind::Chat,
            id,
        }
    }
    pub fn channel(id: i64) -> Self {
        Self {
            kind: PeerKind::Channel,
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeerRow {
    pub key: PeerKey,
    pub access_hash: Option<i64>,
    /// A "min" object (incomplete, e.g. a user seen only in a group): never replaces a
    /// full one.
    pub is_min: bool,
    pub name: String,
    pub username: Option<String>,
    /// Channel that is a supergroup.
    pub is_megagroup: bool,
    /// The TL object and the layer it was serialized with.
    pub raw: Option<(i32, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub peer: PeerKey,
    pub id: i32,
    pub date: i32,
    pub from: Option<PeerKey>,
    pub out: bool,
    pub text: String,
    pub raw: Option<(i32, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DialogRow {
    pub peer: PeerKey,
    pub top_message: i32,
    pub top_date: i32,
    pub pinned: bool,
    pub folder_id: i32,
    pub unread_count: i32,
    /// For channels: the pts of the channel's update sequence.
    pub pts: Option<i32>,
    /// Incoming messages up to this id are read.
    pub read_inbox_max_id: i32,
    /// Outgoing messages up to this id are read by the other side (✓✓).
    pub read_outbox_max_id: i32,
    /// Muted until this Unix time; `None`: the default for the chat type.
    pub mute_until: Option<i32>,
}

/// A chat-list entry with what is needed to show it.
#[derive(Debug, Clone)]
pub struct DialogView {
    pub dialog: DialogRow,
    pub peer: Option<PeerRow>,
    pub top: Option<MessageRow>,
    /// The local draft: text and date.
    pub draft: Option<(String, i32)>,
}

/// See [`Storage::history`].
#[derive(Debug, Clone)]
pub struct CachedHistory {
    /// Newest first.
    pub messages: Vec<MessageRow>,
    /// The oldest id of the contiguous cached segment (`None`: nothing cached).
    pub cached_from: Option<i32>,
    /// The segment reaches the first message of the chat.
    pub complete: bool,
}

/// The common update state, see `tdr_updates::State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateState {
    pub pts: i32,
    pub qts: i32,
    pub date: i32,
    pub seq: i32,
}

pub struct Storage {
    conn: Connection,
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    if version < 1 {
        create_v1(&tx)?;
    }
    if version < 2 {
        // The cached part of each chat's history: all messages with ids in
        // [cached_from, top_message] are present.
        tx.execute_batch(
            "ALTER TABLE dialogs ADD COLUMN cached_from INTEGER;
             ALTER TABLE dialogs ADD COLUMN history_complete INTEGER NOT NULL DEFAULT 0;
             -- Chat-list loads always stored the top message: it starts the segment.
             UPDATE dialogs SET cached_from = top_message
             WHERE EXISTS (SELECT 1 FROM messages m
                           WHERE m.kind = dialogs.kind AND m.peer_id = dialogs.id AND m.id = dialogs.top_message);",
        )?;
    }
    if version < 3 {
        // Local message drafts (what the input field of each chat holds).
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS drafts (
                kind INTEGER NOT NULL,
                id   INTEGER NOT NULL,
                text TEXT NOT NULL,
                date INTEGER NOT NULL,
                PRIMARY KEY (kind, id)
             ) WITHOUT ROWID;",
        )?;
    }
    if version < 4 {
        // Read state (check marks, the "Unread messages" bar) and mute settings
        // (NULL: the default of the chat type).
        tx.execute_batch(
            "ALTER TABLE dialogs ADD COLUMN read_inbox_max_id INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE dialogs ADD COLUMN read_outbox_max_id INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE dialogs ADD COLUMN mute_until INTEGER;",
        )?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()
}

fn create_v1(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value BLOB NOT NULL
        ) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS peers (
            kind         INTEGER NOT NULL,
            id           INTEGER NOT NULL,
            access_hash  INTEGER,
            is_min       INTEGER NOT NULL,
            is_megagroup INTEGER NOT NULL,
            name         TEXT NOT NULL,
            username     TEXT,
            raw_layer    INTEGER,
            raw          BLOB,
            PRIMARY KEY (kind, id)
        ) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS dialogs (
            kind         INTEGER NOT NULL,
            id           INTEGER NOT NULL,
            top_message  INTEGER NOT NULL,
            top_date     INTEGER NOT NULL,
            pinned       INTEGER NOT NULL,
            folder_id    INTEGER NOT NULL,
            unread_count INTEGER NOT NULL,
            pts          INTEGER,
            PRIMARY KEY (kind, id)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS dialogs_order ON dialogs (folder_id, pinned DESC, top_date DESC);

        CREATE TABLE IF NOT EXISTS messages (
            kind      INTEGER NOT NULL,
            peer_id   INTEGER NOT NULL,
            id        INTEGER NOT NULL,
            date      INTEGER NOT NULL,
            from_kind INTEGER,
            from_id   INTEGER,
            out       INTEGER NOT NULL,
            text      TEXT NOT NULL,
            raw_layer INTEGER,
            raw       BLOB,
            PRIMARY KEY (kind, peer_id, id)
        ) WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS channel_pts (
            id  INTEGER PRIMARY KEY,
            pts INTEGER NOT NULL
        );
        ",
    )
}

impl Storage {
    /// Opens (or creates) the cache at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        // WAL: readers never wait for the writer; NORMAL sync is safe with WAL and avoids
        // an fsync per transaction.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = OFF;",
        )?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// Runs `f` in one transaction.
    pub fn write<T>(&mut self, f: impl FnOnce(&Writer<'_>) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction()?;
        let result = f(&Writer { tx: &tx })?;
        tx.commit()?;
        Ok(result)
    }

    pub fn update_state(&self) -> Result<Option<UpdateState>> {
        let raw: Option<Vec<u8>> = self.meta("update_state")?;
        Ok(raw.filter(|r| r.len() == 16).map(|r| {
            let i = |n: usize| i32::from_le_bytes(r[n * 4..n * 4 + 4].try_into().unwrap());
            UpdateState {
                pts: i(0),
                qts: i(1),
                date: i(2),
                seq: i(3),
            }
        }))
    }

    pub fn channel_pts(&self) -> Result<Vec<(i64, i32)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, pts FROM channel_pts")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.conn
            .prepare_cached("SELECT value FROM meta WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()
    }

    pub fn meta_i64(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .meta(key)?
            .filter(|v| v.len() == 8)
            .map(|v| i64::from_le_bytes(v.try_into().unwrap())))
    }

    pub fn peer(&self, key: PeerKey) -> Result<Option<PeerRow>> {
        self.conn
            .prepare_cached(
                "SELECT kind, id, access_hash, is_min, is_megagroup, name, username, raw_layer, raw
                 FROM peers WHERE kind = ?1 AND id = ?2",
            )?
            .query_row(params![key.kind as i64, key.id], read_peer)
            .optional()
    }

    pub fn access_hash(&self, key: PeerKey) -> Result<Option<i64>> {
        Ok(self.peer(key)?.and_then(|p| p.access_hash))
    }

    pub fn dialog_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM dialogs", [], |r| r.get(0))
    }

    /// The cached history segment of a chat: up to `limit` newest messages known to be
    /// contiguous, newest first, and whether the segment reaches the start of the chat.
    pub fn history(&self, peer: PeerKey, limit: usize) -> Result<CachedHistory> {
        let seg: Option<(i32, Option<i32>, bool)> = self
            .conn
            .prepare_cached(
                "SELECT top_message, cached_from, history_complete FROM dialogs WHERE kind = ?1 AND id = ?2",
            )?
            .query_row(params![peer.kind as i64, peer.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((top, Some(from), complete)) = seg else {
            return Ok(CachedHistory {
                messages: vec![],
                cached_from: None,
                complete: false,
            });
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, date, from_kind, from_id, out, text, raw_layer, raw FROM messages
             WHERE kind = ?1 AND peer_id = ?2 AND id BETWEEN ?3 AND ?4
             ORDER BY id DESC LIMIT ?5",
        )?;
        let messages = stmt
            .query_map(
                params![peer.kind as i64, peer.id, from, top, limit as i64],
                |r| {
                    Ok(MessageRow {
                        peer,
                        id: r.get(0)?,
                        date: r.get(1)?,
                        from: peer_key(r.get(2)?, r.get(3)?),
                        out: r.get(4)?,
                        text: r.get(5)?,
                        raw: raw(r.get(6)?, r.get(7)?),
                    })
                },
            )?
            .collect::<Result<Vec<_>>>()?;
        Ok(CachedHistory {
            messages,
            cached_from: Some(from),
            complete,
        })
    }

    /// The chat list of `folder_id` (0 = main), pinned first, then by last message.
    /// All local drafts: chat, text, date.
    pub fn drafts(&self) -> Result<Vec<(PeerKey, String, i32)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT kind, id, text, date FROM drafts")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                PeerKey {
                    kind: PeerKind::from_i64(r.get(0)?),
                    id: r.get(1)?,
                },
                r.get(2)?,
                r.get(3)?,
            ))
        })?;
        rows.collect()
    }

    /// Unread count and read marks of one chat: (unread, read_inbox_max_id,
    /// read_outbox_max_id).
    pub fn read_marks(&self, peer: PeerKey) -> Result<Option<(i32, i32, i32)>> {
        self.conn
            .prepare_cached(
                "SELECT unread_count, read_inbox_max_id, read_outbox_max_id FROM dialogs
                 WHERE kind = ?1 AND id = ?2",
            )?
            .query_row(params![peer.kind as i64, peer.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
    }

    pub fn dialogs(&self, folder_id: i32, limit: usize) -> Result<Vec<DialogView>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT d.kind, d.id, d.top_message, d.top_date, d.pinned, d.folder_id, d.unread_count, d.pts,
                    p.kind, p.id, p.access_hash, p.is_min, p.is_megagroup, p.name, p.username, p.raw_layer, p.raw,
                    m.date, m.from_kind, m.from_id, m.out, m.text, m.raw_layer, m.raw,
                    dr.text, dr.date,
                    d.read_inbox_max_id, d.read_outbox_max_id, d.mute_until
             FROM dialogs d
             LEFT JOIN peers p ON p.kind = d.kind AND p.id = d.id
             LEFT JOIN messages m ON m.kind = d.kind AND m.peer_id = d.id AND m.id = d.top_message
             LEFT JOIN drafts dr ON dr.kind = d.kind AND dr.id = d.id
             WHERE d.folder_id = ?1
             -- A draft lifts its chat like a new message (tdesktop's adjustedChatListTimeId).
             ORDER BY d.pinned DESC, MAX(d.top_date, COALESCE(dr.date, 0)) DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![folder_id, limit as i64], |r| {
            let dialog = DialogRow {
                peer: PeerKey {
                    kind: PeerKind::from_i64(r.get(0)?),
                    id: r.get(1)?,
                },
                top_message: r.get(2)?,
                top_date: r.get(3)?,
                pinned: r.get(4)?,
                folder_id: r.get(5)?,
                unread_count: r.get(6)?,
                pts: r.get(7)?,
                read_inbox_max_id: r.get(26)?,
                read_outbox_max_id: r.get(27)?,
                mute_until: r.get(28)?,
            };
            let peer = match r.get::<_, Option<i64>>(8)? {
                Some(_) => Some(read_peer_at(r, 8)?),
                None => None,
            };
            let top = match r.get::<_, Option<i32>>(17)? {
                Some(date) => Some(MessageRow {
                    peer: dialog.peer,
                    id: dialog.top_message,
                    date,
                    from: peer_key(r.get(18)?, r.get(19)?),
                    out: r.get(20)?,
                    text: r.get(21)?,
                    raw: raw(r.get(22)?, r.get(23)?),
                }),
                None => None,
            };
            let draft = match r.get::<_, Option<String>>(24)? {
                Some(text) => Some((text, r.get(25)?)),
                None => None,
            };
            Ok(DialogView {
                dialog,
                peer,
                top,
                draft,
            })
        })?;
        rows.collect()
    }
}

fn raw(layer: Option<i32>, data: Option<Vec<u8>>) -> Option<(i32, Vec<u8>)> {
    Some((layer?, data?))
}

fn peer_key(kind: Option<i64>, id: Option<i64>) -> Option<PeerKey> {
    Some(PeerKey {
        kind: PeerKind::from_i64(kind?),
        id: id?,
    })
}

fn read_peer(r: &rusqlite::Row<'_>) -> rusqlite::Result<PeerRow> {
    read_peer_at(r, 0)
}

fn read_peer_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<PeerRow> {
    Ok(PeerRow {
        key: PeerKey {
            kind: PeerKind::from_i64(r.get(at)?),
            id: r.get(at + 1)?,
        },
        access_hash: r.get(at + 2)?,
        is_min: r.get(at + 3)?,
        is_megagroup: r.get(at + 4)?,
        name: r.get(at + 5)?,
        username: r.get(at + 6)?,
        raw: raw(r.get(at + 7)?, r.get(at + 8)?),
    })
}

/// Write operations inside one transaction.
pub struct Writer<'a> {
    tx: &'a Transaction<'a>,
}

impl Writer<'_> {
    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.tx
            .prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?
            .execute(params![key, value])?;
        Ok(())
    }

    pub fn set_meta_i64(&self, key: &str, value: i64) -> Result<()> {
        self.set_meta(key, &value.to_le_bytes())
    }

    pub fn set_update_state(&self, s: UpdateState) -> Result<()> {
        let mut v = Vec::with_capacity(16);
        for x in [s.pts, s.qts, s.date, s.seq] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        self.set_meta("update_state", &v)
    }

    pub fn set_channel_pts(&self, id: i64, pts: i32) -> Result<()> {
        self.tx
            .prepare_cached("INSERT OR REPLACE INTO channel_pts (id, pts) VALUES (?1, ?2)")?
            .execute(params![id, pts])?;
        Ok(())
    }

    /// Inserts or updates a peer. A "min" row never replaces a full one; a full row
    /// keeps a known access hash if the new object lacks one.
    pub fn put_peer(&self, p: &PeerRow) -> Result<()> {
        let (layer, data) = match &p.raw {
            Some((l, d)) => (Some(*l), Some(d.as_slice())),
            None => (None, None),
        };
        self.tx
            .prepare_cached(
                "INSERT INTO peers (kind, id, access_hash, is_min, is_megagroup, name, username, raw_layer, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (kind, id) DO UPDATE SET
                    access_hash  = COALESCE(excluded.access_hash, peers.access_hash),
                    is_min       = excluded.is_min,
                    is_megagroup = excluded.is_megagroup,
                    name         = excluded.name,
                    username     = excluded.username,
                    raw_layer    = excluded.raw_layer,
                    raw          = excluded.raw
                 WHERE excluded.is_min = 0 OR peers.is_min = 1",
            )?
            .execute(params![
                p.key.kind as i64,
                p.key.id,
                p.access_hash,
                p.is_min,
                p.is_megagroup,
                p.name,
                p.username,
                layer,
                data
            ])?;
        Ok(())
    }

    pub fn put_message(&self, m: &MessageRow) -> Result<()> {
        let (layer, data) = match &m.raw {
            Some((l, d)) => (Some(*l), Some(d.as_slice())),
            None => (None, None),
        };
        self.tx
            .prepare_cached(
                "INSERT OR REPLACE INTO messages (kind, peer_id, id, date, from_kind, from_id, out, text, raw_layer, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![
                m.peer.kind as i64,
                m.peer.id,
                m.id,
                m.date,
                m.from.map(|f| f.kind as i64),
                m.from.map(|f| f.id),
                m.out,
                m.text,
                layer,
                data
            ])?;
        Ok(())
    }

    /// Deletes messages of the common box (private chats and basic groups share ids).
    pub fn delete_common_messages(&self, ids: &[i32]) -> Result<()> {
        let mut stmt = self
            .tx
            .prepare_cached("DELETE FROM messages WHERE kind IN (0, 1) AND id = ?1")?;
        for id in ids {
            stmt.execute([id])?;
        }
        Ok(())
    }

    pub fn delete_messages(&self, peer: PeerKey, ids: &[i32]) -> Result<()> {
        let mut stmt = self
            .tx
            .prepare_cached("DELETE FROM messages WHERE kind = ?1 AND peer_id = ?2 AND id = ?3")?;
        for id in ids {
            stmt.execute(params![peer.kind as i64, peer.id, id])?;
        }
        Ok(())
    }

    pub fn delete_all_messages(&self, peer: PeerKey) -> Result<()> {
        self.tx
            .prepare_cached("DELETE FROM messages WHERE kind = ?1 AND peer_id = ?2")?
            .execute(params![peer.kind as i64, peer.id])?;
        Ok(())
    }

    /// Inserts or refreshes a chat-list entry (from `messages.getDialogs`). Its top
    /// message must be stored too. If the top message changed without us seeing the
    /// messages in between, the cached history segment restarts at the new top.
    /// Stores the local draft of a chat (`None` removes it).
    pub fn set_draft(&self, peer: PeerKey, draft: Option<(&str, i32)>) -> Result<()> {
        match draft {
            Some((text, date)) => self
                .tx
                .prepare_cached(
                    "INSERT INTO drafts (kind, id, text, date) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (kind, id) DO UPDATE SET text = excluded.text, date = excluded.date",
                )?
                .execute(params![peer.kind as i64, peer.id, text, date])?,
            None => self
                .tx
                .prepare_cached("DELETE FROM drafts WHERE kind = ?1 AND id = ?2")?
                .execute(params![peer.kind as i64, peer.id])?,
        };
        Ok(())
    }

    /// A new id for a message made by the client (above [`SERVER_MAX_MESSAGE_ID`]).
    pub fn next_local_message_id(&self) -> Result<i32> {
        let last: i64 = self
            .tx
            .query_row(
                "SELECT value FROM meta WHERE key = 'local_message_id'",
                [],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .and_then(|v| v.try_into().ok().map(i64::from_le_bytes))
            .unwrap_or(i64::from(SERVER_MAX_MESSAGE_ID));
        let next = last + 1;
        self.set_meta_i64("local_message_id", next)?;
        Ok(next as i32)
    }

    /// A client-made message with this date and text exists (the same service
    /// notification can come twice: in a difference and live).
    pub fn has_local_message(&self, peer: PeerKey, date: i32, text: &str) -> Result<bool> {
        self.tx
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM messages WHERE kind = ?1 AND peer_id = ?2
                                AND id > ?3 AND date = ?4 AND text = ?5)",
            )?
            .query_row(
                params![peer.kind as i64, peer.id, SERVER_MAX_MESSAGE_ID, date, text],
                |r| r.get(0),
            )
    }

    /// Stores a peer unless one is already known (e.g. the "Telegram" service account
    /// for notifications that arrive before its user object).
    pub fn put_peer_if_missing(&self, p: &PeerRow) -> Result<()> {
        let known: bool = self
            .tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM peers WHERE kind = ?1 AND id = ?2)")?
            .query_row(params![p.key.kind as i64, p.key.id], |r| r.get(0))?;
        if known { Ok(()) } else { self.put_peer(p) }
    }

    pub fn put_dialog(&self, d: &DialogRow) -> Result<()> {
        // A newer client-made top message (a service notification) survives refreshes
        // that bring the server's older top message.
        self.tx
            .prepare_cached(
                "INSERT INTO dialogs (kind, id, top_message, top_date, pinned, folder_id, unread_count, pts, cached_from,
                                      read_inbox_max_id, read_outbox_max_id, mute_until)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?3, ?10, ?11, ?12)
                 ON CONFLICT (kind, id) DO UPDATE SET
                    read_inbox_max_id  = MAX(dialogs.read_inbox_max_id, excluded.read_inbox_max_id),
                    read_outbox_max_id = MAX(dialogs.read_outbox_max_id, excluded.read_outbox_max_id),
                    mute_until       = excluded.mute_until,
                    cached_from      = CASE WHEN dialogs.top_message = excluded.top_message
                                              OR (dialogs.top_message > ?9 AND dialogs.top_date >= excluded.top_date)
                                            THEN dialogs.cached_from ELSE excluded.top_message END,
                    history_complete = CASE WHEN dialogs.top_message = excluded.top_message
                                              OR (dialogs.top_message > ?9 AND dialogs.top_date >= excluded.top_date)
                                            THEN dialogs.history_complete ELSE 0 END,
                    top_message      = CASE WHEN dialogs.top_message > ?9 AND dialogs.top_date >= excluded.top_date
                                            THEN dialogs.top_message ELSE excluded.top_message END,
                    top_date         = MAX(dialogs.top_date * (dialogs.top_message > ?9), excluded.top_date),
                    pinned           = excluded.pinned,
                    folder_id        = excluded.folder_id,
                    unread_count     = excluded.unread_count,
                    pts              = excluded.pts",
            )?
            .execute(params![
                d.peer.kind as i64,
                d.peer.id,
                d.top_message,
                d.top_date,
                d.pinned,
                d.folder_id,
                d.unread_count,
                d.pts,
                SERVER_MAX_MESSAGE_ID,
                d.read_inbox_max_id,
                d.read_outbox_max_id,
                d.mute_until
            ])?;
        Ok(())
    }

    /// A new message arrived: it becomes the chat's last message, and the chat appears
    /// in the list if it was not there.
    pub fn message_arrived(&self, m: &MessageRow) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO dialogs (kind, id, top_message, top_date, pinned, folder_id, unread_count, pts, cached_from)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, NULL, ?3)
                 ON CONFLICT (kind, id) DO UPDATE SET
                    top_message  = MAX(dialogs.top_message, excluded.top_message),
                    top_date     = MAX(dialogs.top_date, excluded.top_date),
                    unread_count = dialogs.unread_count + excluded.unread_count",
            )?
            .execute(params![m.peer.kind as i64, m.peer.id, m.id, m.date, (!m.out) as i32])?;
        Ok(())
    }

    /// `updateReadHistoryInbox`: read up to `max_id`, `unread` left.
    pub fn set_read_inbox(&self, peer: PeerKey, max_id: i32, unread: i32) -> Result<()> {
        self.tx
            .prepare_cached(
                "UPDATE dialogs SET unread_count = ?4, read_inbox_max_id = MAX(read_inbox_max_id, ?3)
                 WHERE kind = ?1 AND id = ?2",
            )?
            .execute(params![peer.kind as i64, peer.id, max_id, unread])?;
        Ok(())
    }

    /// `updateReadHistoryOutbox`: the other side read our messages up to `max_id`.
    pub fn set_read_outbox(&self, peer: PeerKey, max_id: i32) -> Result<()> {
        self.tx
            .prepare_cached(
                "UPDATE dialogs SET read_outbox_max_id = MAX(read_outbox_max_id, ?3)
                 WHERE kind = ?1 AND id = ?2",
            )?
            .execute(params![peer.kind as i64, peer.id, max_id])?;
        Ok(())
    }

    /// `updateNotifySettings` for one chat.
    pub fn set_mute_until(&self, peer: PeerKey, mute_until: Option<i32>) -> Result<()> {
        self.tx
            .prepare_cached("UPDATE dialogs SET mute_until = ?3 WHERE kind = ?1 AND id = ?2")?
            .execute(params![peer.kind as i64, peer.id, mute_until])?;
        Ok(())
    }

    pub fn set_unread(&self, peer: PeerKey, unread: i32) -> Result<()> {
        self.tx
            .prepare_cached("UPDATE dialogs SET unread_count = ?3 WHERE kind = ?1 AND id = ?2")?
            .execute(params![peer.kind as i64, peer.id, unread])?;
        Ok(())
    }

    /// Forgets the chat list (before reloading it after `differenceTooLong`).
    pub fn clear_dialogs(&self) -> Result<()> {
        self.tx.execute("DELETE FROM dialogs", [])?;
        Ok(())
    }

    /// Stores older messages loaded with `offset_id` (newest first, as the server
    /// returns them). If they continue the cached segment, the segment grows down to
    /// the oldest of them; fewer than `limit` messages means the start of the chat.
    pub fn extend_history(
        &self,
        peer: PeerKey,
        offset_id: i32,
        limit: usize,
        rows: &[MessageRow],
    ) -> Result<()> {
        for m in rows {
            self.put_message(m)?;
        }
        let Some(oldest) = rows.iter().map(|m| m.id).min() else {
            if offset_id != 0 {
                self.mark_complete(peer, offset_id)?;
            }
            return Ok(());
        };
        self.tx
            .prepare_cached(
                "UPDATE dialogs SET cached_from = ?4
                 WHERE kind = ?1 AND id = ?2 AND (cached_from = ?3 OR (?3 = 0 AND cached_from IS NULL))",
            )?
            .execute(params![peer.kind as i64, peer.id, offset_id, oldest])?;
        if rows.len() < limit {
            self.mark_complete(peer, oldest)?;
        }
        Ok(())
    }

    fn mark_complete(&self, peer: PeerKey, cached_from: i32) -> Result<()> {
        self.tx
            .prepare_cached(
                "UPDATE dialogs SET history_complete = 1
                 WHERE kind = ?1 AND id = ?2 AND cached_from = ?3",
            )?
            .execute(params![peer.kind as i64, peer.id, cached_from])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, name: &str, hash: Option<i64>, min: bool) -> PeerRow {
        PeerRow {
            key: PeerKey::user(id),
            access_hash: hash,
            is_min: min,
            name: name.into(),
            username: None,
            is_megagroup: false,
            raw: Some((233, vec![1, 2, 3])),
        }
    }

    fn msg(peer: PeerKey, id: i32, date: i32, out: bool, text: &str) -> MessageRow {
        MessageRow {
            peer,
            id,
            date,
            from: None,
            out,
            text: text.into(),
            raw: None,
        }
    }

    #[test]
    fn local_messages_survive_chat_list_refreshes() {
        let mut s = Storage::open_in_memory().unwrap();
        let tg = PeerKey::user(777000);
        let dialog = |top_message, top_date| DialogRow {
            peer: tg,
            top_message,
            top_date,
            pinned: false,
            folder_id: 0,
            unread_count: 0,
            pts: None,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            mute_until: None,
        };
        s.write(|w| {
            w.put_message(&msg(tg, 100, 1000, false, "old"))?;
            w.put_dialog(&dialog(100, 1000))
        })
        .unwrap();
        // A service notification becomes a local message on top.
        let local = s
            .write(|w| {
                let id = w.next_local_message_id()?;
                let m = msg(tg, id, 2000, false, "notification");
                w.put_message(&m)?;
                w.message_arrived(&m)?;
                Ok(id)
            })
            .unwrap();
        assert!(local > SERVER_MAX_MESSAGE_ID);
        let ids = |s: &Storage| -> Vec<i32> {
            s.history(tg, 10)
                .unwrap()
                .messages
                .iter()
                .map(|m| m.id)
                .collect()
        };
        assert_eq!(ids(&s), vec![local, 100]);
        // The server's list still has the old top message: the local one stays.
        s.write(|w| w.put_dialog(&dialog(100, 1000))).unwrap();
        assert_eq!(ids(&s), vec![local, 100]);
        // A newer real message takes over.
        s.write(|w| {
            w.put_message(&msg(tg, 101, 3000, false, "new"))?;
            w.put_dialog(&dialog(101, 3000))
        })
        .unwrap();
        assert_eq!(ids(&s)[0], 101);
        // Ids keep counting up.
        let next = s.write(|w| w.next_local_message_id()).unwrap();
        assert_eq!(next, local + 1);
    }

    #[test]
    fn drafts_are_stored_and_lift_their_chat() {
        let mut s = Storage::open_in_memory().unwrap();
        let (a, b) = (PeerKey::user(1), PeerKey::user(2));
        s.write(|w| {
            for (peer, id, date) in [(a, 10, 100), (b, 20, 200)] {
                w.put_dialog(&DialogRow {
                    peer,
                    top_message: id,
                    top_date: date,
                    pinned: false,
                    folder_id: 0,
                    unread_count: 0,
                    pts: None,
                    read_inbox_max_id: 0,
                    read_outbox_max_id: 0,
                    mute_until: None,
                })?;
            }
            Ok(())
        })
        .unwrap();
        let order = |s: &Storage| -> Vec<PeerKey> {
            s.dialogs(0, 10)
                .unwrap()
                .iter()
                .map(|d| d.dialog.peer)
                .collect()
        };
        assert_eq!(order(&s), vec![b, a]);
        // A draft newer than b's last message moves a up.
        s.write(|w| w.set_draft(a, Some(("draft text", 300))))
            .unwrap();
        assert_eq!(order(&s), vec![a, b]);
        let first = &s.dialogs(0, 10).unwrap()[0];
        assert_eq!(first.draft, Some(("draft text".into(), 300)));
        assert_eq!(s.drafts().unwrap(), vec![(a, "draft text".into(), 300)]);
        s.write(|w| w.set_draft(a, None)).unwrap();
        assert_eq!(order(&s), vec![b, a]);
        assert!(s.drafts().unwrap().is_empty());
    }

    #[test]
    fn min_peers_never_replace_full_ones() {
        let mut s = Storage::open_in_memory().unwrap();
        s.write(|w| w.put_peer(&user(1, "Full", Some(42), false)))
            .unwrap();
        s.write(|w| w.put_peer(&user(1, "Min", None, true)))
            .unwrap();
        let p = s.peer(PeerKey::user(1)).unwrap().unwrap();
        assert_eq!(
            (p.name.as_str(), p.access_hash, p.is_min),
            ("Full", Some(42), false)
        );
        // A newer full object without a hash keeps the known hash.
        s.write(|w| w.put_peer(&user(1, "Renamed", None, false)))
            .unwrap();
        let p = s.peer(PeerKey::user(1)).unwrap().unwrap();
        assert_eq!((p.name.as_str(), p.access_hash), ("Renamed", Some(42)));
        assert_eq!(p.raw, Some((233, vec![1, 2, 3])));
    }

    #[test]
    fn chat_list_order_and_last_message() {
        let mut s = Storage::open_in_memory().unwrap();
        let a = PeerKey::user(1);
        let b = PeerKey::channel(2);
        s.write(|w| {
            w.put_peer(&user(1, "Alice", Some(1), false))?;
            for (peer, id, date) in [(a, 10, 100), (b, 5, 200)] {
                let m = msg(peer, id, date, false, "hi");
                w.put_message(&m)?;
                w.message_arrived(&m)?;
            }
            Ok(())
        })
        .unwrap();
        let list = s.dialogs(0, 10).unwrap();
        assert_eq!(
            list.iter().map(|d| d.dialog.peer).collect::<Vec<_>>(),
            [b, a]
        );
        assert_eq!(list[1].peer.as_ref().unwrap().name, "Alice");
        assert_eq!(list[1].top.as_ref().unwrap().text, "hi");
        assert_eq!(list[1].dialog.unread_count, 1);

        // A newer message in A moves it up; an outgoing one doesn't add unread.
        s.write(|w| {
            let m = msg(a, 11, 300, true, "reply");
            w.put_message(&m)?;
            w.message_arrived(&m)
        })
        .unwrap();
        let list = s.dialogs(0, 10).unwrap();
        assert_eq!(list[0].dialog.peer, a);
        assert_eq!(list[0].top.as_ref().unwrap().text, "reply");
        assert_eq!(list[0].dialog.unread_count, 1);
    }

    #[test]
    fn deletes_and_state() {
        let mut s = Storage::open_in_memory().unwrap();
        let a = PeerKey::user(1);
        let c = PeerKey::channel(9);
        s.write(|w| {
            w.put_message(&msg(a, 1, 1, false, "x"))?;
            w.put_message(&msg(c, 1, 1, false, "y"))?;
            w.delete_common_messages(&[1])?;
            w.set_update_state(UpdateState {
                pts: 5,
                qts: 6,
                date: 7,
                seq: 8,
            })?;
            w.set_channel_pts(9, 77)
        })
        .unwrap();
        assert_eq!(
            s.update_state().unwrap(),
            Some(UpdateState {
                pts: 5,
                qts: 6,
                date: 7,
                seq: 8
            })
        );
        assert_eq!(s.channel_pts().unwrap(), [(9, 77)]);
        let count: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "the channel message with the same id survives");
    }

    #[test]
    fn history_segment_grows_and_restarts() {
        let mut s = Storage::open_in_memory().unwrap();
        let a = PeerKey::user(1);
        let dialog = |top: i32| DialogRow {
            peer: a,
            top_message: top,
            top_date: top,
            pinned: false,
            folder_id: 0,
            unread_count: 0,
            pts: None,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            mute_until: None,
        };
        // Chat list load: only the top message is known.
        s.write(|w| {
            w.put_message(&msg(a, 100, 100, false, "top"))?;
            w.put_dialog(&dialog(100))
        })
        .unwrap();
        let h = s.history(a, 10).unwrap();
        assert_eq!(
            (h.messages.len(), h.cached_from, h.complete),
            (1, Some(100), false)
        );

        // Older messages loaded below the segment extend it.
        let older: Vec<_> = (90..100)
            .rev()
            .map(|id| msg(a, id, id, false, "old"))
            .collect();
        s.write(|w| w.extend_history(a, 100, 10, &older)).unwrap();
        assert_eq!(s.history(a, 50).unwrap().messages.len(), 11);

        // A live message (in order) keeps the segment contiguous.
        s.write(|w| {
            let m = msg(a, 101, 101, false, "new");
            w.put_message(&m)?;
            w.message_arrived(&m)
        })
        .unwrap();
        let h = s.history(a, 50).unwrap();
        assert_eq!(
            (h.messages.len(), h.messages[0].id, h.cached_from),
            (12, 101, Some(90))
        );

        // A short page means the start of the chat.
        s.write(|w| w.extend_history(a, 90, 10, &[msg(a, 50, 50, false, "first")]))
            .unwrap();
        let h = s.history(a, 50).unwrap();
        assert!(h.complete);
        assert_eq!(h.cached_from, Some(50));

        // The chat list says the top moved without us seeing it: the segment restarts.
        s.write(|w| {
            w.put_message(&msg(a, 200, 200, false, "jump"))?;
            w.put_dialog(&dialog(200))
        })
        .unwrap();
        let h = s.history(a, 50).unwrap();
        assert_eq!(
            (h.messages.len(), h.cached_from, h.complete),
            (1, Some(200), false)
        );

        // Refreshing with the same top keeps the segment.
        s.write(|w| w.extend_history(a, 200, 10, &older)).unwrap();
        s.write(|w| w.put_dialog(&dialog(200))).unwrap();
        assert_eq!(s.history(a, 50).unwrap().cached_from, Some(90));
    }

    #[test]
    fn migrates_from_v1() {
        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            create_v1(&tx).unwrap();
            tx.pragma_update(None, "user_version", 1).unwrap();
            tx.commit().unwrap();
        }
        let s = Storage::init(conn).unwrap();
        let v: i32 = s
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(s.history(PeerKey::user(1), 5).unwrap().cached_from, None);
    }

    #[test]
    fn reopening_keeps_data() {
        let dir = std::env::temp_dir().join(format!("tdr-storage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.sqlite");
        {
            let mut s = Storage::open(&path).unwrap();
            s.write(|w| w.set_meta_i64("self_id", 123)).unwrap();
        }
        let s = Storage::open(&path).unwrap();
        assert_eq!(s.meta_i64("self_id").unwrap(), Some(123));
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
