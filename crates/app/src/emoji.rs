//! Emoji images (lib_ui's `Ui::Emoji`): tdesktop's own pictures, never the system
//! font's. The eight sprites (`emoji_N.webp`, 32 x 16 images of 72 px) are decoded
//! once on a worker thread and scaled per size (`UniversalImages::generate`); single
//! emoji are cut from them on demand.
//!
//! In text, an emoji is laid out as a space of its width (`emoji_font`) and its image
//! is painted over it: see [`shape_text`].

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use gpui::{Bounds, Corners, Pixels, RenderImage, TextRun, Window, point, px, size};

use crate::emoji_font;
use crate::scale;

/// `st::emojiSize`, `st::emojiPadding`.
pub const EMOJI_SIZE: f32 = 18.;
pub const EMOJI_PADDING: f32 = 1.;

/// `Ui::Emoji::GetSizeNormal`: device pixels.
pub fn size_normal() -> u32 {
    scale::device(EMOJI_SIZE) as u32
}

/// `Ui::Emoji::GetSizeLarge`: `ConvertScale(18 * 4 / 3)`, device pixels.
pub fn size_large() -> u32 {
    scale::device(EMOJI_SIZE * 4. / 3.) as u32
}

/// An emoji's advance in text, logical pixels (`SinglePixmap`'s width).
pub fn advance() -> f32 {
    (scale::device(EMOJI_SIZE) + 2. * scale::device(EMOJI_PADDING)) / scale::get()
}

const SPRITE_BYTES: [&[u8]; tdr_emoji::SPRITES] = [
    include_bytes!("../../../assets/emoji/emoji_1.webp"),
    include_bytes!("../../../assets/emoji/emoji_2.webp"),
    include_bytes!("../../../assets/emoji/emoji_3.webp"),
    include_bytes!("../../../assets/emoji/emoji_4.webp"),
    include_bytes!("../../../assets/emoji/emoji_5.webp"),
    include_bytes!("../../../assets/emoji/emoji_6.webp"),
    include_bytes!("../../../assets/emoji/emoji_7.webp"),
    include_bytes!("../../../assets/emoji/emoji_8.webp"),
];

/// A transparent border around each cut image: GPUI samples atlas images with
/// linear filtering, an image touching its edge would bleed its neighbour.
const PAD: u32 = 1;

#[derive(Default)]
struct Cache {
    /// Scaled sprites by image size (device pixels).
    sprites: HashMap<u32, Vec<Option<Arc<image::RgbaImage>>>>,
    loading: HashSet<u32>,
    images: HashMap<(u16, u32), Arc<RenderImage>>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut guard = CACHE.lock().unwrap();
    f(guard.get_or_insert_with(Cache::default))
}

/// Starts preparing the sprites for `sizes` (those not ready or loading yet).
pub fn prepare(sizes: &[u32]) {
    let missing: Vec<u32> = with_cache(|c| {
        let missing: Vec<u32> = sizes
            .iter()
            .copied()
            .filter(|s| *s > 0 && !c.sprites.contains_key(s) && !c.loading.contains(s))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        c.loading.extend(&missing);
        missing
    });
    if missing.is_empty() {
        return;
    }
    tdr_emoji::init();
    // A thread per sprite: decoding one takes a quarter of a second.
    let left = Arc::new(std::sync::atomic::AtomicUsize::new(SPRITE_BYTES.len()));
    for (index, bytes) in SPRITE_BYTES.iter().enumerate() {
        let missing = missing.clone();
        let left = left.clone();
        std::thread::spawn(move || {
            match image::load_from_memory_with_format(bytes, image::ImageFormat::WebP) {
                Ok(universal) => {
                    let universal = universal.to_rgba8();
                    for &size in &missing {
                        let scaled = Arc::new(scale_sprite(&universal, size));
                        with_cache(|c| {
                            let list = c
                                .sprites
                                .entry(size)
                                .or_insert_with(|| vec![None; tdr_emoji::SPRITES]);
                            list[index] = Some(scaled);
                        });
                    }
                }
                Err(_) => tracing::warn!(sprite = index, "emoji: sprite not decoded"),
            }
            if left.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
                with_cache(|c| {
                    for size in &missing {
                        c.loading.remove(size);
                    }
                });
            }
        });
    }
}

/// Every image of a sprite scaled to `size` on its own (no bleeding between cells).
fn scale_sprite(universal: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let cell = tdr_emoji::UNIVERSAL_SIZE;
    let columns = universal.width() / cell;
    let rows = universal.height() / cell;
    let mut out = image::RgbaImage::new(columns * size, rows * size);
    for row in 0..rows {
        for column in 0..columns {
            let source =
                image::imageops::crop_imm(universal, column * cell, row * cell, cell, cell)
                    .to_image();
            let scaled =
                image::imageops::resize(&source, size, size, image::imageops::FilterType::Triangle);
            image::imageops::replace(
                &mut out,
                &scaled,
                i64::from(column * size),
                i64::from(row * size),
            );
        }
    }
    out
}

/// The image of emoji `index` at `size` device pixels with its transparent border,
/// or `None` while the sprites are prepared (another frame is asked for).
pub fn image(index: u16, size: u32, window: &mut Window) -> Option<Arc<RenderImage>> {
    let found = with_cache(|c| {
        if let Some(image) = c.images.get(&(index, size)) {
            return Some(Some(image.clone()));
        }
        let one = tdr_emoji::get(index)?;
        let sprite = c.sprites.get(&size)?.get(one.sprite())?.clone()?;
        let mut cut = image::RgbaImage::new(size + 2 * PAD, size + 2 * PAD);
        let x = one.column() as u32 * size;
        let y = one.row() as u32 * size;
        for dy in 0..size {
            for dx in 0..size {
                let p = sprite.get_pixel(x + dx, y + dy);
                // GPUI expects BGRA.
                cut.put_pixel(PAD + dx, PAD + dy, image::Rgba([p[2], p[1], p[0], p[3]]));
            }
        }
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            cut
        )]));
        c.images.insert((index, size), image.clone());
        Some(Some(image))
    });
    match found {
        Some(image) => image,
        None => {
            prepare(&[size_normal(), size_large(), size]);
            window.request_animation_frame();
            None
        }
    }
}

/// Paints emoji `index` as a `size` device pixels square with its top left at
/// `origin`.
pub fn paint(index: u16, origin: gpui::Point<Pixels>, size_device: u32, window: &mut Window) {
    let Some(image) = image(index, size_device, window) else {
        return;
    };
    let scale = window.scale_factor();
    let pad = PAD as f32 / scale;
    let side = (size_device + 2 * PAD) as f32 / scale;
    // On the device pixel grid.
    let x = (f32::from(origin.x) * scale).round() / scale - pad;
    let y = (f32::from(origin.y) * scale).round() / scale - pad;
    let bounds = Bounds::new(point(px(x), px(y)), size(px(side), px(side)));
    let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
}

/// The emoji of a text: byte ranges and emoji.
pub fn find_all(text: &str) -> Vec<(Range<usize>, u16)> {
    // Plain ASCII holds no emoji (the ones starting with a digit, '#' or '*' are
    // keycaps that need U+20E3).
    if text.is_ascii() {
        return Vec::new();
    }
    tdr_emoji::split(text)
        .into_iter()
        .filter_map(|(range, emoji)| emoji.map(|e| (range, e)))
        .collect()
}

/// The text to shape for `text` (a part of a longer text starting at byte `base`
/// of it, whose emoji are `emoji`): each emoji becomes a space of its width at
/// `font_size` plus fillers of the same byte length; `runs` (covering `text`) are
/// cut at the emoji, which get the spaces' font.
pub fn shape_text(
    text: &str,
    base: usize,
    emoji: &[(Range<usize>, u16)],
    runs: Vec<TextRun>,
    font_size: Pixels,
) -> (String, Vec<TextRun>) {
    let end = base + text.len();
    let inside: Vec<Range<usize>> = emoji
        .iter()
        .filter(|(r, _)| r.start >= base && r.end <= end)
        .map(|(r, _)| r.start - base..r.end - base)
        .collect();
    if inside.is_empty() {
        return (text.to_string(), runs);
    }
    let space = emoji_font::space_char(advance(), f32::from(font_size));
    let mut display = String::with_capacity(text.len());
    let mut at = 0;
    for r in &inside {
        display.push_str(&text[at..r.start]);
        display.push(space);
        for _ in 3..r.len() {
            display.push(emoji_font::FILLER);
        }
        at = r.end;
    }
    display.push_str(&text[at..]);
    debug_assert_eq!(display.len(), text.len());

    // Cut the runs at the emoji.
    let mut cuts: Vec<usize> = vec![0, text.len()];
    let mut offset = 0;
    for run in &runs {
        offset += run.len;
        cuts.push(offset.min(text.len()));
    }
    for r in &inside {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let run_at = |i: usize| {
        let mut offset = 0;
        for run in &runs {
            if i < offset + run.len {
                return run;
            }
            offset += run.len;
        }
        runs.last().unwrap()
    };
    let font = gpui::font(emoji_font::FAMILY);
    // A color of its own: `layout_line` keeps the previous run's font while the
    // color and decorations stay the same.
    let color = gpui::hsla(0.5, 0.5, 0.5, 0.);
    let mut out = Vec::with_capacity(cuts.len());
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b {
            continue;
        }
        let mut run = run_at(a).clone();
        run.len = b - a;
        if inside.iter().any(|r| r.start <= a && b <= r.end) {
            run.font = font.clone();
            run.color = color;
        }
        out.push(run);
    }
    (display, out)
}

/// `index` moved out of an emoji (to its end) if it falls inside one.
pub fn snap(emoji: &[(Range<usize>, u16)], index: usize) -> usize {
    match emoji.iter().find(|(r, _)| r.start < index && index < r.end) {
        Some((r, _)) => r.end,
        None => index,
    }
}

/// `index` moved to the start of an emoji it falls inside.
pub fn snap_down(emoji: &[(Range<usize>, u16)], index: usize) -> usize {
    match emoji.iter().find(|(r, _)| r.start < index && index < r.end) {
        Some((r, _)) => r.start,
        None => index,
    }
}

/// The emoji image's top left inside a line of text at `x` (the space's start),
/// `line_top` and `line_height`, all logical.
pub fn origin_in_line(x: Pixels, line_top: Pixels, line_height: Pixels) -> gpui::Point<Pixels> {
    let size = size_normal() as f32 / scale::get();
    let pad = scale::device(EMOJI_PADDING) / scale::get();
    let y = ((f32::from(line_height) - size) / 2. * scale::get()).floor() / scale::get();
    point(x + px(pad), line_top + px(y))
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn sprite_timing() {
        let t = std::time::Instant::now();
        let u =
            image::load_from_memory_with_format(super::SPRITE_BYTES[0], image::ImageFormat::WebP)
                .unwrap()
                .to_rgba8();
        let decoded = t.elapsed();
        let _ = super::scale_sprite(&u, 22);
        println!("decode {decoded:?}, scale {:?}", t.elapsed() - decoded);
    }
}
