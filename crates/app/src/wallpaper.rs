//! Chat background. The night theme has a solid one; the day theme uses tdesktop's
//! default wallpaper: a four-color "freeform" gradient (ported from lib_ui's
//! `GenerateSmallComplexGradient`) with a pattern on top (the pattern is not drawn yet).

use std::sync::{Arc, LazyLock};

use gpui::{Img, ObjectFit, RenderImage, img, prelude::*};
use tdr_theme::Color;

/// lib_ui renders the gradient at 64x64 and scales it smoothly to the chat size.
const SIZE: u32 = 64;

static DEFAULT_GRADIENT: LazyLock<Arc<RenderImage>> =
    LazyLock::new(|| Arc::new(gradient_image(&tdr_theme::DEFAULT_WALLPAPER_COLORS)));

/// The default wallpaper's gradient, stretched over its parent.
pub fn default_gradient() -> Img {
    img(DEFAULT_GRADIENT.clone())
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .object_fit(ObjectFit::Fill)
}

fn gradient_image(colors: &[Color]) -> RenderImage {
    let rgb = gradient_rgb(colors, 0, 1.);
    let mut buffer = image::RgbaImage::new(SIZE, SIZE);
    for (px, [r, g, b]) in buffer.pixels_mut().zip(rgb) {
        px.0 = [b, g, r, 255]; // GPUI wants BGRA
    }
    RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])
}

/// `GenerateSmallComplexGradient`: four color points moved along a swirl; every pixel
/// is the inverse-distance (4th power) weighted mix of the colors. Row-major RGB.
pub fn gradient_rgb(colors: &[Color], rotation: i32, progress: f32) -> Vec<[u8; 3]> {
    const POSITIONS: [(f32, f32); 8] = [
        (0.80, 0.10),
        (0.60, 0.20),
        (0.35, 0.25),
        (0.25, 0.60),
        (0.20, 0.90),
        (0.40, 0.80),
        (0.65, 0.75),
        (0.75, 0.40),
    ];
    let for_phase = |phase: usize| -> [(f32, f32); 4] {
        std::array::from_fn(|i| {
            let (x, y) = POSITIONS[(phase + i * 2) % 8];
            (x, 1. - y)
        })
    };
    let phase = (rotation.clamp(0, 315) / 45) as usize;
    let previous = for_phase((phase + 1) % 8);
    let current = for_phase(phase);
    let points: Vec<(f32, f32)> = (0..colors.len().min(4))
        .map(|i| {
            (
                previous[i].0 + (current[i].0 - previous[i].0) * progress,
                previous[i].1 + (current[i].1 - previous[i].1) * progress,
            )
        })
        .collect();
    let rgb: Vec<[f32; 3]> = colors
        .iter()
        .map(|c| {
            let v = c.rgba();
            [
                (v >> 24) as u8 as f32,
                (v >> 16) as u8 as f32,
                (v >> 8) as u8 as f32,
            ]
        })
        .collect();

    let mut out = Vec::with_capacity((SIZE * SIZE) as usize);
    let inv = 1. / SIZE as f32;
    for y in 0..SIZE {
        let cy = y as f32 * inv - 0.5;
        for x in 0..SIZE {
            let cx = x as f32 * inv - 0.5;
            let center = (cx * cx + cy * cy).sqrt();
            let swirl = 0.35 * center;
            let theta = swirl * swirl * 0.8 * 8.0;
            let (sin, cos) = theta.sin_cos();
            let px = (0.5 + cx * cos - cy * sin).clamp(0., 1.);
            let py = (0.5 + cx * sin + cy * cos).clamp(0., 1.);

            let mut sum = 0.;
            let mut acc = [0f32; 3];
            for (point, color) in points.iter().zip(&rgb) {
                let (dx, dy) = (px - point.0, py - point.1);
                let distance = (0.9 - (dx * dx + dy * dy).sqrt()).max(0.);
                let fourth = distance.powi(4);
                sum += fourth;
                for c in 0..3 {
                    acc[c] += fourth * color[c];
                }
            }
            out.push(acc.map(|v| (v / sum) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_stays_within_the_colors() {
        let colors = tdr_theme::DEFAULT_WALLPAPER_COLORS;
        let rgb = gradient_rgb(&colors, 0, 1.);
        assert_eq!(rgb.len(), 64 * 64);
        // Every pixel is a convex mix: each channel lies within the colors' range.
        for c in 0..3 {
            let channel = |v: u32| (v >> (24 - 8 * c)) as u8;
            let lo = colors.iter().map(|k| channel(k.rgba())).min().unwrap();
            let hi = colors.iter().map(|k| channel(k.rgba())).max().unwrap();
            assert!(
                rgb.iter()
                    .all(|p| p[c] >= lo.saturating_sub(1) && p[c] <= hi)
            );
        }
        // And it is not flat.
        assert_ne!(rgb[0], rgb[64 * 64 - 1]);
    }
}
