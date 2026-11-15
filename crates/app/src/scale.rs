//! tdesktop's interface scale. Its styles are in 100 % pixels; at another scale every
//! size is converted to whole device pixels (`style::ConvertScale`), fonts included,
//! and a font's line height is Qt's: rounded ascent plus rounded descent. Laying out
//! with the same integers gives the same rows, bubbles and lines as the original
//! (at 125 % a chat row is 77 px, not 77.5).

use std::sync::atomic::{AtomicU32, Ordering};

use gpui::Pixels;

/// The window's scale factor (1.25 at 125 %), set every frame.
static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

pub fn set(scale_factor: f32) {
    if scale_factor.is_finite() && scale_factor > 0. {
        SCALE.store(scale_factor.to_bits(), Ordering::Relaxed);
    }
}

pub fn get() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// `style::ConvertScale`: a style size in device pixels.
pub fn device(value: f32) -> f32 {
    device_at(value, get())
}

fn device_at(value: f32, scale: f32) -> f32 {
    if value < 0. {
        return -device_at(-value, scale);
    }
    let result = (value * scale - 0.01).round();
    if value > 0. && result == 0. {
        1.
    } else {
        result
    }
}

/// A style size in logical pixels, on the device pixel grid.
pub fn lg(value: f32) -> f32 {
    device(value) / get()
}

/// [`lg`] as GPUI pixels. `ui` uses this in place of `gpui::px`.
pub fn px(value: f32) -> Pixels {
    gpui::px(lg(value))
}

/// Exactly `device` device pixels.
pub fn dpx(device: f32) -> Pixels {
    gpui::px(device / get())
}

/// Open Sans (the bundled fonts are tdesktop's): hhea ascender and descender per em.
const ASCENT: f32 = 2189. / 2048.;
const DESCENT: f32 = 600. / 2048.;

/// A tdesktop font (`style::font`) at the current scale, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: Pixels,
    pub ascent: f32,
    pub descent: f32,
    /// Qt's `QFontMetrics::height`: the line height of the font.
    pub height: f32,
}

impl Font {
    pub fn line_height(&self) -> Pixels {
        gpui::px(self.height)
    }
}

/// `font(size px)`.
pub fn font(size: f32) -> Font {
    font_at(size, get())
}

fn font_at(size: f32, scale: f32) -> Font {
    let device = device_at(size, scale);
    let ascent = (device * ASCENT).round();
    let descent = (device * DESCENT).round();
    Font {
        size: gpui::px(device / scale),
        ascent: ascent / scale,
        descent: descent / scale,
        height: (ascent + descent) / scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_scale_like_tdesktop() {
        // 62 * 1.25 = 77.5: tdesktop subtracts 0.01 before rounding.
        assert_eq!(device_at(62., 1.25), 77.);
        assert_eq!(device_at(46., 1.25), 57.);
        assert_eq!(device_at(8., 1.25), 10.);
        assert_eq!(device_at(11., 1.25), 14.);
        assert_eq!(device_at(-17., 1.25), -21.);
        assert_eq!(device_at(1., 1.0), 1.);
        // Never rounds a nonzero size away.
        assert_eq!(device_at(0.3, 1.0), 1.);
    }

    #[test]
    fn font_heights_match_the_original() {
        // Measured in tdesktop at 125 %: 22 px between message lines.
        let f = font_at(13., 1.25);
        assert_eq!(f.height * 1.25, 22.);
        assert_eq!(f.size, gpui::px(16. / 1.25));
        assert_eq!(font_at(12., 1.25).height * 1.25, 20.);
        assert_eq!(font_at(13., 1.).height, 18.);
    }
}
