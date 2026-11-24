//! Downloaded files (photos, thumbnails, documents) with size and age limits.
//!
//! Defaults match tdesktop (`lib_storage`, `Storage::Cache::Database::Settings`): 1 GiB
//! and 31 days since last use. Least recently used files are evicted first.
//!
//! Small files (thumbnails, avatars, stickers) are kept inside SQLite: reading many small
//! blobs from one database is faster than opening as many separate files
//! (https://sqlite.org/fasterthanfs.html). Large files are separate files named by row id.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

#[derive(Debug, Clone)]
pub struct FileCacheConfig {
    /// Total size limit, bytes.
    pub size_limit: u64,
    /// Files not used for this long are removed.
    pub time_limit: Duration,
    /// Files up to this size are stored inside the database.
    pub inline_max: usize,
}

impl Default for FileCacheConfig {
    fn default() -> Self {
        Self {
            size_limit: 1024 * 1024 * 1024,
            time_limit: Duration::from_secs(31 * 24 * 60 * 60),
            inline_max: 128 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileCacheStats {
    pub files: u64,
    pub bytes: u64,
}

pub struct FileCache {
    dir: PathBuf,
    conn: Connection,
    config: FileCacheConfig,
    total: u64,
    /// Opened for reading only: nothing is pruned or marked used.
    read_only: bool,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

impl FileCache {
    /// Opens the cache in `dir` (created if missing) and removes expired files.
    pub fn open(dir: impl AsRef<Path>, config: FileCacheConfig) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir).map_err(io)?;
        let conn = Connection::open(dir.join("index.sqlite"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS files (
                 key         TEXT PRIMARY KEY,
                 size        INTEGER NOT NULL,
                 last_access INTEGER NOT NULL,
                 data        BLOB
             );
             CREATE INDEX IF NOT EXISTS files_lru ON files (last_access);",
        )?;
        let total: i64 =
            conn.query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;
        let mut cache = Self {
            dir,
            conn,
            config,
            total: total as u64,
            read_only: false,
        };
        cache.prune(now())?;
        Ok(cache)
    }

    /// Opens an existing cache for reading only (checks and the demo): nothing
    /// is changed, a running app may keep using it.
    pub fn open_read_only(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let conn = Connection::open_with_flags(
            dir.join("index.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        Ok(Self {
            dir,
            conn,
            config: FileCacheConfig::default(),
            total: 0,
            read_only: true,
        })
    }

    /// The keys starting with `prefix`, at most `limit`.
    pub fn keys(&self, prefix: &str, limit: usize) -> Result<Vec<String>> {
        self.conn
            .prepare("SELECT key FROM files WHERE key LIKE ?1 || '%' ORDER BY rowid LIMIT ?2")?
            .query_map(params![prefix, limit as i64], |r| r.get(0))?
            .collect()
    }

    fn path(&self, rowid: i64) -> PathBuf {
        self.dir
            .join(format!("{:02x}", rowid & 0xff))
            .join(format!("{rowid:016x}"))
    }

    pub fn stats(&self) -> Result<FileCacheStats> {
        let files: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        Ok(FileCacheStats {
            files: files as u64,
            bytes: self.total,
        })
    }

    /// Returns the file and marks it as used.
    pub fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>> {
        let row: Option<(i64, Option<Vec<u8>>)> = self
            .conn
            .prepare_cached("SELECT rowid, data FROM files WHERE key = ?1")?
            .query_row([key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((rowid, inline)) = row else {
            return Ok(None);
        };
        let data = match inline {
            Some(d) => d,
            None => match fs::read(self.path(rowid)) {
                Ok(d) => d,
                Err(_) => {
                    // The file disappeared (deleted by the user?): forget it.
                    self.remove(key)?;
                    return Ok(None);
                }
            },
        };
        if !self.read_only {
            self.conn
                .prepare_cached("UPDATE files SET last_access = ?2 WHERE rowid = ?1")?
                .execute(params![rowid, now()])?;
        }
        Ok(Some(data))
    }

    /// Stores `data` under `key`, replacing an older version, then enforces the limits.
    pub fn put(&mut self, key: &str, data: &[u8]) -> Result<()> {
        self.remove(key)?;
        let inline = data.len() <= self.config.inline_max;
        self.conn
            .prepare_cached(
                "INSERT INTO files (key, size, last_access, data) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                key,
                data.len() as i64,
                now(),
                inline.then_some(data)
            ])?;
        if !inline {
            let path = self.path(self.conn.last_insert_rowid());
            let written = (|| {
                fs::create_dir_all(path.parent().unwrap())?;
                // Write-then-rename: a crash never leaves a truncated file under the name.
                let tmp = path.with_extension("tmp");
                fs::write(&tmp, data)?;
                fs::rename(&tmp, &path)
            })();
            if let Err(e) = written {
                self.conn
                    .execute("DELETE FROM files WHERE key = ?1", [key])?;
                return Err(io(e));
            }
        }
        self.total += data.len() as u64;
        self.evict_to(self.config.size_limit)
    }

    pub fn remove(&mut self, key: &str) -> Result<()> {
        let row: Option<(i64, i64, bool)> = self
            .conn
            .prepare_cached("SELECT rowid, size, data IS NULL FROM files WHERE key = ?1")?
            .query_row([key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((rowid, size, on_disk)) = row {
            self.delete_row(rowid, size, on_disk)?;
        }
        Ok(())
    }

    fn delete_row(&mut self, rowid: i64, size: i64, on_disk: bool) -> Result<()> {
        if on_disk {
            let _ = fs::remove_file(self.path(rowid));
        }
        self.conn
            .prepare_cached("DELETE FROM files WHERE rowid = ?1")?
            .execute([rowid])?;
        self.total = self.total.saturating_sub(size as u64);
        Ok(())
    }

    /// Removes the least recently used files until the total fits in `limit`.
    fn evict_to(&mut self, limit: u64) -> Result<()> {
        while self.total > limit {
            let oldest: Option<(i64, i64, bool)> = self
                .conn
                .prepare_cached("SELECT rowid, size, data IS NULL FROM files ORDER BY last_access, rowid LIMIT 1")?
                .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let Some((rowid, size, on_disk)) = oldest else {
                break;
            };
            self.delete_row(rowid, size, on_disk)?;
        }
        Ok(())
    }

    /// Removes files unused for longer than the time limit.
    pub fn prune(&mut self, now: i64) -> Result<()> {
        let cutoff = now - self.config.time_limit.as_secs() as i64;
        let expired: Vec<(i64, i64, bool)> = self
            .conn
            .prepare_cached("SELECT rowid, size, data IS NULL FROM files WHERE last_access < ?1")?
            .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_>>()?;
        for (rowid, size, on_disk) in expired {
            self.delete_row(rowid, size, on_disk)?;
        }
        Ok(())
    }

    /// Removes everything (the "Clear cache" button).
    pub fn clear(&mut self) -> Result<()> {
        self.evict_to(0)
    }
}

fn io(e: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tdr-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn config(limit: u64) -> FileCacheConfig {
        FileCacheConfig {
            size_limit: limit,
            inline_max: 100,
            ..Default::default()
        }
    }

    #[test]
    fn small_inline_big_on_disk() {
        let dir = temp_dir("inline");
        let mut c = FileCache::open(&dir, config(10_000)).unwrap();
        c.put("small", &[1; 50]).unwrap();
        c.put("big", &[2; 500]).unwrap();
        assert_eq!(c.get("small").unwrap().unwrap(), vec![1; 50]);
        assert_eq!(c.get("big").unwrap().unwrap(), vec![2; 500]);
        assert_eq!(
            c.stats().unwrap(),
            FileCacheStats {
                files: 2,
                bytes: 550
            }
        );
        // Only the big one is a separate file (plus the index database).
        let on_disk = walk(&dir)
            .into_iter()
            .filter(|p| !p.to_string_lossy().contains("index.sqlite"))
            .count();
        assert_eq!(on_disk, 1);
        // Replacing keeps the total right.
        c.put("big", &[3; 300]).unwrap();
        assert_eq!(c.stats().unwrap().bytes, 350);
        drop(c);
        // Survives reopening.
        let mut c = FileCache::open(&dir, config(10_000)).unwrap();
        assert_eq!(c.get("big").unwrap().unwrap(), vec![3; 300]);
        assert_eq!(c.stats().unwrap().bytes, 350);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evicts_least_recently_used() {
        let dir = temp_dir("lru");
        let mut c = FileCache::open(&dir, config(1000)).unwrap();
        for key in ["a", "b", "c"] {
            c.put(key, &[0; 300]).unwrap();
        }
        // Make "a" the most recently used.
        c.conn
            .execute(
                "UPDATE files SET last_access = last_access + 10 WHERE key = 'a'",
                [],
            )
            .unwrap();
        c.put("d", &[0; 300]).unwrap(); // 1200 > 1000: the oldest goes ("b").
        assert!(c.get("b").unwrap().is_none());
        assert!(c.get("a").unwrap().is_some());
        assert!(c.stats().unwrap().bytes <= 1000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn expires_unused_files() {
        let dir = temp_dir("expire");
        let mut c = FileCache::open(&dir, config(10_000)).unwrap();
        c.put("old", &[0; 500]).unwrap();
        c.put("new", &[0; 10]).unwrap();
        c.prune(now() + 32 * 24 * 60 * 60).unwrap();
        assert_eq!(c.stats().unwrap().files, 0);
        assert!(
            walk(&dir)
                .iter()
                .all(|p| p.to_string_lossy().contains("index.sqlite"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
