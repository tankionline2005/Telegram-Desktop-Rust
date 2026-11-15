//! Window position and size across restarts (tdesktop keeps them in its settings as
//! `Core::WindowPosition`). Stored as one line, "x y width height maximized", in
//! logical pixels.

use std::path::Path;

use gpui::{App, Bounds, Pixels, WindowBounds, point, px, size};

/// Smallest size we restore (the window's minimum size).
const MIN_WIDTH: f32 = 640.;
const MIN_HEIGHT: f32 = 420.;

pub fn encode(bounds: &WindowBounds) -> String {
    let (b, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    format!(
        "{} {} {} {} {}\n",
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
        u8::from(maximized)
    )
}

pub fn decode(text: &str) -> Option<WindowBounds> {
    let v: Vec<f32> = text
        .split_whitespace()
        .map(|s| s.parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h, maximized] = v[..] else {
        return None;
    };
    if !(x.is_finite() && y.is_finite() && w >= MIN_WIDTH && h >= MIN_HEIGHT) {
        return None;
    }
    let b = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    Some(if maximized != 0. {
        WindowBounds::Maximized(b)
    } else {
        WindowBounds::Windowed(b)
    })
}

/// The saved placement, if it is still on one of the connected displays.
pub fn load(path: &Path, cx: &App) -> Option<WindowBounds> {
    let saved = decode(&std::fs::read_to_string(path).ok()?)?;
    let b = match &saved {
        WindowBounds::Windowed(b) | WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => *b,
    };
    // At least a title-bar-sized piece must be visible on some display, or the window
    // would open off-screen (a monitor was unplugged since).
    let grip: Bounds<Pixels> = Bounds::new(b.origin, size(b.size.width.min(px(200.)), px(24.)));
    cx.displays()
        .iter()
        .any(|d| d.bounds().intersects(&grip))
        .then_some(saved)
}

pub fn save(path: &Path, bounds: &WindowBounds) {
    if let Err(e) = tdr_api::write_atomic(path, encode(bounds).as_bytes()) {
        tracing::warn!("window placement: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rejects_garbage() {
        let b = WindowBounds::Maximized(Bounds::new(
            point(px(-1910.), px(12.5)),
            size(px(1100.), px(720.)),
        ));
        assert_eq!(decode(&encode(&b)), Some(b));
        assert_eq!(decode(""), None);
        assert_eq!(decode("1 2 3"), None);
        assert_eq!(decode("0 0 10 10 0"), None); // below the minimum size
        assert_eq!(decode("0 0 NaN 800 0"), None);
    }
}
