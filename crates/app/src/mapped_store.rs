//! A disk cache of pixel blobs read through memory-mapped files: rendered
//! custom emoji frames (`custom_emoji`) and emoji images cut from the sprites
//! (`emoji`).
//!
//! Mapped file pages are not the process's private memory: Windows keeps them
//! in its file cache while there is room (reading them back costs a memory
//! access) and drops them first when memory is short, so what the app has shown
//! once costs it no RAM afterwards, survives restarts (nothing is rendered or
//! decoded twice) and still comes back at once. tdesktop keeps such caches on
//! disk too, but reads them back into its own memory.
//!
//! Layout: `<name>.bin` holds the blobs in segments of `SEGMENT` bytes (each
//! mapped once, never moved; a blob never crosses one), `<name>.idx` an
//! append-only list of records (key, place, length, checksum) after a header
//! with the format's version. A blob is written before its record, so a crash
//! leaves at most an unlisted blob, and every blob is checked against its sum
//! the first time it is read in a session.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use memmap2::MmapMut;

/// Bytes per mapped segment.
const SEGMENT: usize = 32 << 20;
const MAGIC: &[u8; 8] = b"TDRSTOR1";
const RECORD: usize = 32;

pub type Key = (i64, u32);

#[derive(Clone, Copy)]
struct Place {
    segment: u32,
    offset: u32,
    len: u32,
    sum: u32,
    checked: bool,
}

struct Segment {
    /// Kept alive (never unmapped while the store lives: it lives in a static).
    map: MmapMut,
}

pub struct Store {
    data: File,
    index: File,
    segments: Vec<Segment>,
    places: HashMap<Key, Place>,
    /// Where the next blob goes.
    end: (u32, u32),
    max_segments: u32,
    full: bool,
}

// SAFETY: the raw mappings are only reached through `&self`/`&mut self` of a
// store behind a mutex; blobs handed out are never written again.
unsafe impl Send for Store {}

/// A checksum of a blob (64-bit words, FNV-like).
fn checksum(bytes: &[u8]) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &b in rest {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h ^ (h >> 32)) as u32
}

impl Store {
    /// Opens (or starts anew) the store `name` in `dir`. A store made by another
    /// `version` or grown past `max_segments` starts empty.
    pub fn open(dir: &Path, name: &str, version: u64, max_segments: u32) -> Option<Store> {
        std::fs::create_dir_all(dir).ok()?;
        let open = |ext: &str| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(format!("{name}.{ext}")))
                .ok()
        };
        let data = open("bin")?;
        let mut index = open("idx")?;
        let mut header = [0u8; 16];
        let mut want = [0u8; 16];
        want[..8].copy_from_slice(MAGIC);
        want[8..].copy_from_slice(&version.to_le_bytes());
        let mut records = Vec::new();
        let valid = index.read_exact(&mut header).is_ok() && header == want && {
            index.read_to_end(&mut records).is_ok()
        };
        let data_len = data.metadata().ok()?.len() as usize;
        let segments_on_disk = data_len / SEGMENT;
        if !valid || segments_on_disk >= max_segments as usize {
            // Start anew.
            index.set_len(0).ok()?;
            data.set_len(0).ok()?;
            let mut index_w = &index;
            index_w.write_all(&want).ok()?;
            records.clear();
        }
        let mut store = Store {
            data,
            index,
            segments: Vec::new(),
            places: HashMap::new(),
            end: (0, 0),
            max_segments,
            full: false,
        };
        let segments_now = store.data.metadata().ok()?.len() as usize / SEGMENT;
        for k in 0..segments_now {
            store.map_segment(k)?;
        }
        for r in records.as_chunks::<RECORD>().0 {
            let u32_at = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
            let key = (i64::from_le_bytes(r[0..8].try_into().unwrap()), u32_at(8));
            let place = Place {
                segment: u32_at(12),
                offset: u32_at(16),
                len: u32_at(20),
                sum: u32_at(24),
                checked: false,
            };
            if (place.segment as usize) < segments_now
                && place.offset as usize + place.len as usize <= SEGMENT
            {
                store.places.insert(key, place);
                store.end = (place.segment, place.offset + place.len);
            }
        }
        Some(store)
    }

    fn map_segment(&mut self, k: usize) -> Option<()> {
        // SAFETY: the file is ours (one app per data folder) and is only grown
        // while mapped, never shrunk.
        let map = unsafe {
            memmap2::MmapOptions::new()
                .offset((k * SEGMENT) as u64)
                .len(SEGMENT)
                .map_mut(&self.data)
                .ok()?
        };
        self.segments.push(Segment { map });
        Some(())
    }

    fn slice(&self, place: &Place) -> &'static [u8] {
        let map = &self.segments[place.segment as usize].map;
        // SAFETY: inside the segment (checked on open and put); the mapping
        // lives as long as the store, which lives in a static for the rest of
        // the process, and a written blob is never written again.
        unsafe {
            std::slice::from_raw_parts(map.as_ptr().add(place.offset as usize), place.len as usize)
        }
    }

    /// The blob of `key`, if stored (and intact).
    pub fn get(&mut self, key: Key) -> Option<&'static [u8]> {
        let place = *self.places.get(&key)?;
        let blob = self.slice(&place);
        if !place.checked {
            if checksum(blob) != place.sum {
                self.places.remove(&key);
                return None;
            }
            if let Some(p) = self.places.get_mut(&key) {
                p.checked = true;
            }
        }
        Some(blob)
    }

    pub fn contains(&self, key: Key) -> bool {
        self.places.contains_key(&key)
    }

    /// Stores `parts` (one after another) as the blob of `key`; the stored blob,
    /// or `None` when it does not fit (too big, or the store is full).
    pub fn put(&mut self, key: Key, parts: &[&[u8]]) -> Option<&'static [u8]> {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        if self.full || len == 0 || len > SEGMENT {
            return None;
        }
        let (mut segment, mut offset) = self.end;
        if offset as usize + len > SEGMENT {
            segment += 1;
            offset = 0;
        }
        if segment >= self.max_segments {
            self.full = true;
            return None;
        }
        while self.segments.len() <= segment as usize {
            let k = self.segments.len();
            self.data.set_len(((k + 1) * SEGMENT) as u64).ok()?;
            self.map_segment(k)?;
        }
        let map = &mut self.segments[segment as usize].map;
        let mut at = offset as usize;
        for part in parts {
            map[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        let place = Place {
            segment,
            offset,
            len: len as u32,
            sum: checksum(&map[offset as usize..at]),
            checked: true,
        };
        let mut record = [0u8; RECORD];
        record[0..8].copy_from_slice(&key.0.to_le_bytes());
        record[8..12].copy_from_slice(&key.1.to_le_bytes());
        record[12..16].copy_from_slice(&place.segment.to_le_bytes());
        record[16..20].copy_from_slice(&place.offset.to_le_bytes());
        record[20..24].copy_from_slice(&place.len.to_le_bytes());
        record[24..28].copy_from_slice(&place.sum.to_le_bytes());
        let mut index = &self.index;
        let _ = index.seek_end_and_write(&record);
        self.end = (segment, offset + len as u32);
        self.places.insert(key, place);
        Some(self.slice(&place))
    }
}

trait AppendRecord {
    fn seek_end_and_write(&mut self, bytes: &[u8]) -> std::io::Result<()>;
}

impl AppendRecord for &File {
    fn seek_end_and_write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Seek;
        self.seek(std::io::SeekFrom::End(0))?;
        self.write_all(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_blobs_across_opens_and_checks_them() {
        let dir = std::env::temp_dir().join(format!("tdr-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut s = Store::open(&dir, "t", 1, 4).unwrap();
            assert_eq!(s.put((1, 2), &[b"ab", b"cd"]).unwrap(), b"abcd");
            assert!(s.put((3, 4), &[&vec![7u8; 1000]]).is_some());
            assert_eq!(s.get((1, 2)).unwrap(), b"abcd");
        }
        {
            let mut s = Store::open(&dir, "t", 1, 4).unwrap();
            assert_eq!(s.get((1, 2)).unwrap(), b"abcd");
            assert_eq!(s.get((3, 4)).unwrap().len(), 1000);
            assert!(s.get((5, 6)).is_none());
        }
        // Another version starts anew.
        {
            let mut s = Store::open(&dir, "t", 2, 4).unwrap();
            assert!(s.get((1, 2)).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
