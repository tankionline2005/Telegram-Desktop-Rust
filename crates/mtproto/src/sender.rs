//! Async request/response over one MTProto session.
//!
//! One task owns the socket writer and the [`Session`]; a second task only reads frames
//! (reads are not cancel-safe, so they never sit inside `select!`). Requests submitted
//! while a write is in flight are batched into one container on the next write.
//!
//! The connection heals itself: on an I/O error or a silent socket the task reconnects
//! with backoff, keeps the session and resends unanswered requests with their original
//! msg_ids (so the server answers from its cache instead of executing them twice). Only an
//! unknown auth key (transport error -404) stops the sender: the key has to be recreated.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use tdr_crypto::AuthKey;
use tdr_tl::{Deserializable, Reader, RemoteCall, Serializable};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::session::{Incoming, Outbound, Session};
use crate::transport::{self, TransportError};
use crate::{Error, handshake};

/// Something the server sent that is not an answer to a request.
#[derive(Debug, Clone)]
pub enum Event {
    /// A raw update (`Updates` or one of its constructors).
    Update(Bytes),
    /// The server started a new session: updates may have been missed.
    NewSession,
    /// The connection dropped; a new one is being made.
    ConnectionLost,
    /// The connection dropped and was re-established.
    Reconnected,
}

/// Backwards-compatible name for raw updates.
pub type Update = Bytes;

enum Body {
    Raw(Vec<u8>),
    Bind {
        perm: Box<AuthKey>,
        expires_at: i32,
    },
    /// Test hook: drop the socket as if the network failed.
    DropConnection,
}

struct Request {
    body: Body,
    reply: Option<oneshot::Sender<Result<Bytes, Error>>>,
    /// `(msg_id, seq_no)` once sent; kept for resends after a reconnect.
    sent: Option<(i64, i32)>,
}

impl Request {
    fn answer(mut self, result: Result<Bytes, Error>) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(result);
        }
    }
}

/// How to obtain the key for a new connection.
pub enum KeySpec {
    /// Use an existing key (permanent, or temporary and already bound).
    /// `salt` may be a saved server salt (0 if unknown: the server will send one).
    Existing {
        key: AuthKey,
        time_offset: i32,
        salt: i64,
    },
    /// Create a new permanent key.
    CreatePermanent,
    /// Create a temporary key for `expires_in` seconds and bind it to `perm`.
    CreateTemporary { perm: AuthKey, expires_in: i32 },
}

/// Handle to a connection. Cheap to clone.
#[derive(Clone)]
pub struct Sender {
    tx: mpsc::UnboundedSender<Request>,
    /// Current server salt, mirrored from the session for saving.
    salt: Arc<AtomicI64>,
}

/// What `Sender::connect` produced.
pub struct Connected {
    pub sender: Sender,
    pub events: mpsc::UnboundedReceiver<Event>,
    pub auth_key: AuthKey,
    pub time_offset: i32,
    /// For a temporary key: when it expires (server time, unix seconds).
    pub expires_at: Option<i32>,
}

const PING_INTERVAL: Duration = Duration::from_secs(30);
// tdesktop's `session_private.cpp`: a connection round may take `kMinConnectedTimeout`,
// doubled after each timeout up to `kMaxConnectedTimeout`; a sent request must get
// something back within `kMinReceiveTimeout`, doubled after each such failure up to
// `kMaxReceiveTimeout` (and brought down again by fast answers).
const MIN_CONNECTED: Duration = Duration::from_secs(1);
const MAX_CONNECTED: Duration = Duration::from_secs(8);
const MIN_RECEIVE: Duration = Duration::from_secs(4);
const MAX_RECEIVE: Duration = Duration::from_secs(64);
/// The server closes the connection if it hears nothing for this long.
const PING_DISCONNECT_DELAY: i32 = 75;
/// No frame for this long (pings are answered) means the socket is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(45);
/// A round where every address failed at once is retried after 1, 2, 3 ms, then 1 s
/// doubling (`retryByTimer`) up to this (tdesktop: 64 s).
const MAX_RETRY: Duration = Duration::from_secs(8);

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct Endpoint {
    /// Every known address of the DC: all are tried at once.
    addrs: Vec<String>,
    transport_dc: i16,
}

/// The adaptive waits of one sender, kept across its reconnects.
struct Timing {
    connect: Duration,
    receive: Duration,
}

impl Timing {
    fn new() -> Self {
        Self {
            connect: MIN_CONNECTED,
            receive: MIN_RECEIVE,
        }
    }
}

fn timed_out(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

/// Connects to the first of `addrs` that answers a fake `req_pq_multi`: all of them at
/// once, like tdesktop's test connections (`SessionPrivate::connectToServer`).
async fn connect_fastest(
    addrs: &[String],
    dc: i16,
    wait: Duration,
) -> Result<(transport::Reader, transport::Writer), Error> {
    let mut set = tokio::task::JoinSet::new();
    for addr in addrs {
        let addr = addr.clone();
        set.spawn(async move {
            let started = std::time::Instant::now();
            let (mut r, mut w) = transport::connect(&addr, dc).await?;
            handshake::probe(&mut r, &mut w).await?;
            debug!(addr, elapsed = ?started.elapsed(), "connected");
            Ok::<_, Error>((r, w))
        });
    }
    let race = async {
        let mut last = None;
        while let Some(done) = set.join_next().await {
            match done {
                Ok(Ok(c)) => return Ok(c),
                Ok(Err(e)) => last = Some(e),
                Err(_) => {}
            }
        }
        Err(last.unwrap_or(Error::Disconnected))
    };
    // The losers are aborted when `set` is dropped.
    match tokio::time::timeout(wait, race).await {
        Ok(result) => result,
        Err(_) => Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("no address answered in {wait:?}"),
        ))),
    }
}

/// Connects, trying again until it works or `stop` says to give up: a round that timed
/// out is retried at once with a longer wait (`waitConnectedFailed`), one where every
/// address failed after a growing pause (`retryByTimer`).
async fn establish(
    endpoint: &Endpoint,
    timing: &mut Timing,
    stop: impl Fn() -> bool,
) -> Option<(transport::Reader, transport::Writer)> {
    let mut retry = Duration::ZERO;
    loop {
        if stop() {
            return None;
        }
        match connect_fastest(&endpoint.addrs, endpoint.transport_dc, timing.connect).await {
            Ok(c) => {
                timing.connect = MIN_CONNECTED;
                return Some(c);
            }
            Err(e) if timed_out(&e) => {
                debug!("connect: {e}");
                timing.connect = (timing.connect * 2).min(MAX_CONNECTED);
            }
            Err(e) => {
                retry = if retry < Duration::from_millis(3) {
                    retry + Duration::from_millis(1)
                } else if retry < Duration::from_secs(1) {
                    Duration::from_secs(1)
                } else {
                    (retry * 2).min(MAX_RETRY)
                };
                debug!("connect failed: {e}; retry in {retry:?}");
                tokio::time::sleep(retry).await;
            }
        }
    }
}

struct Connection {
    writer: transport::Writer,
    frames: mpsc::UnboundedReceiver<std::io::Result<Bytes>>,
}

fn spawn_reader(mut reader: transport::Reader) -> mpsc::UnboundedReceiver<std::io::Result<Bytes>> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let frame = reader.recv().await;
            let stop = frame.is_err();
            if tx.send(frame).is_err() || stop {
                break;
            }
        }
    });
    rx
}

impl Sender {
    /// Connects to `addr` and prepares the key according to `key`.
    ///
    /// `dc_id` is the plain DC number (negative for media-only DCs); the test
    /// environment offset is applied here.
    pub async fn connect(
        addrs: &[String],
        dc_id: i32,
        key: KeySpec,
        test: bool,
    ) -> Result<Connected, Error> {
        let full_dc = if test {
            dc_id + if dc_id < 0 { -10_000 } else { 10_000 }
        } else {
            dc_id
        };
        let endpoint = Endpoint {
            addrs: addrs.to_vec(),
            transport_dc: full_dc as i16,
        };
        let mut timing = Timing::new();
        let (mut reader, mut writer) = establish(&endpoint, &mut timing, || false)
            .await
            .ok_or(Error::Disconnected)?;

        let (auth_key, time_offset, salt, bind) = match key {
            KeySpec::Existing {
                key,
                time_offset,
                salt,
            } => (key, time_offset, salt, None),
            KeySpec::CreatePermanent => {
                let c = handshake::create_auth_key(&mut reader, &mut writer, full_dc, test, None)
                    .await?;
                (c.auth_key, c.time_offset, c.first_salt, None)
            }
            KeySpec::CreateTemporary { perm, expires_in } => {
                let c = handshake::create_auth_key(
                    &mut reader,
                    &mut writer,
                    full_dc,
                    test,
                    Some(expires_in),
                )
                .await?;
                let expires_at = (unix_now() + c.time_offset as i64) as i32 + expires_in;
                (
                    c.auth_key,
                    c.time_offset,
                    c.first_salt,
                    Some((perm, expires_at)),
                )
            }
        };
        debug!(key = ?auth_key, time_offset, "connection ready");

        let session = Session::new(auth_key.clone(), salt, time_offset);
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::unbounded_channel();
        let conn = Connection {
            writer,
            frames: spawn_reader(reader),
        };
        let salt_mirror = Arc::new(AtomicI64::new(salt));
        tokio::spawn(run(
            session,
            endpoint,
            timing,
            conn,
            rx,
            events_tx,
            salt_mirror.clone(),
        ));
        let sender = Sender {
            tx,
            salt: salt_mirror,
        };

        let expires_at = match bind {
            Some((perm, expires_at)) => {
                let answer = sender
                    .call(Body::Bind {
                        perm: Box::new(perm),
                        expires_at,
                    })
                    .await?;
                if !bool::deserialize(&mut Reader::new(&answer))? {
                    return Err(Error::Security("auth.bindTempAuthKey returned false"));
                }
                Some(expires_at)
            }
            None => None,
        };
        Ok(Connected {
            sender,
            events,
            auth_key,
            time_offset,
            expires_at,
        })
    }

    /// Sends a request and waits for its result.
    pub async fn invoke<R: RemoteCall>(&self, request: &R) -> Result<R::Return, Error> {
        let bytes = self.invoke_raw(request.to_bytes()).await?;
        Ok(R::Return::deserialize(&mut Reader::new(&bytes))?)
    }

    /// Sends a serialized request; returns the raw result.
    pub async fn invoke_raw(&self, body: Vec<u8>) -> Result<Bytes, Error> {
        self.call(Body::Raw(body)).await
    }

    async fn call(&self, body: Body) -> Result<Bytes, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request {
                body,
                reply: Some(reply),
                sent: None,
            })
            .map_err(|_| Error::Disconnected)?;
        rx.await.map_err(|_| Error::Disconnected)?
    }

    /// Whether the connection task has stopped (e.g. the auth key became unknown).
    /// The server salt currently in use (worth saving with the key: a restart then
    /// skips the `bad_server_salt` round trip).
    pub fn salt(&self) -> i64 {
        self.salt.load(Ordering::Relaxed)
    }

    /// Drops the current socket as if the network failed (for testing reconnects).
    #[doc(hidden)]
    pub async fn simulate_network_failure(&self) -> Result<(), Error> {
        self.call(Body::DropConnection).await.map(|_| ())
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Why a connection ended.
enum Ended {
    /// All `Sender` handles were dropped.
    Dropped,
    /// Network problem: reconnect.
    Broken(String),
    /// The server does not know our key.
    UnknownKey,
}

async fn run(
    mut session: Session,
    endpoint: Endpoint,
    mut timing: Timing,
    mut conn: Connection,
    mut requests: mpsc::UnboundedReceiver<Request>,
    events: mpsc::UnboundedSender<Event>,
    salt: Arc<AtomicI64>,
) {
    // msg_id → request awaiting its answer.
    let mut pending: HashMap<i64, Request> = HashMap::new();
    // Requests to (re)send on the next write.
    let mut queue: Vec<Request> = Vec::new();

    loop {
        let ended = serve(
            &mut session,
            &mut timing,
            &mut conn,
            &mut requests,
            &events,
            &mut pending,
            &mut queue,
            &salt,
        )
        .await;
        match ended {
            Ended::Dropped => return,
            Ended::UnknownKey => {
                warn!("server does not know the auth key");
                for r in pending.drain().map(|(_, r)| r).chain(queue.drain(..)) {
                    r.answer(Err(Error::AuthKeyUnknown));
                }
                return;
            }
            Ended::Broken(why) => {
                info!("connection lost: {why}; reconnecting");
                let _ = events.send(Event::ConnectionLost);
                // Unanswered requests go first, keeping their ids while still valid.
                let mut resend: Vec<Request> = pending.drain().map(|(_, r)| r).collect();
                resend.sort_by_key(|r| r.sent.map(|s| s.0));
                for r in &mut resend {
                    if r.sent.is_some_and(|(id, _)| !session.is_fresh(id))
                        || matches!(r.body, Body::Bind { .. })
                    {
                        r.sent = None;
                    }
                }
                resend.append(&mut queue);
                queue = resend;

                let stop = || requests.is_closed() && queue.iter().all(|r| r.reply.is_none());
                let Some((reader, writer)) = establish(&endpoint, &mut timing, stop).await else {
                    return;
                };
                conn = Connection {
                    writer,
                    frames: spawn_reader(reader),
                };
                let _ = events.send(Event::Reconnected);
            }
        }
    }
}

/// Serves one connection until it ends.
#[allow(clippy::too_many_arguments)]
async fn serve(
    session: &mut Session,
    timing: &mut Timing,
    conn: &mut Connection,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    events: &mpsc::UnboundedSender<Event>,
    pending: &mut HashMap<i64, Request>,
    queue: &mut Vec<Request>,
    salt: &AtomicI64,
) -> Ended {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    let mut ping_id = 0i64;
    let mut deadline = Instant::now() + READ_TIMEOUT;
    // Something sent waits for an answer until then (`onSentSome`); when the first of
    // them went out.
    let mut answer_by: Option<Instant> = None;
    let mut first_sent: Option<Instant> = None;

    loop {
        // Flush the queue (and acks) in as few frames as possible.
        match flush(session, conn, pending, queue).await {
            Ok(0) => {}
            Ok(size) => {
                let now = Instant::now();
                if answer_by.is_none() {
                    // 8 KB/s: a big request may take longer.
                    let by_size = timing.receive.mul_f64(size as f64 / 8192.);
                    answer_by = Some(now + by_size.clamp(timing.receive, MAX_RECEIVE));
                }
                first_sent.get_or_insert(now);
            }
            Err(e) => return Ended::Broken(e.to_string()),
        }
        let answer_deadline = answer_by.unwrap_or(deadline);

        tokio::select! {
            req = requests.recv() => match req {
                Some(r) if matches!(r.body, Body::DropConnection) => {
                    r.answer(Ok(Bytes::new()));
                    return Ended::Broken("simulated network failure".into());
                }
                Some(r) => {
                    queue.push(r);
                    // Batch everything that is already waiting.
                    while let Ok(r) = requests.try_recv() {
                        if matches!(r.body, Body::DropConnection) {
                            r.answer(Ok(Bytes::new()));
                            return Ended::Broken("simulated network failure".into());
                        }
                        queue.push(r);
                    }
                }
                None => {
                    if pending.is_empty() && queue.is_empty() {
                        return Ended::Dropped;
                    }
                }
            },
            frame = conn.frames.recv() => {
                let frame = match frame {
                    Some(Ok(f)) => f,
                    Some(Err(e)) if TransportError::from_io(&e) == Some(-404) => return Ended::UnknownKey,
                    Some(Err(e)) => return Ended::Broken(e.to_string()),
                    // The reader task is gone without an error: the runtime is shutting down.
                    None => return Ended::Dropped,
                };
                deadline = Instant::now() + READ_TIMEOUT;
                // `onReceivedSome`: a fast answer shortens the wait again.
                answer_by = None;
                if let Some(sent) = first_sent.take() {
                    let took = sent.elapsed();
                    if took * 2 < timing.receive {
                        timing.receive = (took * 2).max(MIN_RECEIVE);
                    }
                }
                let incoming = match session.unpack(&frame) {
                    Ok(e) => e,
                    Err(e) => { warn!("dropping bad frame: {e}"); continue; }
                };
                salt.store(session.salt(), Ordering::Relaxed);
                for event in incoming {
                    handle(event, pending, queue, events);
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Ended::Broken("no data from server".into());
            }
            _ = tokio::time::sleep_until(answer_deadline), if answer_by.is_some() => {
                // `waitReceivedFailed`: a new connection, and more patience next time.
                let waited = timing.receive;
                timing.receive = (timing.receive * 2).min(MAX_RECEIVE);
                return Ended::Broken(format!("no answer in {waited:?}"));
            }
            _ = ping.tick() => {
                ping_id += 1;
                let ping = tdr_tl::mtproto::functions::PingDelayDisconnect {
                    ping_id,
                    disconnect_delay: PING_DISCONNECT_DELAY,
                };
                queue.push(Request { body: Body::Raw(ping.to_bytes()), reply: None, sent: None });
            }
        }
    }
}

fn handle(
    event: Incoming,
    pending: &mut HashMap<i64, Request>,
    queue: &mut Vec<Request>,
    events: &mpsc::UnboundedSender<Event>,
) {
    match event {
        Incoming::Result { req_msg_id, result } => {
            if let Some(r) = pending.remove(&req_msg_id) {
                r.answer(result.map_err(Error::Rpc));
            }
        }
        Incoming::Pong { msg_id } => {
            pending.remove(&msg_id);
        }
        Incoming::Resend { bad_msg_id } => {
            debug!(bad_msg_id, "server asked to resend (new salt or clock fix)");
            let mut resend: Vec<Request> = match pending.remove(&bad_msg_id) {
                Some(r) => vec![r],
                // The bad id may be a container: resend everything in flight.
                None => pending.drain().map(|(_, r)| r).collect(),
            };
            for r in &mut resend {
                r.sent = None; // A new salt or clock needs a new msg_id.
            }
            queue.extend(resend);
        }
        Incoming::BadMessage { bad_msg_id, code } => {
            warn!(bad_msg_id, code, "bad_msg_notification");
            if let Some(r) = pending.remove(&bad_msg_id) {
                r.answer(Err(Error::Security("bad_msg_notification")));
            }
        }
        Incoming::NewSession => {
            let _ = events.send(Event::NewSession);
        }
        Incoming::Update(u) => {
            let _ = events.send(Event::Update(u));
        }
    }
}

/// Sends what is queued; returns the size of the requests sent (acks alone count 0).
async fn flush(
    session: &mut Session,
    conn: &mut Connection,
    pending: &mut HashMap<i64, Request>,
    queue: &mut Vec<Request>,
) -> std::io::Result<usize> {
    let mut sent = 0;
    // A bind request must travel alone (its inner message repeats the outer msg_id).
    while let Some(pos) = queue
        .iter()
        .position(|r| matches!(r.body, Body::Bind { .. }))
    {
        let mut r = queue.remove(pos);
        let Body::Bind { perm, expires_at } = &r.body else {
            unreachable!()
        };
        let (frame, msg_id) = session.pack_bind(perm, *expires_at);
        r.sent = Some((msg_id, 0));
        pending.insert(msg_id, r);
        sent += frame.len();
        conn.writer.send(&frame).await?;
    }
    while !queue.is_empty() || session.has_pending_acks() {
        let (frame, ids) = {
            let items: Vec<Outbound<'_>> = queue
                .iter()
                .map(|r| match &r.body {
                    Body::Raw(b) => (b.as_slice(), r.sent),
                    Body::Bind { .. } | Body::DropConnection => unreachable!("never queued"),
                })
                .collect();
            session.pack(&items)
        };
        // Requests that did not fit stay in the queue for the next frame.
        let rest = queue.split_off(ids.len());
        let packed = std::mem::replace(queue, rest);
        let requests = !packed.is_empty();
        for (id, mut r) in ids.into_iter().zip(packed) {
            r.sent = Some(id);
            pending.insert(id.0, r);
        }
        if requests {
            sent += frame.len();
        }
        conn.writer.send(&frame).await?;
    }
    Ok(sent)
}
