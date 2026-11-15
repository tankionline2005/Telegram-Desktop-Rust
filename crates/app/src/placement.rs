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

// window.style: columnMinimalWidthLeft, columnMinimalWidthMain
pub const COLUMN_MIN_LEFT: f32 = 260.;
pub const COLUMN_MIN_MAIN: f32 = 380.;
/// The chat list squeezed to userpics (`dialogsSmallColumnWidth`: padding, photo,
/// padding of `defaultDialogRow`).
pub const COLUMN_SMALL: f32 = 10. + 46. + 10.;

/// The chat list's share of the window width, as tdesktop's `Core::Settings` keeps it:
/// one ratio while a chat is open and one with no chat; 0 is the small column
/// (userpics only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Columns {
    with_chat: f32,
    no_chat: f32,
    /// `_dialogsWidthSetToZeroWithoutChat`.
    zero_without_chat: bool,
}

impl Default for Columns {
    fn default() -> Self {
        // kDefaultDialogsWidthRatio
        Self {
            with_chat: 5. / 14.,
            no_chat: 5. / 14.,
            zero_without_chat: false,
        }
    }
}

impl Columns {
    /// `Settings::dialogsWidthRatio`.
    pub fn ratio(&self, no_chat: bool) -> f32 {
        if !no_chat || self.with_chat > 0. {
            self.with_chat
        } else {
            self.no_chat
        }
    }

    /// `Settings::updateDialogsWidthRatio`.
    pub fn update(&mut self, ratio: f32, no_chat: bool) {
        let change_with_chat = !no_chat || self.with_chat > 0. || self.zero_without_chat;
        let changed_with_chat = change_with_chat && self.with_chat != ratio;
        let change_no_chat = no_chat || self.with_chat != ratio;
        let changed_no_chat = change_no_chat && self.no_chat != ratio;
        if changed_with_chat {
            self.zero_without_chat = no_chat && ratio <= 0.;
            self.with_chat = ratio;
        }
        if changed_no_chat {
            self.no_chat = ratio;
        }
    }

    /// The chat list's width in a window `body` wide (`countDialogsWidthFromRatio` and
    /// `updateWindowAdaptiveLayout`).
    pub fn width(&self, no_chat: bool, body: f32) -> f32 {
        let ratio = self.ratio(no_chat);
        if ratio <= 0. {
            COLUMN_SMALL
        } else {
            (body * ratio)
                .round()
                .max(COLUMN_MIN_LEFT)
                .min(body - COLUMN_MIN_MAIN)
        }
    }

    /// The column's edge dragged to `width` (`moveLeftCallback`): narrower than half
    /// the minimum collapses it to userpics.
    pub fn drag_to(&mut self, width: f32, no_chat: bool, body: f32) {
        let ratio = if width < COLUMN_MIN_LEFT / 2. {
            0.
        } else {
            width / body
        };
        self.update(ratio, no_chat);
    }

    /// The drag ended (`moveFinishedCallback`): keep the width actually shown.
    pub fn drag_finished(&mut self, no_chat: bool, body: f32) {
        if self.ratio(no_chat) > 0. {
            let shown = self.width(no_chat, body);
            self.update(shown / body, no_chat);
        }
    }

    pub fn encode(&self) -> String {
        format!("{} {}\n", self.with_chat, self.no_chat)
    }

    /// As tdesktop reads its settings: the no-chat ratio follows a nonzero with-chat one.
    pub fn decode(text: &str) -> Option<Self> {
        let v: Vec<f32> = text
            .split_whitespace()
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        let [with_chat, no_chat] = v[..] else {
            return None;
        };
        let valid = |r: f32| r.is_finite().then(|| r.clamp(0., 1.));
        let with_chat = valid(with_chat)?;
        let no_chat = valid(no_chat)?;
        Some(Self {
            with_chat,
            no_chat: if with_chat > 0. { with_chat } else { no_chat },
            zero_without_chat: false,
        })
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| Self::decode(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Err(e) = tdr_api::write_atomic(path, self.encode().as_bytes()) {
            tracing::warn!("column widths: {e}");
        }
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

    #[test]
    fn column_width_limits() {
        let mut c = Columns::default();
        // 5/14 of 1400 = 500.
        assert_eq!(c.width(false, 1400.), 500.);
        // Never below the minimum, never squeezing the chat below its minimum.
        c.drag_to(200., false, 1400.);
        assert_eq!(c.width(false, 1400.), COLUMN_MIN_LEFT);
        c.drag_to(1300., false, 1400.);
        assert_eq!(c.width(false, 1400.), 1400. - COLUMN_MIN_MAIN);
        c.drag_finished(false, 1400.);
        assert_eq!(c.ratio(false), (1400. - COLUMN_MIN_MAIN) / 1400.);
        // Left of half the minimum: userpics only, kept after the drag.
        c.drag_to(129., false, 1400.);
        assert_eq!(c.width(false, 1400.), COLUMN_SMALL);
        c.drag_finished(false, 1400.);
        assert_eq!(c.width(false, 1400.), COLUMN_SMALL);
        // Back to the right: the minimum width again.
        c.drag_to(140., false, 1400.);
        assert_eq!(c.width(false, 1400.), COLUMN_MIN_LEFT);
    }

    #[test]
    fn columns_with_and_without_chat() {
        let mut c = Columns::default();
        // Collapsed with no chat open: the chat view keeps it collapsed too
        // (`_dialogsWidthSetToZeroWithoutChat`), and widening without a chat widens both.
        c.drag_to(100., true, 1000.);
        assert_eq!(c.width(true, 1000.), COLUMN_SMALL);
        assert_eq!(c.width(false, 1000.), COLUMN_SMALL);
        c.drag_to(400., true, 1000.);
        assert_eq!(c.width(true, 1000.), 400.);
        assert_eq!(c.width(false, 1000.), 400.);
        // Collapsed with a chat open: both views collapse, but widening it with no chat
        // open then leaves the chat view collapsed.
        c.drag_to(100., false, 1000.);
        assert_eq!(c.width(false, 1000.), COLUMN_SMALL);
        assert_eq!(c.width(true, 1000.), COLUMN_SMALL);
        c.drag_to(400., true, 1000.);
        assert_eq!(c.width(true, 1000.), 400.);
        assert_eq!(c.width(false, 1000.), COLUMN_SMALL);
    }

    #[test]
    fn columns_round_trip() {
        let mut c = Columns::default();
        c.drag_to(450., false, 1000.);
        assert_eq!(
            Columns::decode(&c.encode()).unwrap().width(false, 1000.),
            450.
        );
        assert_eq!(Columns::decode("x"), None);
        assert_eq!(Columns::decode("NaN 0.3"), None);
        assert_eq!(Columns::decode("0 0.4").unwrap().width(true, 1000.), 400.);
    }
}
