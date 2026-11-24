//! Emoji images (lib_ui's `Ui::Emoji`): tdesktop's own pictures, never the system
//! font's. The eight sprites (`emoji_N.webp`, 32 x 16 images of 72 px) are decoded
//! once per size on worker threads and every emoji is cut and scaled on its own
//! (`UniversalImages::generate`) into a disk cache read through a memory mapping
//! (`mapped_store`; tdesktop caches its scaled sprites on disk too): later runs
//! decode nothing, and only the images on screen take the app's own memory.
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

/// Images not painted for this long leave memory and the GPU atlas (they come
/// back from the disk cache at once).
const IMAGE_KEEP: std::time::Duration = std::time::Duration::from_secs(20);

/// Sprites decoded at once the first time a size is needed (each takes ~11 MB
/// while decoded).
const DECODERS: usize = 3;

struct Shown {
    image: Arc<RenderImage>,
    painted: std::time::Instant,
}

#[derive(Default)]
struct Cache {
    /// Sizes being cut from the sprites.
    loading: HashSet<u32>,
    /// Cut images when the disk cache is not available.
    fallback: HashMap<(u16, u32), Arc<[u8]>>,
    images: HashMap<(u16, u32), Shown>,
    retired: Vec<Arc<RenderImage>>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut guard = CACHE.lock().unwrap();
    f(guard.get_or_insert_with(Cache::default))
}

static STORE_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
static STORE: std::sync::OnceLock<Option<Mutex<crate::mapped_store::Store>>> =
    std::sync::OnceLock::new();

/// Where the cut images are cached (`mapped_store`): every emoji at each size
/// is cut from the sprites once, then read back from a mapped file.
pub fn set_cache_dir(dir: std::path::PathBuf) {
    let _ = STORE_DIR.set(dir);
}

fn store() -> Option<std::sync::MutexGuard<'static, crate::mapped_store::Store>> {
    STORE
        .get_or_init(|| {
            // The sprites themselves are the version: new pictures, new cache.
            let version = SPRITE_BYTES.iter().fold(
                {
                    tdr_emoji::init();
                    tdr_emoji::all().len() as u64
                },
                |h, b| (h ^ b.len() as u64).wrapping_mul(0x0000_0100_0000_01b3),
            );
            crate::mapped_store::Store::open(STORE_DIR.get()?, "emoji-images", version, 4)
                .map(Mutex::new)
        })
        .as_ref()
        .map(|m| m.lock().unwrap())
}

/// The marker stored once every emoji of a size is cut.
fn done_key(size: u32) -> crate::mapped_store::Key {
    (-1, size)
}

/// Starts cutting the images for `sizes` (those not cached or cutting yet).
pub fn prepare(sizes: &[u32]) {
    let missing: Vec<u32> = with_cache(|c| {
        let missing: Vec<u32> = sizes
            .iter()
            .copied()
            .filter(|s| *s > 0 && !c.loading.contains(s))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|s| !store().is_some_and(|st| st.contains(done_key(*s))))
            .collect();
        c.loading.extend(&missing);
        missing
    });
    if missing.is_empty() {
        return;
    }
    tdr_emoji::init();
    // A few sprites at a time (decoding one takes a quarter of a second).
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let left = Arc::new(std::sync::atomic::AtomicUsize::new(DECODERS));
    for _ in 0..DECODERS {
        let missing = missing.clone();
        let next = next.clone();
        let left = left.clone();
        std::thread::spawn(move || {
            loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let Some(bytes) = SPRITE_BYTES.get(index) else {
                    break;
                };
                match image::load_from_memory_with_format(bytes, image::ImageFormat::WebP) {
                    Ok(universal) => {
                        let universal = universal.to_rgba8();
                        for &size in &missing {
                            cut_sprite(index, &universal, size);
                        }
                    }
                    Err(_) => tracing::warn!(sprite = index, "emoji: sprite not decoded"),
                }
            }
            if left.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
                for size in &missing {
                    if let Some(mut st) = store() {
                        let _ = st.put(done_key(*size), &[&[1]]);
                    }
                }
                with_cache(|c| {
                    for size in &missing {
                        c.loading.remove(size);
                    }
                });
                crate::repaint::after_ms(0);
            }
        });
    }
}

/// Cuts every image of sprite `index` at `size` (each scaled on its own: no
/// bleeding between cells), with its transparent border, as GPUI's BGRA.
fn cut_sprite(index: usize, universal: &image::RgbaImage, size: u32) {
    let cell = tdr_emoji::UNIVERSAL_SIZE;
    let columns = universal.width() / cell;
    let side = size + 2 * PAD;
    let mut cut = vec![0u8; (side * side * 4) as usize];
    for emoji in 0..tdr_emoji::all().len() {
        let Some(one) = tdr_emoji::get(emoji as u16) else {
            continue;
        };
        if one.sprite() != index {
            continue;
        }
        let (column, row) = (one.column() as u32, one.row() as u32);
        if column >= columns {
            continue;
        }
        let source =
            image::imageops::crop_imm(universal, column * cell, row * cell, cell, cell).to_image();
        let scaled =
            image::imageops::resize(&source, size, size, image::imageops::FilterType::Triangle);
        cut.fill(0);
        for (dy, line) in scaled.rows().enumerate() {
            for (dx, p) in line.enumerate() {
                let at = (((PAD + dy as u32) * side + PAD + dx as u32) * 4) as usize;
                cut[at..at + 4].copy_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        let key = (emoji as u16, size);
        if store()
            .and_then(|mut st| st.put((i64::from(key.0), key.1), &[&cut]))
            .is_none()
        {
            with_cache(|c| {
                c.fallback.insert(key, Arc::from(cut.as_slice()));
            });
        }
    }
}

/// Called once per window frame: images not painted for a while leave memory
/// and the GPU atlas.
pub fn begin_frame(window: &mut Window) {
    let retired = with_cache(|c| {
        let now = std::time::Instant::now();
        c.images.retain(|_, v| {
            let keep = now.duration_since(v.painted) < IMAGE_KEEP;
            if !keep {
                c.retired.push(v.image.clone());
            }
            keep
        });
        std::mem::take(&mut c.retired)
    });
    for image in retired {
        let _ = window.drop_image(image);
    }
}

/// Sizes for the memory report (`TDR_MEMORY`).
pub fn stats() -> String {
    with_cache(|c| {
        let images: usize = c
            .images
            .values()
            .filter_map(|v| v.image.as_bytes(0).map(<[u8]>::len))
            .sum();
        let fallback: usize = c.fallback.values().map(|b| b.len()).sum();
        format!(
            "emoji: images {} ({:.1} MB), not cached {:.1} MB",
            c.images.len(),
            images as f64 / 1048576.,
            fallback as f64 / 1048576.,
        )
    })
}

/// The image of emoji `index` at `size` device pixels with its transparent border,
/// or `None` while the sprites are prepared (another frame is asked for).
pub fn image(index: u16, size: u32, window: &mut Window) -> Option<Arc<RenderImage>> {
    let now = std::time::Instant::now();
    let found = with_cache(|c| {
        if let Some(v) = c.images.get_mut(&(index, size)) {
            v.painted = now;
            return Some(v.image.clone());
        }
        let side = size + 2 * PAD;
        let bytes: Vec<u8> = match c.fallback.get(&(index, size)) {
            Some(b) => b.to_vec(),
            None => store()?.get((i64::from(index), size))?.to_vec(),
        };
        let cut = image::RgbaImage::from_raw(side, side, bytes)?;
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            cut
        )]));
        c.images.insert(
            (index, size),
            Shown {
                image: image.clone(),
                painted: now,
            },
        );
        Some(image)
    });
    if found.is_none() {
        prepare(&[size_normal(), size_large(), size]);
        window.request_animation_frame();
    }
    found
}

/// Paints emoji `index` as a `size` device pixels square with its top left at
/// `origin`.
pub fn paint(index: u16, origin: gpui::Point<Pixels>, size_device: u32, window: &mut Window) {
    let _timer = crate::profile::Timer::new("emoji::paint");
    if crate::profile::off("emoji") {
        return;
    }
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

/// What takes an emoji's place in laid out text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// One of Telegram's emoji images.
    Emoji(u16),
    /// A custom emoji (its document) drawn over its fallback text.
    Custom(i64),
}

/// The emoji of a text: byte ranges and emoji.
pub fn find_all(text: &str) -> Vec<(Range<usize>, Glyph)> {
    // Plain ASCII holds no emoji (the ones starting with a digit, '#' or '*' are
    // keycaps that need U+20E3).
    if text.is_ascii() {
        return Vec::new();
    }
    tdr_emoji::split(text)
        .into_iter()
        .filter_map(|(range, emoji)| emoji.map(|e| (range, Glyph::Emoji(e))))
        .collect()
}

/// `emoji` with the custom emoji of `custom` in place of what they cover.
pub fn with_custom(
    emoji: Vec<(Range<usize>, Glyph)>,
    custom: &[(Range<usize>, i64)],
) -> Vec<(Range<usize>, Glyph)> {
    if custom.is_empty() {
        return emoji;
    }
    // A custom emoji takes a space of three bytes at least.
    let custom: Vec<(Range<usize>, Glyph)> = custom
        .iter()
        .filter(|(r, _)| r.len() >= 3)
        .map(|(r, id)| (r.clone(), Glyph::Custom(*id)))
        .collect();
    let mut out: Vec<(Range<usize>, Glyph)> = emoji
        .into_iter()
        .filter(|(r, _)| {
            !custom
                .iter()
                .any(|(c, _)| c.start < r.end && r.start < c.end)
        })
        .collect();
    out.extend(custom);
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// Paints a glyph whose space starts at `x` in a line at `line_top` of
/// `line_height`; text colored custom emoji take `color`.
pub fn paint_glyph(
    glyph: Glyph,
    x: Pixels,
    line_top: Pixels,
    line_height: Pixels,
    color: gpui::Hsla,
    window: &mut Window,
) {
    match glyph {
        Glyph::Emoji(index) => {
            paint(
                index,
                origin_in_line(x, line_top, line_height),
                size_normal(),
                window,
            );
        }
        Glyph::Custom(id) => {
            // `_customEmojiSkip`: centered on the emoji's place (a bit larger).
            let at = origin_in_line(x, line_top, line_height);
            let side = crate::custom_emoji::size_normal();
            let skip = (size_normal() as f32 - side as f32) / 2. / scale::get();
            let at = point(at.x + px(skip), at.y + px(skip));
            crate::custom_emoji::paint(id, at, side, color, window);
        }
    }
}

/// The text to shape for `text` (a part of a longer text starting at byte `base`
/// of it, whose emoji are `emoji`): each emoji becomes a space of its width at
/// `font_size` plus fillers of the same byte length; `runs` (covering `text`) are
/// cut at the emoji, which get the spaces' font.
pub fn shape_text(
    text: &str,
    base: usize,
    emoji: &[(Range<usize>, Glyph)],
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
pub fn snap(emoji: &[(Range<usize>, Glyph)], index: usize) -> usize {
    match emoji.iter().find(|(r, _)| r.start < index && index < r.end) {
        Some((r, _)) => r.end,
        None => index,
    }
}

/// `index` moved to the start of an emoji it falls inside.
pub fn snap_down(emoji: &[(Range<usize>, Glyph)], index: usize) -> usize {
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

/// The recent emoji and chosen skin tones as the panel keeps them, for the
/// field's replaces and suggestions (`defaultEmojiVariant`, `recentEmoji`).
static RECENT: std::sync::Mutex<Option<tdr_emoji::recent::Recent>> = std::sync::Mutex::new(None);

/// Keeps a copy of the panel's recent emoji (when they changed).
pub fn sync_recent(recent: &tdr_emoji::recent::Recent) {
    let mut guard = RECENT.lock().unwrap();
    if guard.as_ref() != Some(recent) {
        *guard = Some(recent.clone());
    }
}

/// The emoji in the variant chosen for it (`lookupEmojiVariant`).
pub fn variant_of(emoji: u16) -> u16 {
    RECENT
        .lock()
        .unwrap()
        .as_ref()
        .map_or(emoji, |r| r.variant_of(emoji))
}

/// The recent emoji, the most used first.
pub fn recent_list() -> Vec<u16> {
    RECENT
        .lock()
        .unwrap()
        .as_mut()
        .map_or_else(Vec::new, |r| r.list())
}
