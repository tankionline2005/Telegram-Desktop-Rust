//! Frame cost counters for performance work (`TDR_PROFILE=1`): time spent in named
//! parts, printed every two seconds to stderr. Off: one relaxed atomic load.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ON: AtomicBool = AtomicBool::new(false);
type Counters = HashMap<&'static str, (Duration, u32)>;

static DATA: Mutex<Option<(Instant, Counters)>> = Mutex::new(None);

pub fn init() {
    ON.store(std::env::var_os("TDR_PROFILE").is_some(), Ordering::Relaxed);
}

/// Times `f` under `name`.
pub fn time<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    if !ON.load(Ordering::Relaxed) {
        return f();
    }
    let start = Instant::now();
    let r = f();
    record(name, start.elapsed());
    r
}

fn record(name: &'static str, spent: Duration) {
    let mut guard = DATA.lock().unwrap();
    let (since, map) = guard.get_or_insert_with(|| (Instant::now(), HashMap::new()));
    let e = map.entry(name).or_default();
    e.0 += spent;
    e.1 += 1;
    if since.elapsed() >= Duration::from_secs(2) {
        let mut list: Vec<_> = map.iter().collect();
        list.sort_by_key(|(_, v)| std::cmp::Reverse(v.0));
        let line: Vec<String> = list
            .iter()
            .map(|(k, (d, n))| format!("{k}: {:.1} ms/s ({n})", d.as_secs_f64() * 1000. / 2.))
            .collect();
        eprintln!("profile: {}", line.join(", "));
        map.clear();
        *since = Instant::now();
    }
}

/// Times its own life under `name`.
pub struct Timer(&'static str, Option<Instant>);

impl Timer {
    pub fn new(name: &'static str) -> Self {
        Timer(name, ON.load(Ordering::Relaxed).then(Instant::now))
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            record(self.0, start.elapsed());
        }
    }
}

/// A part switched off for performance experiments (`TDR_OFF=tails,spoilers,...`).
pub fn off(part: &str) -> bool {
    static PARTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    PARTS
        .get_or_init(|| {
            std::env::var("TDR_OFF")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        })
        .iter()
        .any(|p| p == part)
}
