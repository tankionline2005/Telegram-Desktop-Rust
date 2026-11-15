//! Chat background. The night theme has a solid one; the day theme uses tdesktop's
//! default wallpaper: a four-color "freeform" gradient (ported from lib_ui's
//! `GenerateSmallComplexGradient`) with the doodle pattern (`background.tgv`) over it.

use std::sync::{Arc, LazyLock, Mutex};

use gpui::{Img, ObjectFit, RenderImage, img, prelude::*, relative};
use tdr_theme::Color;

/// lib_ui renders the gradient at 64x64 and scales it smoothly to the chat size.
const SIZE: u32 = 64;
/// Edge pixels repeated around the image. GPUI samples atlas images with linear
/// filtering, so the outer half pixel of a stretched image blends with its
/// transparent atlas surroundings: a grey frame tens of pixels wide. The border is
/// drawn outside the parent (which clips it).
const PAD: u32 = 2;

static DEFAULT_GRADIENT: LazyLock<Arc<RenderImage>> =
    LazyLock::new(|| Arc::new(gradient_image(&tdr_theme::DEFAULT_WALLPAPER_COLORS)));

/// The default wallpaper's gradient, stretched over its parent (which must clip:
/// the repeated border lies outside it).
pub fn default_gradient() -> Img {
    let pad = PAD as f32 / SIZE as f32;
    img(DEFAULT_GRADIENT.clone())
        .absolute()
        .top(relative(-pad))
        .left(relative(-pad))
        .w(relative(1. + 2. * pad))
        .h(relative(1. + 2. * pad))
        .object_fit(ObjectFit::Fill)
}

fn gradient_image(colors: &[Color]) -> RenderImage {
    let rgb = gradient_rgb(colors, 0, 1.);
    let full = SIZE + 2 * PAD;
    let mut buffer = image::RgbaImage::new(full, full);
    for (x, y, px) in buffer.enumerate_pixels_mut() {
        let sx = x.saturating_sub(PAD).min(SIZE - 1);
        let sy = y.saturating_sub(PAD).min(SIZE - 1);
        let [r, g, b] = rgb[(sy * SIZE + sx) as usize];
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

/// The default wallpaper's pattern: a gzipped SVG (`art/background.tgv`).
const PATTERN_SVG: &[u8] = include_bytes!("../../../assets/tdesktop-art/background.tgv");
/// `WallPaper::ConstructDefault`: intensity 50, drawn as `patternOpacity` 0.5.
const PATTERN_OPACITY: f32 = 0.5;

static PATTERN: LazyLock<Option<resvg::usvg::Tree>> = LazyLock::new(|| {
    resvg::usvg::Tree::from_data(PATTERN_SVG, &resvg::usvg::Options::default())
        .map_err(|e| tracing::error!("wallpaper pattern: {e}"))
        .ok()
});

/// A rasterized pattern tile: height, width, alpha per pixel.
type Tile = (u32, u32, Arc<Vec<u8>>);

/// The last rasterized pattern tile.
static TILE: Mutex<Option<Tile>> = Mutex::new(None);

/// The pattern scaled to `height` pixels (its aspect kept), as coverage.
fn pattern_tile(height: u32) -> Option<(u32, Arc<Vec<u8>>)> {
    if let Some((h, w, alpha)) = &*TILE.lock().unwrap()
        && *h == height
    {
        return Some((*w, alpha.clone()));
    }
    let tree = PATTERN.as_ref()?;
    let size = tree.size();
    let scale = height as f32 / size.height();
    let width = ((size.width() * scale).round() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let alpha: Arc<Vec<u8>> = Arc::new(pixmap.pixels().iter().map(|p| p.alpha()).collect());
    *TILE.lock().unwrap() = Some((height, width, alpha.clone()));
    Some((width, alpha))
}

/// The whole day background for a chat area of `width` x `height` device pixels, as
/// tdesktop's `CacheBackgroundByRequest` makes it: the gradient stretched over the
/// area, then the pattern scaled to the area's height and tiled from the center (an
/// odd number of tiles), blended with `SoftLight` at the pattern opacity.
pub fn day_background(width: u32, height: u32) -> RenderImage {
    let (width, height) = (width.max(1), height.max(1));
    let rgb = gradient_rgb(&tdr_theme::DEFAULT_WALLPAPER_COLORS, 0, 1.);
    let mut small = image::RgbImage::new(SIZE, SIZE);
    for (px, c) in small.pixels_mut().zip(rgb) {
        px.0 = c;
    }
    let gradient =
        image::imageops::resize(&small, width, height, image::imageops::FilterType::Triangle);
    let mut out = image::RgbaImage::new(width, height);
    let tile = pattern_tile(height);
    let (tile_w, xshift) = match &tile {
        Some((w, _)) => {
            let cx = width.div_ceil(*w);
            let cols = (cx / 2) * 2 + 1;
            (*w as i64, (width as i64 - cols as i64 * *w as i64) / 2)
        }
        None => (1, 0),
    };
    for (x, y, px) in out.enumerate_pixels_mut() {
        let [r, g, b] = gradient.get_pixel(x, y).0;
        let a = tile.as_ref().map_or(0., |(_, alpha)| {
            let tx = (x as i64 - xshift).rem_euclid(tile_w) as usize;
            alpha[y as usize * tile_w as usize + tx] as f32 / 255. * PATTERN_OPACITY
        });
        // Qt's `soft_light_op` for a black source of alpha `a` over an opaque
        // destination: D * (1 - a * (1 - D)).
        let blend = |d: u8| {
            let d = d as f32 / 255.;
            ((d * (1. - a * (1. - d))) * 255.).round() as u8
        };
        px.0 = [blend(b), blend(g), blend(r), 255]; // GPUI wants BGRA
    }
    RenderImage::new(smallvec::smallvec![image::Frame::new(out)])
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

    #[test]
    fn pattern_darkens_the_gradient_in_places() {
        let (width, alpha) = pattern_tile(296).expect("pattern parses");
        assert_eq!(width, 144); // 1440 x 2960
        let covered = alpha.iter().filter(|&&a| a > 0).count();
        // Doodles, not a fill: some of the tile, far from all of it.
        assert!(covered > alpha.len() / 50 && covered < alpha.len() / 2);
        let image = day_background(300, 296);
        assert_eq!(image.size(0), gpui::size(300.into(), 296.into()));
    }
}
