//! Encrypted MTProto session state machine (no I/O).
//!
//! Turns outgoing request bodies into encrypted frames (batching several requests and
//! pending acks into one `msg_container`) and incoming frames into [`Incoming`] events.

use std::io::Read;

use bytes::Bytes;
use tdr_crypto::{AuthKey, Side};
use tdr_tl::mtproto::enums;
use tdr_tl::{Deserializable, Reader, Serializable};

use crate::{Error, MsgIdGen, RpcError};

const RPC_RESULT: u32 = 0xf35c6d01;
const RPC_ERROR: u32 = 0x2144ca19;
const GZIP_PACKED: u32 = 0x3072cfa1;
const MSG_CONTAINER: u32 = 0x73f1f8dc;
const MSGS_ACK: u32 = 0x62d6b459;
const BAD_SERVER_SALT: u32 = 0xedab447b;
const BAD_MSG_NOTIFICATION: u32 = 0xa7eff811;
const NEW_SESSION_CREATED: u32 = 0x9ec20908;
const PONG: u32 = 0x347773c5;
const FUTURE_SALTS: u32 = 0xae500895;
const MSG_DETAILED_INFO: u32 = 0x276d3ec6;
const MSG_NEW_DETAILED_INFO: u32 = 0x809db6df;

/// Max bytes of payload packed into one container (the server limit is 1 MiB).
const MAX_CONTAINER_BYTES: usize = 512 * 1024;
const MAX_CONTAINER_MESSAGES: usize = 1020;

/// What the server told us.
#[derive(Debug)]
pub enum Incoming {
    /// The answer to a request.
    Result {
        req_msg_id: i64,
        result: Result<Bytes, RpcError>,
    },
    /// The message must be resent (new salt or corrected clock already applied).
    Resend { bad_msg_id: i64 },
    /// A fatal problem with a message (`bad_msg_notification` with other codes).
    BadMessage { bad_msg_id: i64, code: i32 },
    /// The server created a new session; updates may have been lost.
    NewSession,
    /// An update (`Updates` and friends), raw.
    Update(Bytes),
    /// The answer to a ping.
    Pong { msg_id: i64 },
}

/// `(msg_id, seq_no)` assigned to a sent message.
pub type MsgSlot = (i64, i32);

/// A request body and the slot it was sent with before (for resends), if any.
pub type Outbound<'a> = (&'a [u8], Option<MsgSlot>);

pub struct Session {
    auth_key: AuthKey,
    session_id: i64,
    salt: i64,
    ids: MsgIdGen,
    /// Number of content-related messages sent.
    content_sent: i32,
    pending_acks: Vec<i64>,
}

/// One message ready to be packed: `(msg_id, seq_no, body)`.
struct Outgoing {
    msg_id: i64,
    seq_no: i32,
    body: Vec<u8>,
}

impl Session {
    pub fn new(auth_key: AuthKey, salt: i64, time_offset: i32) -> Self {
        Self {
            auth_key,
            session_id: i64::from_le_bytes(tdr_crypto::random_bytes()),
            salt,
            ids: MsgIdGen::new(time_offset),
            content_sent: 0,
            pending_acks: Vec::new(),
        }
    }

    pub fn auth_key(&self) -> &AuthKey {
        &self.auth_key
    }

    pub fn salt(&self) -> i64 {
        self.salt
    }

    pub fn time_offset(&self) -> i32 {
        self.ids.time_offset
    }

    pub fn has_pending_acks(&self) -> bool {
        !self.pending_acks.is_empty()
    }

    fn seq_no(&mut self, content_related: bool) -> i32 {
        if content_related {
            self.content_sent += 1;
            self.content_sent * 2 - 1
        } else {
            self.content_sent * 2
        }
    }

    fn outgoing(&mut self, body: Vec<u8>, content_related: bool) -> Outgoing {
        Outgoing {
            msg_id: self.ids.next_id(),
            seq_no: self.seq_no(content_related),
            body,
        }
    }

    /// A message id assigned earlier is still usable for a resend if it is not too old
    /// (the server rejects ids more than 300 s in the past).
    pub fn is_fresh(&self, msg_id: i64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + self.ids.time_offset as i64;
        now - (msg_id >> 32) < 240
    }

    /// Packs as many `requests` as fit into one encrypted frame (plus pending acks).
    ///
    /// Each request is `(body, previous (msg_id, seq_no))`: a request resent after a
    /// reconnect keeps its id, so the server answers from its cache instead of executing
    /// it twice. Returns the frame and the `(msg_id, seq_no)` of each packed request, in
    /// order; requests beyond the returned count were not packed and must be passed again.
    pub fn pack(&mut self, requests: &[Outbound<'_>]) -> (Vec<u8>, Vec<MsgSlot>) {
        let mut messages = Vec::new();
        if !self.pending_acks.is_empty() {
            let ack = enums::MsgsAck::MsgsAck(Box::new(tdr_tl::mtproto::types::MsgsAck {
                msg_ids: std::mem::take(&mut self.pending_acks),
            }));
            messages.push(self.outgoing(ack.to_bytes(), false));
        }
        let mut ids = Vec::new();
        let mut size = 0;
        for (body, previous) in requests {
            if !ids.is_empty()
                && (size + body.len() > MAX_CONTAINER_BYTES
                    || messages.len() >= MAX_CONTAINER_MESSAGES)
            {
                break;
            }
            size += body.len();
            let m = match previous {
                Some((msg_id, seq_no)) => Outgoing {
                    msg_id: *msg_id,
                    seq_no: *seq_no,
                    body: body.to_vec(),
                },
                None => self.outgoing(body.to_vec(), true),
            };
            ids.push((m.msg_id, m.seq_no));
            messages.push(m);
        }

        let (msg_id, seq_no, body) = if messages.len() == 1 {
            let m = messages.pop().unwrap();
            (m.msg_id, m.seq_no, m.body)
        } else {
            let mut body =
                Vec::with_capacity(8 + messages.iter().map(|m| 16 + m.body.len()).sum::<usize>());
            body.extend_from_slice(&MSG_CONTAINER.to_le_bytes());
            body.extend_from_slice(&(messages.len() as u32).to_le_bytes());
            for m in &messages {
                body.extend_from_slice(&m.msg_id.to_le_bytes());
                body.extend_from_slice(&m.seq_no.to_le_bytes());
                body.extend_from_slice(&(m.body.len() as u32).to_le_bytes());
                body.extend_from_slice(&m.body);
            }
            // The container's id must be greater than the ids inside it.
            let c = self.outgoing(body, false);
            (c.msg_id, c.seq_no, c.body)
        };
        (self.encrypt(msg_id, seq_no, &body), ids)
    }

    fn encrypt(&self, msg_id: i64, seq_no: i32, body: &[u8]) -> Vec<u8> {
        let mut plain = Vec::with_capacity(32 + body.len() + 1024);
        plain.extend_from_slice(&self.salt.to_le_bytes());
        plain.extend_from_slice(&self.session_id.to_le_bytes());
        plain.extend_from_slice(&msg_id.to_le_bytes());
        plain.extend_from_slice(&seq_no.to_le_bytes());
        plain.extend_from_slice(&(body.len() as u32).to_le_bytes());
        plain.extend_from_slice(body);
        self.auth_key.encrypt_message(plain, Side::Client)
    }

    /// Packs `auth.bindTempAuthKey`, which binds this session's (temporary) key to `perm`.
    /// The inner message must carry the same msg_id as the outer one, so this is always
    /// sent alone. Returns the frame and the msg_id.
    pub fn pack_bind(&mut self, perm: &AuthKey, expires_at: i32) -> (Vec<u8>, i64) {
        let msg_id = self.ids.next_id();
        let seq_no = self.seq_no(true);
        let nonce = i64::from_le_bytes(tdr_crypto::random_bytes());
        let inner = enums::BindAuthKeyInner::BindAuthKeyInner(Box::new(
            tdr_tl::mtproto::types::BindAuthKeyInner {
                nonce,
                temp_auth_key_id: self.auth_key.id() as i64,
                perm_auth_key_id: perm.id() as i64,
                temp_session_id: self.session_id,
                expires_at,
            },
        ))
        .to_bytes();
        // MTProto 1.0 message: random salt + session_id, the same msg_id, seq_no 0.
        let mut plain = tdr_crypto::random_bytes::<16>().to_vec();
        plain.extend_from_slice(&msg_id.to_le_bytes());
        plain.extend_from_slice(&0i32.to_le_bytes());
        plain.extend_from_slice(&(inner.len() as u32).to_le_bytes());
        plain.extend_from_slice(&inner);
        let request = tdr_tl::functions::auth::BindTempAuthKey {
            perm_auth_key_id: perm.id() as i64,
            nonce,
            expires_at,
            encrypted_message: perm.encrypt_message_v1(plain).into(),
        }
        .to_bytes();
        (self.encrypt(msg_id, seq_no, &request), msg_id)
    }

    /// Decrypts a frame and turns it into events.
    pub fn unpack(&mut self, frame: &[u8]) -> Result<Vec<Incoming>, Error> {
        let plain = self.auth_key.decrypt_message(frame, Side::Server)?;
        let plain = Bytes::from(plain);
        let session_id = i64::from_le_bytes(plain[8..16].try_into().unwrap());
        if session_id != self.session_id {
            return Err(Error::Security("message for another session"));
        }
        let msg_id = i64::from_le_bytes(plain[16..24].try_into().unwrap());
        if msg_id % 2 == 0 {
            return Err(Error::Security("server msg_id must be odd"));
        }
        let seq_no = i32::from_le_bytes(plain[24..28].try_into().unwrap());
        let len = u32::from_le_bytes(plain[28..32].try_into().unwrap()) as usize;
        let body = plain.slice(32..32 + len);
        let mut events = Vec::new();
        self.process(msg_id, seq_no, body, &mut events)?;
        Ok(events)
    }

    fn process(
        &mut self,
        msg_id: i64,
        seq_no: i32,
        body: Bytes,
        events: &mut Vec<Incoming>,
    ) -> Result<(), Error> {
        if seq_no % 2 == 1 {
            self.pending_acks.push(msg_id);
        }
        let mut r = Reader::new(&body);
        let id = r.peek_u32()?;
        match id {
            MSG_CONTAINER => {
                r.read_u32()?;
                let count = r.read_u32()?;
                for _ in 0..count {
                    let inner_id = i64::deserialize(&mut r)?;
                    let inner_seq = i32::deserialize(&mut r)?;
                    let inner_len = u32::deserialize(&mut r)? as usize;
                    let start = r.position();
                    r.read_slice(inner_len)?;
                    self.process(
                        inner_id,
                        inner_seq,
                        body.slice(start..start + inner_len),
                        events,
                    )?;
                }
            }
            GZIP_PACKED => {
                let unpacked = gunzip(&body)?;
                self.process(msg_id, seq_no & !1, unpacked, events)?;
            }
            RPC_RESULT => {
                r.read_u32()?;
                let req_msg_id = i64::deserialize(&mut r)?;
                let mut result = r.rest();
                if Reader::new(&result).peek_u32()? == GZIP_PACKED {
                    result = gunzip(&result)?;
                }
                let result = if Reader::new(&result).peek_u32()? == RPC_ERROR {
                    let mut rr = Reader::new(&result);
                    rr.read_u32()?;
                    let code = i32::deserialize(&mut rr)?;
                    let message = String::from_utf8_lossy(rr.read_bytes_ref()?).into_owned();
                    Err(RpcError { code, message })
                } else {
                    Ok(result)
                };
                events.push(Incoming::Result { req_msg_id, result });
            }
            BAD_SERVER_SALT | BAD_MSG_NOTIFICATION => {
                match enums::BadMsgNotification::deserialize(&mut r)? {
                    enums::BadMsgNotification::BadServerSalt(x) => {
                        self.salt = x.new_server_salt;
                        events.push(Incoming::Resend {
                            bad_msg_id: x.bad_msg_id,
                        });
                    }
                    enums::BadMsgNotification::BadMsgNotification(x) => match x.error_code {
                        // msg_id too low / too high: fix the clock and resend.
                        16 | 17 => {
                            self.ids.sync_with_server_msg_id(msg_id);
                            events.push(Incoming::Resend {
                                bad_msg_id: x.bad_msg_id,
                            });
                        }
                        // seq_no problems: resend with fresh numbers.
                        32 | 33 => events.push(Incoming::Resend {
                            bad_msg_id: x.bad_msg_id,
                        }),
                        code => events.push(Incoming::BadMessage {
                            bad_msg_id: x.bad_msg_id,
                            code,
                        }),
                    },
                }
            }
            NEW_SESSION_CREATED => {
                let enums::NewSession::Created(x) = enums::NewSession::deserialize(&mut r)?;
                self.salt = x.server_salt;
                events.push(Incoming::NewSession);
            }
            PONG => {
                r.read_u32()?;
                events.push(Incoming::Pong {
                    msg_id: i64::deserialize(&mut r)?,
                });
            }
            MSGS_ACK | FUTURE_SALTS | MSG_DETAILED_INFO | MSG_NEW_DETAILED_INFO => {}
            _ => events.push(Incoming::Update(body)),
        }
        Ok(())
    }
}

fn gunzip(packed: &Bytes) -> Result<Bytes, Error> {
    let mut r = Reader::new(packed);
    r.read_u32()?; // gzip_packed id
    let data = r.read_bytes_ref()?;
    let mut out = Vec::with_capacity(data.len() * 4);
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|_| Error::Security("bad gzip_packed data"))?;
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_frame(s: &Session, msg_id: i64, seq_no: i32, body: &[u8]) -> Vec<u8> {
        let mut plain = Vec::new();
        plain.extend_from_slice(&s.salt.to_le_bytes());
        plain.extend_from_slice(&s.session_id.to_le_bytes());
        plain.extend_from_slice(&msg_id.to_le_bytes());
        plain.extend_from_slice(&seq_no.to_le_bytes());
        plain.extend_from_slice(&(body.len() as u32).to_le_bytes());
        plain.extend_from_slice(body);
        s.auth_key.encrypt_message(plain, Side::Server)
    }

    fn session() -> Session {
        Session::new(AuthKey::from_bytes(tdr_crypto::random_bytes()), 1, 0)
    }

    #[test]
    fn packs_single_and_container() {
        let mut s = session();
        let (frame, ids) = s.pack(&[(b"abcd", None)]);
        assert_eq!(ids.len(), 1);
        // Decrypt as the server would and check the layout.
        let plain = s.auth_key.decrypt_message(&frame, Side::Client).unwrap();
        assert_eq!(
            i64::from_le_bytes(plain[16..24].try_into().unwrap()),
            ids[0].0
        );
        assert_eq!(i32::from_le_bytes(plain[24..28].try_into().unwrap()), 1);

        let (frame, ids) = s.pack(&[(b"aaaa", None), (b"bbbb", None)]);
        assert_eq!(ids.len(), 2);
        let plain = s.auth_key.decrypt_message(&frame, Side::Client).unwrap();
        assert_eq!(
            u32::from_le_bytes(plain[32..36].try_into().unwrap()),
            MSG_CONTAINER
        );
        let container_id = i64::from_le_bytes(plain[16..24].try_into().unwrap());
        assert!(container_id > ids[1].0);

        // A resend keeps the original id.
        let (_, again) = s.pack(&[(b"aaaa", Some(ids[0]))]);
        assert_eq!(again[0], ids[0]);
    }

    #[test]
    fn parses_rpc_result_and_error() {
        let mut s = session();
        let mut body = RPC_RESULT.to_le_bytes().to_vec();
        body.extend_from_slice(&77i64.to_le_bytes());
        body.extend_from_slice(&RPC_ERROR.to_le_bytes());
        body.extend_from_slice(&420i32.to_le_bytes());
        tdr_tl::write_tl_bytes(&mut body, b"FLOOD_WAIT_3");
        let frame = server_frame(&s, 0x5555_0001, 1, &body);
        let events = s.unpack(&frame).unwrap();
        match &events[..] {
            [
                Incoming::Result {
                    req_msg_id: 77,
                    result: Err(e),
                },
            ] => {
                assert_eq!(e.code, 420);
                assert_eq!(e.value(), Some(3));
            }
            other => panic!("{other:?}"),
        }
        // Content-related message → ack is queued.
        assert!(s.has_pending_acks());
    }

    #[test]
    fn bad_salt_updates_salt() {
        let mut s = session();
        let mut body = BAD_SERVER_SALT.to_le_bytes().to_vec();
        body.extend_from_slice(&10i64.to_le_bytes());
        body.extend_from_slice(&1i32.to_le_bytes());
        body.extend_from_slice(&48i32.to_le_bytes());
        body.extend_from_slice(&999i64.to_le_bytes());
        let frame = server_frame(&s, 0x5555_0003, 0, &body);
        let events = s.unpack(&frame).unwrap();
        assert!(matches!(events[..], [Incoming::Resend { bad_msg_id: 10 }]));
        assert_eq!(s.salt(), 999);
    }
}
