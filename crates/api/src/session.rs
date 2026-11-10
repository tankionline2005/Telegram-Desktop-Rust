//! Persistent session data: which DC is home, keys per DC, clock offset.
//!
//! The binary format is versioned; v1 (the console client's first format, one DC and one
//! permanent key) is still readable. Encryption of this data at rest is the job of the
//! storage layer.

use std::collections::BTreeMap;

use tdr_crypto::AuthKey;

const MAGIC: &[u8; 4] = b"TDRS";
const VERSION: u32 = 3;

#[derive(Clone, Default)]
pub struct DcKeys {
    /// Permanent key: the account's authorization on this DC.
    pub perm: Option<AuthKey>,
    /// Temporary (PFS) key bound to `perm`.
    pub temp: Option<TempKey>,
    /// Whether `perm` is logged in (home DC after login, or authorization imported).
    pub authorized: bool,
}

#[derive(Clone)]
pub struct TempKey {
    pub key: AuthKey,
    /// Expiry, server unix time.
    pub expires_at: i32,
    /// Last known server salt for this key (0 if unknown).
    pub salt: i64,
}

#[derive(Clone, Default)]
pub struct SessionData {
    pub home_dc: i32,
    /// `server_time - local_time`, seconds.
    pub time_offset: i32,
    pub dcs: BTreeMap<i32, DcKeys>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError(pub &'static str);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bad session data: {}", self.0)
    }
}

impl std::error::Error for SessionError {}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], SessionError> {
        if self.0.len() < n {
            return Err(SessionError("truncated"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn i32(&mut self) -> Result<i32, SessionError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn key(&mut self) -> Result<AuthKey, SessionError> {
        Ok(AuthKey::from_bytes(self.take(256)?.try_into().unwrap()))
    }
}

impl SessionData {
    pub fn new(home_dc: i32) -> Self {
        Self {
            home_dc,
            ..Default::default()
        }
    }

    pub fn is_authorized(&self) -> bool {
        self.dcs.get(&self.home_dc).is_some_and(|d| d.authorized)
    }

    /// Serializes the session. Temporary keys are included only with `with_temp_keys`.
    pub fn to_bytes(&self, with_temp_keys: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.home_dc.to_le_bytes());
        out.extend_from_slice(&self.time_offset.to_le_bytes());
        out.extend_from_slice(&(self.dcs.len() as u32).to_le_bytes());
        for (dc, keys) in &self.dcs {
            let temp = keys.temp.as_ref().filter(|_| with_temp_keys);
            let flags = keys.perm.is_some() as u8
                | (temp.is_some() as u8) << 1
                | (keys.authorized as u8) << 2;
            out.extend_from_slice(&dc.to_le_bytes());
            out.push(flags);
            if let Some(k) = &keys.perm {
                out.extend_from_slice(&k.to_bytes());
            }
            if let Some(t) = temp {
                out.extend_from_slice(&t.key.to_bytes());
                out.extend_from_slice(&t.expires_at.to_le_bytes());
                out.extend_from_slice(&t.salt.to_le_bytes());
            }
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, SessionError> {
        let mut c = Cursor(data);
        if c.take(4)? != MAGIC {
            return Err(SessionError("wrong magic"));
        }
        match c.i32()? {
            1 => {
                // v1: dc_id, time_offset, permanent key of an authorized session.
                let home_dc = c.i32()?;
                let time_offset = c.i32()?;
                let perm = c.key()?;
                let mut s = Self {
                    home_dc,
                    time_offset,
                    ..Default::default()
                };
                s.dcs.insert(
                    home_dc,
                    DcKeys {
                        perm: Some(perm),
                        temp: None,
                        authorized: true,
                    },
                );
                Ok(s)
            }
            version @ (2 | 3) => {
                let home_dc = c.i32()?;
                let time_offset = c.i32()?;
                let count = c.i32()?;
                let mut dcs = BTreeMap::new();
                for _ in 0..count {
                    let dc = c.i32()?;
                    let flags = c.take(1)?[0];
                    let perm = if flags & 1 != 0 { Some(c.key()?) } else { None };
                    let temp = if flags & 2 != 0 {
                        let key = c.key()?;
                        let expires_at = c.i32()?;
                        // v2 did not store the salt.
                        let salt = if version >= 3 {
                            i64::from_le_bytes(c.take(8)?.try_into().unwrap())
                        } else {
                            0
                        };
                        Some(TempKey {
                            key,
                            expires_at,
                            salt,
                        })
                    } else {
                        None
                    };
                    dcs.insert(
                        dc,
                        DcKeys {
                            perm,
                            temp,
                            authorized: flags & 4 != 0,
                        },
                    );
                }
                Ok(Self {
                    home_dc,
                    time_offset,
                    dcs,
                })
            }
            _ => Err(SessionError("unsupported version")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AuthKey {
        AuthKey::from_bytes(tdr_crypto::random_bytes())
    }

    #[test]
    fn roundtrip_with_and_without_temp_keys() {
        let mut s = SessionData::new(2);
        s.time_offset = -7;
        s.dcs.insert(
            2,
            DcKeys {
                perm: Some(key()),
                temp: Some(TempKey {
                    key: key(),
                    expires_at: 123,
                    salt: -5,
                }),
                authorized: true,
            },
        );
        s.dcs.insert(
            4,
            DcKeys {
                perm: Some(key()),
                temp: None,
                authorized: false,
            },
        );

        let back = SessionData::from_bytes(&s.to_bytes(true)).unwrap();
        assert_eq!(back.home_dc, 2);
        assert_eq!(back.time_offset, -7);
        assert!(back.is_authorized());
        assert_eq!(back.dcs[&2].temp.as_ref().unwrap().expires_at, 123);
        assert_eq!(back.dcs[&2].temp.as_ref().unwrap().salt, -5);
        assert_eq!(
            back.dcs[&2].perm.as_ref().unwrap().id(),
            s.dcs[&2].perm.as_ref().unwrap().id()
        );
        assert!(!back.dcs[&4].authorized);

        let without = SessionData::from_bytes(&s.to_bytes(false)).unwrap();
        assert!(without.dcs[&2].temp.is_none());
    }

    #[test]
    fn reads_v1() {
        let k = key();
        let mut v1 = b"TDRS".to_vec();
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&4i32.to_le_bytes());
        v1.extend_from_slice(&3i32.to_le_bytes());
        v1.extend_from_slice(&k.to_bytes());
        let s = SessionData::from_bytes(&v1).unwrap();
        assert_eq!(s.home_dc, 4);
        assert!(s.is_authorized());
        assert_eq!(s.dcs[&4].perm.as_ref().unwrap().id(), k.id());
    }

    #[test]
    fn rejects_garbage() {
        assert!(SessionData::from_bytes(b"nope").is_err());
        assert!(SessionData::from_bytes(b"TDRS\x02\x00\x00\x00").is_err());
    }
}
