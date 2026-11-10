//! MTProto 2.0 cryptography.
//!
//! * [`aes_ige_encrypt`] / [`aes_ige_decrypt`] — AES-256-IGE (AES-NI is used automatically);
//! * [`AuthKey`] with [`AuthKey::encrypt_message`] / [`AuthKey::decrypt_message`];
//! * [`rsa`] — server public keys and the `RSA_PAD` scheme for the key exchange;
//! * [`dh`] — validation of Diffie-Hellman parameters;
//! * [`factorize`] — `pq` decomposition (Pollard–Brent).

pub mod dh;
pub mod rsa;
pub mod srp;

use aes::Aes256;
use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use sha1::Sha1;
use sha2::{Digest, Sha256};

pub fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Cryptographically secure random bytes from the OS.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    fill_random(&mut buf);
    buf
}

pub fn fill_random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS random number generator failed");
}

fn xor16(a: &mut [u8], b: &[u8]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

/// AES-256-IGE encryption in place. `data.len()` must be a multiple of 16.
pub fn aes_ige_encrypt(data: &mut [u8], key: &[u8; 32], iv: &[u8; 32]) {
    assert_eq!(data.len() % 16, 0, "IGE data must be block-aligned");
    let cipher = Aes256::new(key.into());
    let mut prev_c: [u8; 16] = iv[..16].try_into().unwrap();
    let mut prev_p: [u8; 16] = iv[16..].try_into().unwrap();
    for block in data.as_chunks_mut::<16>().0 {
        let plain: [u8; 16] = *block;
        xor16(block, &prev_c);
        cipher.encrypt_block(block.into());
        xor16(block, &prev_p);
        prev_c.copy_from_slice(block);
        prev_p = plain;
    }
}

/// AES-256-IGE decryption in place. `data.len()` must be a multiple of 16.
pub fn aes_ige_decrypt(data: &mut [u8], key: &[u8; 32], iv: &[u8; 32]) {
    assert_eq!(data.len() % 16, 0, "IGE data must be block-aligned");
    let cipher = Aes256::new(key.into());
    let mut prev_c: [u8; 16] = iv[..16].try_into().unwrap();
    let mut prev_p: [u8; 16] = iv[16..].try_into().unwrap();
    for block in data.as_chunks_mut::<16>().0 {
        let cipher_block: [u8; 16] = *block;
        xor16(block, &prev_p);
        cipher.decrypt_block(block.into());
        xor16(block, &prev_c);
        prev_p.copy_from_slice(block);
        prev_c = cipher_block;
    }
}

/// Which side encrypted a message; selects the `x` offset of MTProto 2.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

impl Side {
    fn x(self) -> usize {
        match self {
            Side::Client => 0,
            Side::Server => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptError {
    TooShort,
    WrongAuthKey,
    MsgKeyMismatch,
    BadLength,
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooShort => "encrypted message is too short",
            Self::WrongAuthKey => "message is encrypted with another auth key",
            Self::MsgKeyMismatch => "msg_key check failed",
            Self::BadLength => "inner message length is invalid",
        })
    }
}

impl std::error::Error for DecryptError {}

/// A 2048-bit MTProto authorization key.
#[derive(Clone)]
pub struct AuthKey {
    data: [u8; 256],
    id: [u8; 8],
    aux_hash: [u8; 8],
}

impl std::fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key itself.
        write!(f, "AuthKey({:016x})", self.id())
    }
}

impl AuthKey {
    pub fn from_bytes(data: [u8; 256]) -> Self {
        let hash = sha1(&[&data]);
        Self {
            data,
            aux_hash: hash[..8].try_into().unwrap(),
            id: hash[12..20].try_into().unwrap(),
        }
    }

    pub fn to_bytes(&self) -> [u8; 256] {
        self.data
    }

    /// `auth_key_id`: the lower 64 bits of SHA1(auth_key).
    pub fn id(&self) -> u64 {
        u64::from_le_bytes(self.id)
    }

    /// The higher 64 bits of SHA1(auth_key), used in the key exchange.
    pub fn aux_hash(&self) -> [u8; 8] {
        self.aux_hash
    }

    fn aes_key_iv(&self, msg_key: &[u8; 16], side: Side) -> ([u8; 32], [u8; 32]) {
        let x = side.x();
        let a = sha256(&[msg_key, &self.data[x..x + 36]]);
        let b = sha256(&[&self.data[40 + x..40 + x + 36], msg_key]);
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&a[..8]);
        key[8..24].copy_from_slice(&b[8..24]);
        key[24..].copy_from_slice(&a[24..]);
        let mut iv = [0u8; 32];
        iv[..8].copy_from_slice(&b[..8]);
        iv[8..24].copy_from_slice(&a[8..24]);
        iv[24..].copy_from_slice(&b[24..]);
        (key, iv)
    }

    fn msg_key(&self, plaintext: &[u8], side: Side) -> [u8; 16] {
        let x = side.x();
        let large = sha256(&[&self.data[88 + x..88 + x + 32], plaintext]);
        large[8..24].try_into().unwrap()
    }

    /// Encrypts an inner message (`salt + session_id + msg_id + seq_no + len + body`).
    /// Adds 12..=1024 bytes of random padding. Returns `auth_key_id + msg_key + data`.
    pub fn encrypt_message(&self, mut plaintext: Vec<u8>, side: Side) -> Vec<u8> {
        // Padding: at least 12 bytes, total a multiple of 16; a little randomness in length.
        let extra = random_bytes::<1>()[0] as usize % 4 * 16;
        let padding = 12 + (16 - (plaintext.len() + 12) % 16) % 16 + extra;
        let start = plaintext.len();
        plaintext.resize(start + padding, 0);
        fill_random(&mut plaintext[start..]);

        let msg_key = self.msg_key(&plaintext, side);
        let (key, iv) = self.aes_key_iv(&msg_key, side);
        aes_ige_encrypt(&mut plaintext, &key, &iv);

        let mut out = Vec::with_capacity(24 + plaintext.len());
        out.extend_from_slice(&self.id);
        out.extend_from_slice(&msg_key);
        out.extend_from_slice(&plaintext);
        out
    }

    /// Decrypts a message encrypted by `side`. Returns the inner message with padding.
    pub fn decrypt_message(&self, data: &[u8], side: Side) -> Result<Vec<u8>, DecryptError> {
        if data.len() < 24 + 32 || !(data.len() - 24).is_multiple_of(16) {
            return Err(DecryptError::TooShort);
        }
        if data[..8] != self.id {
            return Err(DecryptError::WrongAuthKey);
        }
        let msg_key: [u8; 16] = data[8..24].try_into().unwrap();
        let (key, iv) = self.aes_key_iv(&msg_key, side);
        let mut plain = data[24..].to_vec();
        aes_ige_decrypt(&mut plain, &key, &iv);

        // Security guidelines: msg_key must be recomputed and compared.
        if self.msg_key(&plain, side) != msg_key {
            return Err(DecryptError::MsgKeyMismatch);
        }
        let len = u32::from_le_bytes(plain[28..32].try_into().unwrap()) as usize;
        let padding = plain
            .len()
            .checked_sub(32 + len)
            .ok_or(DecryptError::BadLength)?;
        if !len.is_multiple_of(4) || !(12..=1024).contains(&padding) {
            return Err(DecryptError::BadLength);
        }
        Ok(plain)
    }

    /// MTProto 1.0 key derivation (SHA1-based), still used for `bind_auth_key_inner`.
    fn aes_key_iv_v1(&self, msg_key: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
        let k = &self.data;
        let a = sha1(&[msg_key, &k[..32]]);
        let b = sha1(&[&k[32..48], msg_key, &k[48..64]]);
        let c = sha1(&[&k[64..96], msg_key]);
        let d = sha1(&[msg_key, &k[96..128]]);
        let mut key = [0u8; 32];
        key[..8].copy_from_slice(&a[..8]);
        key[8..20].copy_from_slice(&b[8..20]);
        key[20..].copy_from_slice(&c[4..16]);
        let mut iv = [0u8; 32];
        iv[..12].copy_from_slice(&a[8..20]);
        iv[12..20].copy_from_slice(&b[..8]);
        iv[20..24].copy_from_slice(&c[16..20]);
        iv[24..].copy_from_slice(&d[..8]);
        (key, iv)
    }

    /// MTProto 1.0 encryption, only for the `encrypted_message` of `auth.bindTempAuthKey`.
    /// `plaintext` is the full inner message without padding.
    pub fn encrypt_message_v1(&self, mut plaintext: Vec<u8>) -> Vec<u8> {
        let msg_key: [u8; 16] = sha1(&[&plaintext])[4..20].try_into().unwrap();
        let start = plaintext.len();
        plaintext.resize(start.next_multiple_of(16), 0);
        fill_random(&mut plaintext[start..]);
        let (key, iv) = self.aes_key_iv_v1(&msg_key);
        aes_ige_encrypt(&mut plaintext, &key, &iv);
        let mut out = Vec::with_capacity(24 + plaintext.len());
        out.extend_from_slice(&self.id);
        out.extend_from_slice(&msg_key);
        out.extend_from_slice(&plaintext);
        out
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    ((a as u128 * b as u128) % m as u128) as u64
}

/// Splits `pq` into two primes `(p, q)` with `p < q` (Pollard's rho, Brent's variant).
pub fn factorize(pq: u64) -> Option<(u64, u64)> {
    if pq < 4 {
        return None;
    }
    if pq.is_multiple_of(2) {
        return Some((2, pq / 2));
    }
    for c in 1..64u64 {
        let f = |x: u64| (mul_mod(x, x, pq) + c) % pq;
        let (mut y, m) = (2u64, 128u64);
        let (mut g, mut r, mut q) = (1u64, 1u64, 1u64);
        let (mut x, mut ys) = (0u64, 0u64);
        while g == 1 {
            x = y;
            for _ in 0..r {
                y = f(y);
            }
            let mut k = 0;
            while k < r && g == 1 {
                ys = y;
                for _ in 0..m.min(r - k) {
                    y = f(y);
                    q = mul_mod(q, x.abs_diff(y), pq);
                }
                g = gcd(q, pq);
                k += m;
            }
            r *= 2;
        }
        if g == pq {
            loop {
                ys = f(ys);
                g = gcd(x.abs_diff(ys), pq);
                if g > 1 {
                    break;
                }
            }
        }
        if g != pq && g > 1 {
            let (a, b) = (g, pq / g);
            return Some((a.min(b), a.max(b)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factorizes_like_the_docs() {
        // Example from https://core.telegram.org/mtproto/samples-auth_key
        assert_eq!(
            factorize(0x17ED48941A08F981),
            Some((0x494C553B, 0x53911073))
        );
        assert_eq!(factorize(15), Some((3, 5)));
    }

    #[test]
    fn aes_ige_matches_definition() {
        // With zero key, iv and data IGE gives c1 = E(0), c2 = E(c1) xor p1 = E(E(0)).
        let key = [0u8; 32];
        let cipher = Aes256::new((&key).into());
        let mut e0 = [0u8; 16];
        cipher.encrypt_block((&mut e0).into());
        let mut e1 = e0;
        cipher.encrypt_block((&mut e1).into());

        let mut data = [0u8; 32];
        aes_ige_encrypt(&mut data, &key, &[0u8; 32]);
        assert_eq!(data[..16], e0);
        assert_eq!(data[16..], e1);
        aes_ige_decrypt(&mut data, &key, &[0u8; 32]);
        assert_eq!(data, [0u8; 32]);

        let mut text = *b"0123456789abcdef0123456789ABCDEF";
        let iv: [u8; 32] = random_bytes();
        aes_ige_encrypt(&mut text, &[7; 32], &iv);
        aes_ige_decrypt(&mut text, &[7; 32], &iv);
        assert_eq!(&text, b"0123456789abcdef0123456789ABCDEF");
    }

    #[test]
    fn message_roundtrip() {
        let key = AuthKey::from_bytes(random_bytes());
        let mut inner = vec![0u8; 32];
        inner[28..32].copy_from_slice(&8u32.to_le_bytes());
        inner.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let enc = key.encrypt_message(inner.clone(), Side::Server);
        assert_eq!(enc.len() % 16, 8);
        let dec = key.decrypt_message(&enc, Side::Server).unwrap();
        assert_eq!(&dec[..inner.len()], &inner[..]);
        // The other side's keys don't decrypt it.
        assert!(key.decrypt_message(&enc, Side::Client).is_err());
        let mut tampered = enc.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(
            key.decrypt_message(&tampered, Side::Server),
            Err(DecryptError::MsgKeyMismatch)
        );
    }
}
