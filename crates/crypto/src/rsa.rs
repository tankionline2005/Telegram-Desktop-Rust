//! Telegram server RSA keys and the `RSA_PAD` encryption used in the key exchange.

use num_bigint::BigUint;

use crate::{aes_ige_encrypt, fill_random, random_bytes, sha1, sha256};

/// Production server key, as built into tdesktop
/// (`Telegram/SourceFiles/mtproto/mtproto_dc_options.cpp`).
const PRODUCTION_KEYS: &[&str] = &["-----BEGIN RSA PUBLIC KEY-----
MIIBCgKCAQEA6LszBcC1LGzyr992NzE0ieY+BSaOW622Aa9Bd4ZHLl+TuFQ4lo4g
5nKaMBwK/BIb9xUfg0Q29/2mgIR6Zr9krM7HjuIcCzFvDtr+L0GQjae9H0pRB2OO
62cECs5HKhT5DZ98K33vmWiLowc621dQuwKWSQKjWf50XYFw42h21P2KXUGyp2y/
+aEyZ+uVgLLQbRA1dEjSDZ2iGRy12Mk5gpYc397aYp438fsJoHIgJ2lgMv5h7WY9
t6N/byY9Nw9p21Og3AoXSL2q/2IJ1WRUhebgAdGVMlV1fkuOQoEzR7EdpqtQD9Cs
5+bfo3Nhmcyvk5ftB0WkJ9z6bNZ7yxrP8wIDAQAB
-----END RSA PUBLIC KEY-----"];

/// Test-environment server key.
const TEST_KEYS: &[&str] = &["-----BEGIN RSA PUBLIC KEY-----
MIIBCgKCAQEAyMEdY1aR+sCR3ZSJrtztKTKqigvO/vBfqACJLZtS7QMgCGXJ6XIR
yy7mx66W0/sOFa7/1mAZtEoIokDP3ShoqF4fVNb6XeqgQfaUHd8wJpDWHcR2OFwv
plUUI1PLTktZ9uW2WE23b+ixNwJjJGwBDJPQEQFBE+vfmH0JP503wr5INS1poWg/
j25sIWeYPHYeOrFp/eXaqhISP6G+q2IeTaWTXpwZj4LzXq5YOpk4bYEQ6mvRq7D1
aHWfYmlEGepfaYR8Q0YqvvhYtMte3ITnuSJs171+GDqpdKcSwHnd6FudwGO4pcCO
j4WcDuXc2CTHgH8gFTNhp/Y8/SpDOhvn9QIDAQAB
-----END RSA PUBLIC KEY-----"];

#[derive(Debug, Clone)]
pub struct RsaKey {
    n: BigUint,
    e: BigUint,
    fingerprint: i64,
}

impl RsaKey {
    /// Parses a PKCS#1 `RSA PUBLIC KEY` PEM block.
    pub fn from_pem(pem: &str) -> Option<Self> {
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64_decode(&b64)?;
        // SEQUENCE { INTEGER n, INTEGER e }
        let mut p = Der(&der);
        let mut seq = Der(p.read(0x30)?);
        let n = seq.read(0x02)?;
        let e = seq.read(0x02)?;
        Some(Self::new(n, e))
    }

    pub fn new(n: &[u8], e: &[u8]) -> Self {
        let strip = |b: &[u8]| -> Vec<u8> {
            let i = b.iter().position(|&x| x != 0).unwrap_or(b.len());
            b[i..].to_vec()
        };
        let (n, e) = (strip(n), strip(e));
        // Fingerprint: lower 64 bits of SHA1 over TL-serialized `n` and `e`.
        let mut tl = Vec::new();
        tl_bytes(&mut tl, &n);
        tl_bytes(&mut tl, &e);
        let hash = sha1(&[&tl]);
        Self {
            n: BigUint::from_bytes_be(&n),
            e: BigUint::from_bytes_be(&e),
            fingerprint: i64::from_le_bytes(hash[12..20].try_into().unwrap()),
        }
    }

    pub fn fingerprint(&self) -> i64 {
        self.fingerprint
    }

    /// `RSA_PAD(data, server_public_key)` from the MTProto 2.0 key exchange.
    /// `data` is a serialized `p_q_inner_data_*`, at most 144 bytes.
    pub fn encrypt_pad(&self, data: &[u8]) -> Option<[u8; 256]> {
        if data.len() > 144 {
            return None;
        }
        let mut with_padding = [0u8; 192];
        with_padding[..data.len()].copy_from_slice(data);
        fill_random(&mut with_padding[data.len()..]);
        let mut reversed = with_padding;
        reversed.reverse();

        loop {
            let temp_key: [u8; 32] = random_bytes();
            let mut with_hash = [0u8; 224];
            with_hash[..192].copy_from_slice(&reversed);
            with_hash[192..].copy_from_slice(&sha256(&[&temp_key, &with_padding]));
            aes_ige_encrypt(&mut with_hash, &temp_key, &[0u8; 32]);

            let hash = sha256(&[&with_hash]);
            let mut key_aes = [0u8; 256];
            for i in 0..32 {
                key_aes[i] = temp_key[i] ^ hash[i];
            }
            key_aes[32..].copy_from_slice(&with_hash);

            let m = BigUint::from_bytes_be(&key_aes);
            if m >= self.n {
                continue; // Retry with another temp_key, as the spec says.
            }
            let c = m.modpow(&self.e, &self.n).to_bytes_be();
            let mut out = [0u8; 256];
            out[256 - c.len()..].copy_from_slice(&c);
            return Some(out);
        }
    }
}

/// Built-in server keys for the production or test environment.
pub fn builtin_keys(test: bool) -> Vec<RsaKey> {
    let pems = if test { TEST_KEYS } else { PRODUCTION_KEYS };
    pems.iter()
        .map(|p| RsaKey::from_pem(p).expect("built-in RSA key is valid"))
        .collect()
}

fn tl_bytes(out: &mut Vec<u8>, data: &[u8]) {
    let header = if data.len() < 254 {
        out.push(data.len() as u8);
        1
    } else {
        let l = data.len();
        out.extend_from_slice(&[254, l as u8, (l >> 8) as u8, (l >> 16) as u8]);
        4
    };
    out.extend_from_slice(data);
    out.resize(out.len() + (4 - (header + data.len()) % 4) % 4, 0);
}

struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    fn read(&mut self, tag: u8) -> Option<&'a [u8]> {
        let d = self.0;
        if *d.first()? != tag {
            return None;
        }
        let (len, hdr) = match *d.get(1)? {
            l if l < 0x80 => (l as usize, 2),
            0x81 => (*d.get(2)? as usize, 3),
            0x82 => (((*d.get(2)? as usize) << 8) | *d.get(3)? as usize, 4),
            _ => return None,
        };
        let body = d.get(hdr..hdr + len)?;
        self.0 = &d[hdr + len..];
        Some(body)
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_key_fingerprints() {
        // Fingerprints Telegram servers advertise in resPQ for these keys.
        let prod = builtin_keys(false);
        assert_eq!(prod[0].fingerprint() as u64, 0xd09d1d85de64fd85);
        assert_eq!(prod[0].n.bits(), 2048);
        let test = builtin_keys(true);
        assert_eq!(test[0].fingerprint() as u64, 0xb25898df208d2603);
    }

    #[test]
    fn rsa_pad_output_is_below_modulus() {
        let key = &builtin_keys(false)[0];
        let out = key.encrypt_pad(&[1u8; 100]).unwrap();
        assert!(BigUint::from_bytes_be(&out) < key.n);
        assert!(key.encrypt_pad(&[0u8; 145]).is_none());
    }
}
