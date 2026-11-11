//! Telegram client core on top of `tdr-mtproto`.
//!
//! * one connection per DC, created on demand (or ahead of time with [`Client::prepare_dc`]);
//! * permanent keys per DC, optional temporary keys bound to them (perfect forward
//!   secrecy, as tdesktop does), optionally kept across restarts;
//! * authorization exported from the home DC to other DCs;
//! * `initConnection` on every new key, `*_MIGRATE_X` and short `FLOOD_WAIT_X` handled
//!   transparently.

mod session;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use tdr_mtproto::{Event, KeySpec, PRODUCTION_DCS, RpcError, Sender, TEST_DCS};
use tdr_tl::{Deserializable, Reader, RemoteCall, Serializable, enums, functions};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use tracing::{debug, info};

pub use session::{DcKeys, SessionData, SessionError, TempKey};
pub use tdr_mtproto::Error;

/// Lifetime of temporary keys, as in tdesktop (`kTemporaryExpiresIn`).
const TEMP_KEY_LIFETIME: i32 = 24 * 60 * 60;
/// A saved temporary key is reused only if it lives at least this long.
const TEMP_KEY_MIN_REMAINING: i32 = 5 * 60;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub device_model: String,
    pub system_version: String,
    pub app_version: String,
    pub system_lang_code: String,
    pub lang_code: String,
    /// Connect to Telegram's test environment.
    pub test: bool,
    /// Use temporary keys bound to the permanent one (perfect forward secrecy).
    pub pfs: bool,
    /// Keep temporary keys (and their server salts) in the saved session, so a restart
    /// does not redo the Diffie-Hellman exchange: start → first answer ≈ 0.15 s instead
    /// of ≈ 1.2 s. tdesktop keeps them in memory only. Trade-off: someone who steals the
    /// saved session and had recorded the traffic can decrypt the last ≤ 24 hours of it.
    pub persist_temp_keys: bool,
    /// `FLOOD_WAIT_X` up to this many seconds is waited out automatically.
    pub flood_sleep_threshold: u32,
}

impl Config {
    pub fn new(api_id: i32, api_hash: impl Into<String>) -> Self {
        Self {
            api_id,
            api_hash: api_hash.into(),
            device_model: "Desktop".into(),
            system_version: std::env::consts::OS.into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            system_lang_code: "en".into(),
            lang_code: "en".into(),
            test: false,
            pfs: true,
            persist_temp_keys: true,
            flood_sleep_threshold: 60,
        }
    }
}

fn server_now(time_offset: i32) -> i32 {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + time_offset as i64) as i32
}

/// A request already serialized. Used to wrap any request in `initConnection`; the answer
/// is read back through `Sender::invoke_raw`, so `RawAnswer` is never actually built.
struct Raw(Vec<u8>);
struct RawAnswer;

impl Serializable for Raw {
    fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0);
    }
}
impl Deserializable for RawAnswer {
    fn deserialize(r: &mut Reader<'_>) -> tdr_tl::Result<Self> {
        let _ = r.rest();
        Ok(RawAnswer)
    }
}
impl RemoteCall for Raw {
    type Return = RawAnswer;
}

/// A live connection to one DC.
struct DcConn {
    sender: Sender,
    /// The key this connection uses (temporary or permanent).
    is_temp: bool,
    /// `initConnection` was acknowledged on this key.
    initialized: AtomicBool,
}

/// Per-DC slot; the async mutex serializes connection setup for that DC.
type Slot = Arc<AsyncMutex<Option<Arc<DcConn>>>>;

struct Inner {
    config: Config,
    session: Mutex<SessionData>,
    slots: Mutex<HashMap<i32, Slot>>,
    events_tx: mpsc::UnboundedSender<Event>,
    events_rx: AsyncMutex<mpsc::UnboundedReceiver<Event>>,
}

/// Cheap-to-clone handle to the client.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Creates a client. Nothing is connected until the first request.
    pub fn new(config: Config, session: SessionData) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(Inner {
                config,
                session: Mutex::new(session),
                slots: Mutex::new(HashMap::new()),
                events_tx,
                events_rx: AsyncMutex::new(events_rx),
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// A copy of the current session for saving, with the salts of live connections.
    pub fn session(&self) -> SessionData {
        let slots: Vec<(i32, Slot)> = self
            .inner
            .slots
            .lock()
            .unwrap()
            .iter()
            .map(|(dc, s)| (*dc, s.clone()))
            .collect();
        let mut session = self.inner.session.lock().unwrap().clone();
        for (dc, slot) in slots {
            if let Ok(guard) = slot.try_lock()
                && let Some(conn) = guard.as_ref()
                && conn.is_temp
                && let Some(t) = session.dcs.get_mut(&dc).and_then(|k| k.temp.as_mut())
            {
                t.salt = conn.sender.salt();
            }
        }
        session
    }

    /// Serialized session, honoring `Config::persist_temp_keys`.
    pub fn session_bytes(&self) -> Vec<u8> {
        self.session().to_bytes(self.inner.config.persist_temp_keys)
    }

    pub fn home_dc(&self) -> i32 {
        self.inner.session.lock().unwrap().home_dc
    }

    pub fn is_authorized(&self) -> bool {
        self.inner.session.lock().unwrap().is_authorized()
    }

    /// Switches the home DC (e.g. on `auth.loginTokenMigrateTo`).
    pub fn set_home_dc(&self, dc: i32) {
        self.inner.session.lock().unwrap().home_dc = dc;
    }

    /// Marks the home DC as logged in after a successful `auth.*` call.
    pub fn mark_authorized(&self) {
        let mut s = self.inner.session.lock().unwrap();
        let home = s.home_dc;
        s.dcs.entry(home).or_default().authorized = true;
    }

    /// Forgets all keys (after `auth.logOut`).
    pub async fn reset(&self) {
        let home = self.home_dc();
        *self.inner.session.lock().unwrap() = SessionData::new(home);
        self.inner.slots.lock().unwrap().clear();
    }

    /// The next update or connection event from the home DC.
    pub async fn next_event(&self, timeout: Duration) -> Option<Event> {
        let mut rx = self.inner.events_rx.lock().await;
        tokio::time::timeout(timeout, rx.recv())
            .await
            .ok()
            .flatten()
    }

    fn addr(&self, dc: i32) -> Result<&'static str, Error> {
        let table = if self.inner.config.test {
            TEST_DCS
        } else {
            PRODUCTION_DCS
        };
        table
            .iter()
            .find(|(id, _)| *id == dc)
            .map(|(_, a)| *a)
            .ok_or(Error::Security("unknown DC"))
    }

    fn slot(&self, dc: i32) -> Slot {
        self.inner
            .slots
            .lock()
            .unwrap()
            .entry(dc)
            .or_default()
            .clone()
    }

    /// Opens the connection to `dc` in the background (keys, authorization import),
    /// so that the first real request there does not wait for it.
    pub fn prepare_dc(&self, dc: i32) {
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.conn(dc).await {
                debug!(dc, "prepare_dc failed: {e}");
            }
        });
    }

    /// Returns a working connection to `dc`, creating keys and importing the
    /// authorization as needed.
    ///
    /// Boxed with an explicit `Send`: importing the authorization calls `invoke_in` on the
    /// home DC, which calls back into `conn`, and the compiler cannot infer `Send` across
    /// that cycle on its own.
    fn conn(
        &self,
        dc: i32,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<DcConn>, Error>> + Send + '_>> {
        Box::pin(self.conn_inner(dc))
    }

    async fn conn_inner(&self, dc: i32) -> Result<Arc<DcConn>, Error> {
        let slot = self.slot(dc);
        let mut guard = slot.lock().await;
        if let Some(c) = guard.as_ref()
            && !c.sender.is_closed()
        {
            return Ok(c.clone());
        }
        let conn = Arc::new(self.open(dc).await?);
        *guard = Some(conn.clone());
        drop(guard);

        // Other DCs need the account's authorization copied from the home DC.
        let (home, needs_import) = {
            let s = self.inner.session.lock().unwrap();
            let authorized_here = s.dcs.get(&dc).is_some_and(|d| d.authorized);
            (
                s.home_dc,
                dc != s.home_dc && s.is_authorized() && !authorized_here,
            )
        };
        if needs_import {
            let started = std::time::Instant::now();
            let enums::auth::ExportedAuthorization::ExportedAuthorization(exported) = self
                .invoke_in(home, &functions::auth::ExportAuthorization { dc_id: dc })
                .await?;
            let import = functions::auth::ImportAuthorization {
                id: exported.id,
                bytes: exported.bytes,
            };
            self.call(&conn, import.to_bytes()).await?;
            self.inner
                .session
                .lock()
                .unwrap()
                .dcs
                .entry(dc)
                .or_default()
                .authorized = true;
            info!(dc, elapsed = ?started.elapsed(), "authorization imported");
        }
        Ok(conn)
    }

    async fn open(&self, dc: i32) -> Result<DcConn, Error> {
        let cfg = &self.inner.config;
        let addr = self.addr(dc)?;
        let started = std::time::Instant::now();
        let (perm, temp, time_offset) = {
            let s = self.inner.session.lock().unwrap();
            let keys = s.dcs.get(&dc).cloned().unwrap_or_default();
            (keys.perm, keys.temp, s.time_offset)
        };

        // 1. Permanent key.
        let (perm, perm_conn) = match perm {
            Some(k) => (k, None),
            None => {
                let c = Sender::connect(addr, dc, KeySpec::CreatePermanent, cfg.test).await?;
                let mut s = self.inner.session.lock().unwrap();
                s.time_offset = c.time_offset;
                let keys = s.dcs.entry(dc).or_default();
                keys.perm = Some(c.auth_key.clone());
                keys.temp = None;
                keys.authorized = false;
                (c.auth_key.clone(), Some(c))
            }
        };

        // 2. Without PFS the permanent key is used directly.
        if !cfg.pfs {
            let connected = match perm_conn {
                Some(c) => c,
                None => {
                    Sender::connect(
                        addr,
                        dc,
                        KeySpec::Existing {
                            key: perm,
                            time_offset,
                            salt: 0,
                        },
                        cfg.test,
                    )
                    .await?
                }
            };
            self.forward_events(dc, connected.events);
            info!(dc, elapsed = ?started.elapsed(), "connected (permanent key)");
            return Ok(DcConn {
                sender: connected.sender,
                is_temp: false,
                initialized: AtomicBool::new(false),
            });
        }
        drop(perm_conn);

        // 3. Temporary key: reuse a saved one if it lives long enough, else create and bind.
        let reusable =
            temp.filter(|t| t.expires_at - server_now(time_offset) > TEMP_KEY_MIN_REMAINING);
        let reused = reusable.is_some();
        let connected = match reusable {
            Some(t) => {
                let spec = KeySpec::Existing {
                    key: t.key,
                    time_offset,
                    salt: t.salt,
                };
                Sender::connect(addr, dc, spec, cfg.test).await?
            }
            None => {
                let c = Sender::connect(
                    addr,
                    dc,
                    KeySpec::CreateTemporary {
                        perm,
                        expires_in: TEMP_KEY_LIFETIME,
                    },
                    cfg.test,
                )
                .await?;
                let mut s = self.inner.session.lock().unwrap();
                s.time_offset = c.time_offset;
                s.dcs.entry(dc).or_default().temp = Some(TempKey {
                    key: c.auth_key.clone(),
                    expires_at: c.expires_at.unwrap_or(0),
                    salt: c.sender.salt(),
                });
                c
            }
        };
        self.forward_events(dc, connected.events);
        info!(dc, elapsed = ?started.elapsed(), reused, "connected (temporary key)");
        Ok(DcConn {
            sender: connected.sender,
            is_temp: true,
            initialized: AtomicBool::new(false),
        })
    }

    fn forward_events(&self, dc: i32, mut events: mpsc::UnboundedReceiver<Event>) {
        let tx = self.inner.events_tx.clone();
        let session = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            while let Some(e) = events.recv().await {
                let is_home = session
                    .upgrade()
                    .is_some_and(|i| i.session.lock().unwrap().home_dc == dc);
                if is_home && tx.send(e).is_err() {
                    break;
                }
            }
        });
    }

    // Only these thin generic wrappers are instantiated per request type; all the async
    // logic below them (migrations, flood waits, key recovery, initConnection) is
    // compiled once. This keeps builds fast and the binary small.

    /// Invokes `request` on the home DC.
    pub async fn invoke<R: RemoteCall>(&self, request: &R) -> Result<R::Return, Error> {
        let answer = self.invoke_raw(&request.to_bytes()).await?;
        Ok(R::Return::deserialize(&mut Reader::new(&answer))?)
    }

    /// Invokes `request` on a specific DC (e.g. to download a file stored there).
    pub async fn invoke_in<R: RemoteCall>(&self, dc: i32, request: &R) -> Result<R::Return, Error> {
        let answer = self.invoke_in_raw(dc, &request.to_bytes()).await?;
        Ok(R::Return::deserialize(&mut Reader::new(&answer))?)
    }

    /// Invokes a serialized request on the home DC, following `*_MIGRATE_X`.
    pub async fn invoke_raw(&self, body: &[u8]) -> Result<Bytes, Error> {
        let mut migrations = 0;
        loop {
            let home = self.home_dc();
            match self.invoke_in_raw(home, body).await {
                Err(Error::Rpc(e)) if e.code == 303 && migrations < 3 => {
                    let Some(target) = migrate_target(&e) else {
                        return Err(Error::Rpc(e));
                    };
                    info!(from = home, to = target, "{}", e.message);
                    migrations += 1;
                    self.set_home_dc(target);
                }
                other => return other,
            }
        }
    }

    /// Invokes a serialized request on `dc`, waiting out short flood waits and
    /// recreating a key the server forgot.
    pub async fn invoke_in_raw(&self, dc: i32, body: &[u8]) -> Result<Bytes, Error> {
        let mut retried_key = false;
        loop {
            let conn = self.conn(dc).await?;
            match self.call(&conn, body.to_vec()).await {
                Err(Error::Rpc(e)) if e.is("FLOOD_WAIT") || e.is("FLOOD_PREMIUM_WAIT") => {
                    let secs = e.value().unwrap_or(0).max(0) as u32;
                    if secs > self.inner.config.flood_sleep_threshold {
                        return Err(Error::Rpc(e));
                    }
                    info!(dc, secs, "flood wait");
                    tokio::time::sleep(Duration::from_secs(secs as u64 + 1)).await;
                }
                Err(Error::AuthKeyUnknown) if !retried_key => {
                    retried_key = true;
                    self.drop_key(dc, conn.is_temp);
                }
                other => return other,
            }
        }
    }

    /// Sends a serialized request on `conn`, wrapping it in `initConnection` until the
    /// server has accepted one on this key.
    async fn call(&self, conn: &DcConn, body: Vec<u8>) -> Result<Bytes, Error> {
        if conn.initialized.load(Ordering::Acquire) {
            return conn.sender.invoke_raw(body).await;
        }
        let cfg = &self.inner.config;
        let wrapped = functions::InvokeWithLayer {
            layer: tdr_tl::LAYER,
            query: functions::InitConnection {
                api_id: cfg.api_id,
                device_model: cfg.device_model.clone(),
                system_version: cfg.system_version.clone(),
                app_version: cfg.app_version.clone(),
                system_lang_code: cfg.system_lang_code.clone(),
                lang_pack: String::new(),
                lang_code: cfg.lang_code.clone(),
                proxy: None,
                params: None,
                query: Raw(body),
            },
        };
        let r = conn.sender.invoke_raw(wrapped.to_bytes()).await;
        if !matches!(
            r,
            Err(Error::Disconnected | Error::AuthKeyUnknown | Error::Io(_))
        ) {
            conn.initialized.store(true, Ordering::Release);
        }
        r
    }

    /// Drops the socket to `dc` as if the network failed (for testing reconnects).
    #[doc(hidden)]
    pub async fn simulate_network_failure(&self, dc: i32) -> Result<(), Error> {
        self.conn(dc).await?.sender.simulate_network_failure().await
    }

    /// The server forgot a key: drop it so the next connection creates a new one.
    fn drop_key(&self, dc: i32, temp: bool) {
        let mut s = self.inner.session.lock().unwrap();
        if let Some(keys) = s.dcs.get_mut(&dc) {
            if temp {
                keys.temp = None;
            } else {
                // A revoked permanent key means this DC's authorization is gone.
                *keys = DcKeys::default();
            }
        }
        drop(s);
        if let Some(slot) = self.inner.slots.lock().unwrap().get(&dc)
            && let Ok(mut g) = slot.try_lock()
        {
            *g = None;
        }
    }
}

/// DC to switch to for `PHONE_MIGRATE_X`, `USER_MIGRATE_X`, `NETWORK_MIGRATE_X`.
fn migrate_target(e: &RpcError) -> Option<i32> {
    ["PHONE_MIGRATE", "USER_MIGRATE", "NETWORK_MIGRATE"]
        .iter()
        .any(|p| e.is(p))
        .then(|| e.value())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_targets() {
        let e = |m: &str| RpcError {
            code: 303,
            message: m.into(),
        };
        assert_eq!(migrate_target(&e("PHONE_MIGRATE_4")), Some(4));
        assert_eq!(migrate_target(&e("USER_MIGRATE_1")), Some(1));
        assert_eq!(migrate_target(&e("FILE_MIGRATE_5")), None);
    }

    #[test]
    fn raw_request_passes_bytes_through() {
        let r = Raw(vec![1, 2, 3, 4]);
        assert_eq!(r.to_bytes(), vec![1, 2, 3, 4]);
    }
}
