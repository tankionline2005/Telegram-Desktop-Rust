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
    std::thread::Builder::new()
        .name("memory-report".into())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                eprintln!(
                    "memory: heap {} (peak {}); {}; {}",
                    mb(live()),
                    mb(PEAK.load(Ordering::Relaxed)),
                    crate::custom_emoji::stats(),
                    crate::emoji::stats(),
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
            drop(anim);
        }
    }
}
