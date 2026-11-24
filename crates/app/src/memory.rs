//! Memory accounting for optimization work: the heap's live bytes (a counting
//! global allocator, two relaxed atomics per call) and, with `TDR_MEMORY=1`, a
//! report every five seconds to stderr of the big holders (sizes and counts only).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout, passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            added(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: a block of ours with its layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: a block of ours with its layout.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            added(new_size);
        }
        p
    }
}

static BIG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn added(size: usize) {
    if size >= 4 << 20 && BIG.load(Ordering::Relaxed) {
        // `TDR_MEMORY=big`: where the big blocks come from.
        let _ = INSIDE.try_with(|inside| {
            if !inside.get() {
                inside.set(true);
                eprintln!(
                    "memory: big block {} MB at\n{}",
                    size >> 20,
                    std::backtrace::Backtrace::force_capture()
                );
                inside.set(false);
            }
        });
    }
    let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    if now > PEAK.load(Ordering::Relaxed) {
        PEAK.store(now, Ordering::Relaxed);
    }
}

/// The window's part of the report, set by the window now and then.
static UI_STATS: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The report is on (`TDR_MEMORY`).
pub fn on() -> bool {
    REPORT.load(Ordering::Relaxed)
}

static REPORT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_ui_stats(stats: String) {
    *UI_STATS.lock().unwrap() = stats;
}

/// Live heap bytes.
pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn mb(bytes: usize) -> String {
    format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
}

/// Starts the report thread with `TDR_MEMORY=1`.
pub fn init() {
    let Some(mode) = std::env::var_os("TDR_MEMORY") else {
        return;
    };
    BIG.store(mode == "big", Ordering::Relaxed);
    REPORT.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("memory-report".into())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                eprintln!(
                    "memory: heap {} (peak {}); {}; {}; {}; {}",
                    mb(live()),
                    mb(PEAK.load(Ordering::Relaxed)),
                    crate::custom_emoji::stats(),
                    crate::emoji::stats(),
                    crate::spoiler::stats(),
                    UI_STATS.lock().unwrap(),
                );
            }
        })
        .expect("spawn memory report");
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn lottie_sizes() {
        let dir = std::env::var("TGS_DIR").unwrap();
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let before = super::live();
            let anim = tdr_lottie::Animation::from_tgs(&bytes).unwrap();
            let after = super::live();
            eprintln!(
                "{:>8} KB parsed from {:>6} KB: {}",
                (after - before) / 1024,
                bytes.len() / 1024,
                path.file_name().unwrap().to_string_lossy()
            );
            for side in [35u32, 320] {
                let mut pixmap = tdr_lottie::tiny_skia::Pixmap::new(side, side).unwrap();
                let t = std::time::Instant::now();
                let n = anim.frame_count().min(180);
                for k in 0..n {
                    pixmap.fill(tdr_lottie::tiny_skia::Color::TRANSPARENT);
                    anim.render_into(k, &mut pixmap);
                }
                eprintln!("    {side} px: {n} frames in {:?}", t.elapsed());
            }
            drop(anim);
        }
    }

    /// `LOTTIE_REF=<dir>` with `LOTTIE_MODE=dump|compare`: renders the custom
    /// emoji in the file cache (read only) and the files of `TGS_DIR` at four
    /// frames, and keeps them or compares them with the kept ones (a renderer
    /// change must not change the pictures).
    #[test]
    #[ignore]
    fn lottie_reference() {
        let out = std::path::PathBuf::from(std::env::var("LOTTIE_REF").unwrap());
        let dump = std::env::var("LOTTIE_MODE").as_deref() == Ok("dump");
        std::fs::create_dir_all(&out).unwrap();
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        if let Ok(dir) = std::env::var("TGS_DIR") {
            for e in std::fs::read_dir(dir).unwrap() {
                let path = e.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                files.push((name, std::fs::read(&path).unwrap()));
            }
        }
        if let Ok(mut cache) =
            tdr_data::tdr_storage::FileCache::open_read_only(crate::data_dir().join("files"))
        {
            for key in cache.keys("emoji-file:", 400).unwrap_or_default() {
                if let Ok(Some(bytes)) = cache.get(&key)
                    && bytes.starts_with(&[0x1f, 0x8b])
                {
                    files.push((key.replace(':', "_"), bytes));
                }
            }
        }
        let (mut same, mut changed, mut worst) = (0, 0, 0u8);
        let mut rendering = std::time::Duration::ZERO;
        let mut parsed = 0usize;
        for (name, bytes) in &files {
            let before = super::live();
            let Ok(anim) = tdr_lottie::Animation::from_tgs(bytes) else {
                continue;
            };
            parsed += super::live() - before;
            let n = anim.frame_count();
            let mut pixels = Vec::new();
            let t = std::time::Instant::now();
            for k in [0, n / 3, 2 * n / 3, n.saturating_sub(1)] {
                pixels.extend_from_slice(anim.render(k, 100, 100).unwrap().data());
            }
            let mut small = tdr_lottie::tiny_skia::Pixmap::new(35, 35).unwrap();
            for k in 0..n.min(180) {
                small.fill(tdr_lottie::tiny_skia::Color::TRANSPARENT);
                anim.render_into(k, &mut small);
            }
            rendering += t.elapsed();
            let path = out.join(format!("{name}.raw"));
            if dump {
                std::fs::write(&path, &pixels).unwrap();
                continue;
            }
            let reference = std::fs::read(&path).unwrap();
            let max = reference
                .iter()
                .zip(&pixels)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            let off = reference
                .iter()
                .zip(&pixels)
                .filter(|(a, b)| a.abs_diff(**b) > 2)
                .count();
            if max <= 2 {
                same += 1;
            } else {
                changed += 1;
                eprintln!("{name}: max diff {max}, {off} channels over 2");
            }
            worst = worst.max(max);
        }
        eprintln!(
            "{} animations, parsed {} KB, rendering {rendering:?}: {same} same,              {changed} changed, worst {worst}",
            files.len(),
            parsed / 1024,
        );
    }

    /// Heap taken by syntax highlighting of code in many languages.
    #[test]
    #[ignore]
    fn prism_heap() {
        let before = super::live();
        let h =
            tdr_prism::Highlighter::load(include_bytes!("../../../assets/libprisma/grammars.dat"));
        eprintln!("grammars loaded: {} KB", (super::live() - before) / 1024);
        let code = "fn main() { let x = 10; println!(\"Hello, {}!\", x); } // comment
                    def add(a, b):
    return a + b
#include <stdio.h>
int main(void) { return 0; }";
        let languages = [
            "rust",
            "python",
            "c",
            "cpp",
            "csharp",
            "java",
            "javascript",
            "typescript",
            "go",
            "kotlin",
            "swift",
            "php",
            "ruby",
            "haskell",
            "elixir",
            "erlang",
            "lua",
            "sql",
            "bash",
            "objectivec",
            "scala",
            "dart",
            "json",
            "css",
            "markup",
        ];
        let mut at = super::live();
        for (k, lang) in languages.iter().enumerate() {
            let t = std::time::Instant::now();
            let _ = h.highlight(code, lang);
            let now = super::live();
            eprintln!(
                "{k:>2} {lang:<12} +{:>6} KB (total {:>6} KB), {:?}",
                (now as i64 - at as i64) / 1024,
                (now - before) / 1024,
                t.elapsed()
            );
            at = now;
        }
    }
}
