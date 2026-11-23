//! Repaints at a time instead of on every vsync: continuous animations that change
//! a few times a second (spoiler particles at 30 frames a second, custom emoji)
//! ask for the window to be painted again when their next frame is due. GPUI
//! paints the whole window each time, so a vsync-paced loop (144 a second on some
//! monitors) for a 30 fps animation would waste most of the work.

use std::sync::Mutex;
use std::time::{Duration, Instant};

static NEXT: Mutex<Option<Instant>> = Mutex::new(None);
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Animations stop while the window is not active (tdesktop's
/// `GifPauseReason`): they show the frame they had, and resume when it is.
pub fn set_paused(paused: bool) {
    PAUSED.store(paused, std::sync::atomic::Ordering::Relaxed);
}

pub fn paused() -> bool {
    PAUSED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Paint again at `at` (the earliest of the requests since the last paint).
pub fn at(at: Instant) {
    if paused() {
        return;
    }
    let mut next = NEXT.lock().unwrap();
    *next = Some(next.map_or(at, |n| n.min(at)));
}

/// Paint again in `ms` milliseconds.
pub fn after_ms(ms: u64) {
    at(Instant::now() + Duration::from_millis(ms));
}

/// A new paint starts: the requests are made again by what is painted.
pub fn begin() {
    *NEXT.lock().unwrap() = None;
}

/// When the window should be painted again (`None`: nothing moves).
pub fn due() -> Option<Instant> {
    *NEXT.lock().unwrap()
}
