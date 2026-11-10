//! Obfuscated "intermediate" TCP transport (the same protocol family tdesktop uses).
//!
//! The 64-byte random init header makes the stream look like noise to DPI; every frame is
//! `len: u32 LE` + payload, encrypted with AES-256-CTR in both directions.

use std::io;

use aes::Aes256;
use bytes::{Bytes, BytesMut};
use ctr::cipher::{KeyIvInit, StreamCipher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

type Ctr = ctr::Ctr128BE<Aes256>;

const INTERMEDIATE_TAG: [u8; 4] = [0xee; 4];
/// Frames bigger than this are a protocol violation (MTProto messages are ≤ 1 MiB + overhead).
const MAX_FRAME: usize = 2 * 1024 * 1024;

fn new_ctr(key: &[u8], iv: &[u8]) -> Ctr {
    Ctr::new_from_slices(key, iv).expect("key/iv sizes are fixed")
}

/// Builds the init header. Returns (bytes to send, encryptor, decryptor).
fn obfuscation_init(dc_id: i16) -> ([u8; 64], Ctr, Ctr) {
    let mut init = [0u8; 64];
    loop {
        tdr_crypto::fill_random(&mut init);
        let first4 = [init[0], init[1], init[2], init[3]];
        let bad_first = init[0] == 0xef
            || matches!(&first4, b"HEAD" | b"POST" | b"GET " | b"OPTI")
            || first4 == [0xdd; 4]
            || first4 == [0xee; 4]
            || first4 == [0x16, 0x03, 0x01, 0x02]
            || init[4..8] == [0, 0, 0, 0];
        if !bad_first {
            break;
        }
    }
    init[56..60].copy_from_slice(&INTERMEDIATE_TAG);
    init[60..62].copy_from_slice(&dc_id.to_le_bytes());

    let mut reversed = [0u8; 48];
    reversed.copy_from_slice(&init[8..56]);
    reversed.reverse();

    let mut encryptor = new_ctr(&init[8..40], &init[40..56]);
    let decryptor = new_ctr(&reversed[..32], &reversed[32..48]);

    // The tail of the header is sent encrypted; the encryptor keeps its counter state.
    let mut encrypted = init;
    encryptor.apply_keystream(&mut encrypted);
    init[56..].copy_from_slice(&encrypted[56..]);
    (init, encryptor, decryptor)
}

/// An error code sent by the server instead of a message, e.g. `-404` when the auth key
/// is unknown (a temporary key expired or was destroyed) or `-429` on flood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportError(pub i32);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transport error {}", self.0)
    }
}

impl std::error::Error for TransportError {}

impl TransportError {
    /// Extracts the code from an I/O error produced by [`Reader::recv`].
    pub fn from_io(e: &io::Error) -> Option<i32> {
        e.get_ref()?.downcast_ref::<TransportError>().map(|t| t.0)
    }
}

pub struct Reader {
    stream: OwnedReadHalf,
    cipher: Ctr,
}

pub struct Writer {
    stream: OwnedWriteHalf,
    cipher: Ctr,
}

/// Connects and sends the obfuscation header.
pub async fn connect(addr: &str, dc_id: i16) -> io::Result<(Reader, Writer)> {
    let started = std::time::Instant::now();
    let stream = TcpStream::connect(addr).await?;
    tracing::debug!(elapsed = ?started.elapsed(), addr, "TCP connected");
    stream.set_nodelay(true)?;
    let (r, mut w) = stream.into_split();
    let (init, encryptor, decryptor) = obfuscation_init(dc_id);
    w.write_all(&init).await?;
    Ok((
        Reader {
            stream: r,
            cipher: decryptor,
        },
        Writer {
            stream: w,
            cipher: encryptor,
        },
    ))
}

impl Writer {
    pub async fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload);
        self.cipher.apply_keystream(&mut frame);
        self.stream.write_all(&frame).await
    }
}

impl Reader {
    /// Reads one frame. A 4-byte frame is a transport error code from the server.
    pub async fn recv(&mut self) -> io::Result<Bytes> {
        let mut len = [0u8; 4];
        self.stream.read_exact(&mut len).await?;
        self.cipher.apply_keystream(&mut len);
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame too big: {len}"),
            ));
        }
        let mut buf = BytesMut::zeroed(len);
        self.stream.read_exact(&mut buf).await?;
        self.cipher.apply_keystream(&mut buf);
        if len == 4 {
            let code = i32::from_le_bytes(buf[..4].try_into().unwrap());
            return Err(io::Error::other(TransportError(code)));
        }
        Ok(buf.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_header_rules() {
        for _ in 0..200 {
            let (init, _, _) = obfuscation_init(2);
            assert_ne!(init[0], 0xef);
            assert_ne!(&init[4..8], &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn server_can_decrypt_our_stream() {
        // Simulate the server: it derives the same keys from the header.
        let (init, mut enc, _) = obfuscation_init(2);
        let mut server_dec = new_ctr(&init[8..40], &init[40..56]);
        let mut header = init;
        server_dec.apply_keystream(&mut header);
        assert_eq!(&header[56..60], &INTERMEDIATE_TAG);
        assert_eq!(i16::from_le_bytes([header[60], header[61]]), 2);

        let mut msg = *b"hello";
        enc.apply_keystream(&mut msg);
        server_dec.apply_keystream(&mut msg);
        assert_eq!(&msg, b"hello");
    }
}
