//! Temporary session storage for the console client: `data/session.bin` in the workspace
//! (git-ignored). The GUI will keep sessions in the encrypted local storage instead.
//!
//! Format: magic `TDRS`, version u32, dc_id i32, time_offset i32, auth key (256 bytes).

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use tdr_crypto::AuthKey;

const MAGIC: &[u8; 4] = b"TDRS";
const VERSION: u32 = 1;

pub struct SavedSession {
    pub dc_id: i32,
    pub auth_key: AuthKey,
    pub time_offset: i32,
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/session.bin")
}

pub fn load() -> Result<Option<SavedSession>> {
    let data = match fs::read(path()) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if data.len() != 4 + 4 + 4 + 4 + 256 || &data[..4] != MAGIC {
        bail!("session file is corrupted; delete data/session.bin and log in again");
    }
    let int = |at: usize| i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    if int(4) as u32 != VERSION {
        bail!("unsupported session file version");
    }
    Ok(Some(SavedSession {
        dc_id: int(8),
        time_offset: int(12),
        auth_key: AuthKey::from_bytes(data[16..].try_into().unwrap()),
    }))
}

pub fn save(s: &SavedSession) -> Result<()> {
    let mut data = Vec::with_capacity(272);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&VERSION.to_le_bytes());
    data.extend_from_slice(&s.dc_id.to_le_bytes());
    data.extend_from_slice(&s.time_offset.to_le_bytes());
    data.extend_from_slice(&s.auth_key.to_bytes());
    let p = path();
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(p, data)?;
    Ok(())
}

pub fn delete() -> Result<()> {
    match fs::remove_file(path()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
