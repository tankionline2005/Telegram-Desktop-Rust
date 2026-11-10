//! MTProto 2.0 client side.
//!
//! * [`transport`] — obfuscated intermediate TCP transport;
//! * [`handshake`] — creation of an auth key;
//! * [`session`] — encrypted session state (msg_id/seq_no, containers, acks, salts);
//! * [`Sender`] — async request/response over one connection.

pub mod handshake;
pub mod sender;
pub mod session;
pub mod transport;

use std::time::{SystemTime, UNIX_EPOCH};

pub use sender::{Sender, Update};

/// Production DC addresses built into tdesktop (`mtproto_dc_options.cpp`).
pub const PRODUCTION_DCS: &[(i32, &str)] = &[
    (1, "149.154.175.50:443"),
    (2, "149.154.167.51:443"),
    (3, "149.154.175.100:443"),
    (4, "149.154.167.91:443"),
    (5, "149.154.171.5:443"),
];

/// Test environment DC addresses.
pub const TEST_DCS: &[(i32, &str)] = &[
    (1, "149.154.175.10:443"),
    (2, "149.154.167.40:443"),
    (3, "149.154.175.117:443"),
];

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Deserialize(tdr_tl::DeserializeError),
    Decrypt(tdr_crypto::DecryptError),
    /// A check from the MTProto security guidelines failed.
    Security(&'static str),
    /// The server answered the request with `rpc_error`.
    Rpc(RpcError),
    /// The connection was closed while the request was pending.
    Disconnected,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Deserialize(e) => write!(f, "bad TL data: {e}"),
            Self::Decrypt(e) => write!(f, "decryption failed: {e}"),
            Self::Security(e) => write!(f, "security check failed: {e}"),
            Self::Rpc(e) => write!(f, "{e}"),
            Self::Disconnected => f.write_str("disconnected"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<tdr_tl::DeserializeError> for Error {
    fn from(e: tdr_tl::DeserializeError) -> Self {
        Self::Deserialize(e)
    }
}
impl From<tdr_crypto::DecryptError> for Error {
    fn from(e: tdr_crypto::DecryptError) -> Self {
        Self::Decrypt(e)
    }
}

/// `rpc_error`, e.g. `420 FLOOD_WAIT_17` or `303 PHONE_MIGRATE_2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    /// Numeric suffix of messages like `FLOOD_WAIT_17` or `USER_MIGRATE_4`.
    pub fn value(&self) -> Option<i32> {
        self.message.rsplit('_').next()?.parse().ok()
    }

    /// Whether the message is `prefix` or `prefix_<N>`.
    pub fn is(&self, prefix: &str) -> bool {
        self.message == prefix
            || self
                .message
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('_') && rest[1..].parse::<i64>().is_ok())
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RPC error {} {}", self.code, self.message)
    }
}

/// Monotonic generator of client `msg_id`s (`unixtime * 2^32 + fraction`, divisible by 4).
#[derive(Debug, Clone)]
pub struct MsgIdGen {
    last: i64,
    /// `server_time - local_time`, seconds.
    pub time_offset: i32,
}

impl MsgIdGen {
    pub fn new(time_offset: i32) -> Self {
        Self {
            last: 0,
            time_offset,
        }
    }

    pub fn next_id(&mut self) -> i64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let secs = now.as_secs() as i64 + self.time_offset as i64;
        let frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
        let mut id = (secs << 32) | (frac as i64 & !3);
        if id <= self.last {
            id = self.last + 4;
        }
        self.last = id;
        id
    }

    /// Adjusts the clock using a server `msg_id` (after `bad_msg_notification` 16/17).
    pub fn sync_with_server_msg_id(&mut self, server_msg_id: i64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.time_offset = ((server_msg_id >> 32) - now) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_ids_are_monotonic_and_aligned() {
        let mut g = MsgIdGen::new(0);
        let mut prev = 0;
        for _ in 0..10_000 {
            let id = g.next_id();
            assert!(id > prev);
            assert_eq!(id % 4, 0);
            prev = id;
        }
    }

    #[test]
    fn rpc_error_helpers() {
        let e = RpcError {
            code: 420,
            message: "FLOOD_WAIT_17".into(),
        };
        assert_eq!(e.value(), Some(17));
        assert!(e.is("FLOOD_WAIT"));
        assert!(!e.is("FLOOD"));
        let e = RpcError {
            code: 401,
            message: "SESSION_PASSWORD_NEEDED".into(),
        };
        assert!(e.is("SESSION_PASSWORD_NEEDED"));
    }
}
