//! Async request/response over a single connection.
//!
//! One task owns the socket writer and the [`Session`]; a second task only reads frames
//! (reads are not cancel-safe, so they never sit inside `select!`). Requests submitted
//! while a write is in flight are batched into one container on the next write.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use tdr_crypto::AuthKey;
use tdr_tl::{Deserializable, Reader, RemoteCall, Serializable};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use crate::session::{Incoming, Session};
use crate::{Error, handshake, transport};

/// A raw update from the server (`Updates` or one of its constructors).
pub type Update = Bytes;

struct Request {
    body: Vec<u8>,
    reply: oneshot::Sender<Result<Bytes, Error>>,
}

/// Handle to a connection. Cheap to clone.
#[derive(Clone)]
pub struct Sender {
    tx: mpsc::UnboundedSender<Request>,
}

/// What `Sender::connect` produced.
pub struct Connected {
    pub sender: Sender,
    pub updates: mpsc::UnboundedReceiver<Update>,
    pub auth_key: AuthKey,
    pub time_offset: i32,
}

const PING_INTERVAL: Duration = Duration::from_secs(60);

impl Sender {
    /// Connects to `addr`. Without `auth_key` a new key is created first.
    pub async fn connect(
        addr: &str,
        dc_id: i32,
        auth_key: Option<(AuthKey, i32)>,
        test: bool,
    ) -> Result<Connected, Error> {
        let transport_dc = if test { dc_id + 10_000 } else { dc_id } as i16;
        let (mut reader, mut writer) = transport::connect(addr, transport_dc).await?;
        let (auth_key, time_offset, salt) = match auth_key {
            Some((key, offset)) => (key, offset, 0),
            None => {
                let created = handshake::create_auth_key(
                    &mut reader,
                    &mut writer,
                    if test { dc_id + 10_000 } else { dc_id },
                    test,
                )
                .await?;
                debug!(key = ?created.auth_key, offset = created.time_offset, "auth key created");
                (created.auth_key, created.time_offset, created.first_salt)
            }
        };
        let session = Session::new(auth_key.clone(), salt, time_offset);

        let (tx, rx) = mpsc::unbounded_channel();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (frames_tx, frames_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let frame = reader.recv().await;
                let stop = frame.is_err();
                if frames_tx.send(frame).is_err() || stop {
                    break;
                }
            }
        });
        tokio::spawn(run(session, writer, rx, frames_rx, updates_tx));
        Ok(Connected {
            sender: Sender { tx },
            updates,
            auth_key,
            time_offset,
        })
    }

    /// Sends a request and waits for its result.
    pub async fn invoke<R: RemoteCall>(&self, request: &R) -> Result<R::Return, Error> {
        let bytes = self.invoke_raw(request.to_bytes()).await?;
        Ok(R::Return::deserialize(&mut Reader::new(&bytes))?)
    }

    /// Sends a serialized request; returns the raw result.
    pub async fn invoke_raw(&self, body: Vec<u8>) -> Result<Bytes, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request { body, reply })
            .map_err(|_| Error::Disconnected)?;
        rx.await.map_err(|_| Error::Disconnected)?
    }
}

async fn run(
    mut session: Session,
    mut writer: transport::Writer,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut frames: mpsc::UnboundedReceiver<std::io::Result<Bytes>>,
    updates: mpsc::UnboundedSender<Update>,
) {
    // msg_id → request awaiting its answer.
    let mut pending: HashMap<i64, Request> = HashMap::new();
    // Requests to (re)send on the next write.
    let mut queue: Vec<Request> = Vec::new();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut ping_id = 0i64;

    let fail_all = |pending: &mut HashMap<i64, Request>, queue: &mut Vec<Request>, why: &str| {
        warn!("connection closed: {why}");
        for (_, r) in pending.drain() {
            let _ = r.reply.send(Err(Error::Disconnected));
        }
        for r in queue.drain(..) {
            let _ = r.reply.send(Err(Error::Disconnected));
        }
    };

    loop {
        tokio::select! {
            req = requests.recv() => match req {
                Some(r) => {
                    queue.push(r);
                    // Batch everything that is already waiting.
                    while let Ok(r) = requests.try_recv() {
                        queue.push(r);
                    }
                }
                None => break,
            },
            frame = frames.recv() => {
                let frame = match frame {
                    Some(Ok(f)) => f,
                    Some(Err(e)) => { fail_all(&mut pending, &mut queue, &e.to_string()); return; }
                    None => { fail_all(&mut pending, &mut queue, "reader stopped"); return; }
                };
                let events = match session.unpack(&frame) {
                    Ok(e) => e,
                    Err(e) => { warn!("dropping bad frame: {e}"); continue; }
                };
                for event in events {
                    match event {
                        Incoming::Result { req_msg_id, result } => {
                            if let Some(r) = pending.remove(&req_msg_id) {
                                let _ = r.reply.send(result.map_err(Error::Rpc));
                            }
                        }
                        Incoming::Resend { bad_msg_id } => {
                            if let Some(r) = pending.remove(&bad_msg_id) {
                                queue.push(r);
                            } else {
                                // The bad id may be a container: resend everything in flight.
                                queue.extend(pending.drain().map(|(_, r)| r));
                            }
                        }
                        Incoming::BadMessage { bad_msg_id, code } => {
                            warn!(bad_msg_id, code, "bad_msg_notification");
                            if let Some(r) = pending.remove(&bad_msg_id) {
                                let _ = r.reply.send(Err(Error::Security("bad_msg_notification")));
                            }
                        }
                        Incoming::NewSession => debug!("new session created"),
                        Incoming::Update(u) => { let _ = updates.send(u); }
                    }
                }
            }
            _ = ping.tick() => {
                ping_id += 1;
                let ping = tdr_tl::mtproto::functions::PingDelayDisconnect {
                    ping_id,
                    disconnect_delay: (PING_INTERVAL.as_secs() + 15) as i32,
                };
                let (reply, _) = oneshot::channel();
                queue.push(Request { body: ping.to_bytes(), reply });
            }
        }

        // Flush the queue (and acks) in as few frames as possible.
        while !queue.is_empty() || session.has_pending_acks() {
            let bodies: Vec<&[u8]> = queue.iter().map(|r| r.body.as_slice()).collect();
            let (frame, ids) = session.pack(&bodies);
            let sent: Vec<Request> = queue.drain(..ids.len()).collect();
            for (id, r) in ids.into_iter().zip(sent) {
                pending.insert(id, r);
            }
            if let Err(e) = writer.send(&frame).await {
                fail_all(&mut pending, &mut queue, &e.to_string());
                return;
            }
        }
    }
}
