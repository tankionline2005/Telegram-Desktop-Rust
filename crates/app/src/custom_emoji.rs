//! Custom emoji in the UI (tdesktop's `Data::CustomEmojiManager` with
//! `Ui::CustomEmoji::Instance`): asked for by id wherever they are painted, their
//! documents and files come from the data thread (`Update::CustomEmoji`), they are
//! decoded on worker threads (`.tgs` by `tdr-lottie`, `.webm` by `tdr-webm`, `.webp`
//! by `image`) and rendered once per size into all their frames; painting shows the
//! frame of the moment (all copies of one emoji play in step).

// The panel and the sets use the rest (next steps of this module).
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use gpui::{Bounds, Corners, Pixels, Point, RenderImage, Window, point, px, size};
use tdr_data::emoji_sets::{EmojiDocument, EmojiFormat};

/// What is known of a document.
#[derive(Clone)]
pub struct Info {
    pub alt: String,
    pub set: Option<(i64, i64)>,
    pub text_color: bool,
    pub free: bool,
}

enum Source {
    Lottie(tdr_lottie::Animation),
    /// Straight RGBA frames and how long each is shown (ms).
    Frames {
        width: u32,
        height: u32,
        frames: Vec<(Vec<u8>, u32)>,
    },
}

enum Entry {
    Requested,
    Decoding,
    Ready(Arc<Source>),
    Failed,
}

/// One emoji's frames at one size and color, in memory (BGRA): only the frame on
/// screen is a GPU image (`shown`), so an animation never fills the GPU atlas.
struct Rendered {
    frames: Vec<Vec<u8>>,
    side: u32,
    /// Frame end times (ms from the start), the last one is the loop length.
    ends: Vec<u32>,
}

/// The frame on screen of one emoji at one size and color.
struct Shown {
    frame: usize,
    image: Arc<RenderImage>,
    /// The paint counter when it was last painted (unused ones are dropped).
    painted: u64,
}

/// Animations play at most this often (a frame of the whole window each time).
const FRAME_MS: u32 = 33;
/// Frames kept for emoji not painted for this long are dropped.
const KEEP_UNUSED: u64 = 150;

type RenderKey = (i64, u32, u32);

#[derive(Default)]
struct State {
    entries: HashMap<i64, Entry>,
    info: HashMap<i64, Info>,
    wanted: Vec<i64>,
    rendered: HashMap<RenderKey, Arc<Rendered>>,
    rendering: HashSet<RenderKey>,
    shown: HashMap<RenderKey, Shown>,
    /// Images replaced or dropped, removed from the atlas at the next frame.
    retired: Vec<Arc<RenderImage>>,
    /// Paints so far (`begin_frame`).
    counter: u64,
    /// The paint counter when each emoji was last painted.
    touched: HashMap<i64, u64>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

fn clock() -> u32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

/// The animations' clock: it stands still while they are paused.
fn paused_clock() -> u32 {
    static FROZEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if crate::repaint::paused() {
        FROZEN.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        let now = clock();
        FROZEN.store(now, std::sync::atomic::Ordering::Relaxed);
        now
    }
}

enum Job {
    Decode {
        id: i64,
        format: EmojiFormat,
        bytes: Vec<u8>,
    },
    Render {
        key: RenderKey,
        source: Arc<Source>,
    },
}

/// Decoding and rendering threads (`kWorkersCount` in spirit).
fn jobs() -> &'static Mutex<Sender<Job>> {
    static JOBS: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    JOBS.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for k in 0..4 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("custom-emoji-{k}"))
                .spawn(move || worker(&rx))
                .expect("spawn custom emoji worker");
        }
        Mutex::new(tx)
    })
}

fn worker(rx: &Mutex<Receiver<Job>>) {
    loop {
        let job = {
            let guard = rx.lock().unwrap();
            match guard.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        match job {
            Job::Decode { id, format, bytes } => {
                let source = decode(format, &bytes);
                with(|s| {
                    s.entries.insert(
                        id,
                        match source {
                            Some(source) => Entry::Ready(Arc::new(source)),
                            None => Entry::Failed,
                        },
                    );
                });
            }
            Job::Render { key, source } => {
                let rendered = render(&source, key.1, key.2);
                with(|s| {
                    s.rendering.remove(&key);
                    if let Some(r) = rendered {
                        s.rendered.insert(key, Arc::new(r));
                    }
                });
            }
        }
    }
}

/// At most 3 seconds of video (Telegram's limit for emoji and stickers).
const MAX_FRAMES: usize = 200;

fn decode(format: EmojiFormat, bytes: &[u8]) -> Option<Source> {
    match format {
        EmojiFormat::Tgs => tdr_lottie::Animation::from_tgs(bytes)
            .ok()
            .map(Source::Lottie),
        EmojiFormat::Webm => {
            let d = tdr_webm::decode(bytes, MAX_FRAMES)
                .map_err(|e| tracing::debug!("custom emoji video: {e}"))
                .ok()?;
            // Kept small: an emoji is shown at about 27 px (up to 54 on 200 %).
            const KEEP: u32 = 64;
            let (w, h) = if d.width > KEEP || d.height > KEEP {
                (KEEP, KEEP)
            } else {
                (d.width, d.height)
            };
            (!d.frames.is_empty()).then(|| Source::Frames {
                width: w,
                height: h,
                frames: d
                    .frames
                    .into_iter()
                    .filter_map(|f| {
                        let img = image::RgbaImage::from_raw(d.width, d.height, f.rgba)?;
                        let img = if (w, h) == (d.width, d.height) {
                            img
                        } else {
                            image::imageops::resize(
                                &img,
                                w,
                                h,
                                image::imageops::FilterType::Triangle,
                            )
                        };
                        Some((img.into_raw(), f.duration_ms))
                    })
                    .collect(),
            })
        }
        EmojiFormat::Webp => {
            let image = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
                .ok()?
                .to_rgba8();
            Some(Source::Frames {
                width: image.width(),
                height: image.height(),
                frames: vec![(image.into_raw(), 1000)],
            })
        }
    }
}

/// All frames at `side` device pixels, in BGRA for GPUI; `color` (0xRRGGBBAA, 0:
/// keep) recolors the emoji that follow the text color.
fn render(source: &Source, side: u32, color: u32) -> Option<Rendered> {
    let side = side.max(1);
    let mut frames = Vec::new();
    let mut ends = Vec::new();
    let mut at = 0u32;
    let mut push = |mut rgba: Vec<u8>, duration: u32, frames: &mut Vec<Vec<u8>>| {
        if rgba.len() != (side * side * 4) as usize {
            return None;
        }
        for p in rgba.as_chunks_mut::<4>().0 {
            let [r, g, b, a] = *p;
            *p = if color != 0 {
                [
                    (color >> 8) as u8,
                    (color >> 16) as u8,
                    (color >> 24) as u8,
                    a,
                ]
            } else {
                [b, g, r, a]
            };
        }
        frames.push(rgba);
        at += duration.max(1);
        ends.push(at);
        Some(())
    };
    match source {
        Source::Lottie(anim) => {
            let count = anim.frame_count().min(600);
            let step = (1000. / anim.frame_rate).round().max(1.) as u32;
            let mut pixmap = tdr_lottie::tiny_skia::Pixmap::new(side, side)?;
            for k in 0..count {
                pixmap.fill(tdr_lottie::tiny_skia::Color::TRANSPARENT);
                anim.render_into(k, &mut pixmap);
                let mut rgba = Vec::with_capacity((side * side * 4) as usize);
                for p in pixmap.pixels() {
                    let c = p.demultiply();
                    rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
                }
                push(rgba, step, &mut frames)?;
            }
        }
        Source::Frames {
            width,
            height,
            frames: list,
        } => {
            for (rgba, duration) in list {
                let img = image::RgbaImage::from_raw(*width, *height, rgba.clone())?;
                let scaled = if *width == side && *height == side {
                    img
                } else {
                    image::imageops::resize(&img, side, side, image::imageops::FilterType::Triangle)
                };
                push(scaled.into_raw(), *duration, &mut frames)?;
            }
        }
    }
    if frames.is_empty() {
        return None;
    }
    Some(Rendered { frames, side, ends })
}

/// Called once per frame of the window, before painting: drops from the GPU
/// atlas the frames replaced since, and the emoji not painted for a while.
pub fn begin_frame(window: &mut Window) {
    let retired = with(|s| {
        s.counter += 1;
        let counter = s.counter;
        let stale: Vec<RenderKey> = s
            .shown
            .iter()
            .filter(|(_, v)| counter - v.painted > KEEP_UNUSED)
            .map(|(k, _)| *k)
            .collect();
        for key in stale {
            if let Some(v) = s.shown.remove(&key) {
                s.retired.push(v.image);
            }
        }
        // Emoji not painted for a while: their frames and decoded data go (they
        // come back from the file cache when painted again).
        let old: Vec<i64> = s
            .touched
            .iter()
            .filter(|(_, at)| counter - **at > KEEP_UNUSED)
            .map(|(id, _)| *id)
            .collect();
        for id in old {
            s.touched.remove(&id);
            s.rendered.retain(|k, _| k.0 != id);
            if matches!(s.entries.get(&id), Some(Entry::Ready(_))) {
                s.entries.remove(&id);
            }
        }
        std::mem::take(&mut s.retired)
    });
    for image in retired {
        let _ = window.drop_image(image);
    }
}

fn due_in(_: &mut State, ms: u32) {
    crate::repaint::after_ms(u64::from(ms));
}

/// The ids to ask the data thread for (since the last call).
pub fn take_wanted() -> Vec<i64> {
    with(|s| std::mem::take(&mut s.wanted))
}

/// Asks for an emoji's document and file (once).
pub fn want(id: i64) {
    with(|s| {
        if let std::collections::hash_map::Entry::Vacant(e) = s.entries.entry(id) {
            e.insert(Entry::Requested);
            s.wanted.push(id);
        }
    });
}

/// The document and file came: decode it.
pub fn loaded(doc: &EmojiDocument, bytes: Vec<u8>) {
    let start = with(|s| {
        s.info.insert(
            doc.id,
            Info {
                alt: doc.alt.clone(),
                set: doc.set,
                text_color: doc.text_color,
                free: doc.free,
            },
        );
        match s.entries.get(&doc.id) {
            Some(Entry::Ready(_)) | Some(Entry::Decoding) => false,
            _ => {
                s.entries.insert(doc.id, Entry::Decoding);
                true
            }
        }
    });
    if start {
        let _ = jobs().lock().unwrap().send(Job::Decode {
            id: doc.id,
            format: doc.format,
            bytes,
        });
    }
}

/// The document could not be loaded.
pub fn failed(id: i64) {
    with(|s| {
        s.entries.insert(id, Entry::Failed);
    });
}

/// Known details of an emoji (after its document came).
pub fn info(id: i64) -> Option<Info> {
    with(|s| s.info.get(&id).cloned())
}

/// Remembers a document's details without its file (from a set's list).
pub fn remember(doc: &EmojiDocument) {
    with(|s| {
        s.info.entry(doc.id).or_insert_with(|| Info {
            alt: doc.alt.clone(),
            set: doc.set,
            text_color: doc.text_color,
            free: doc.free,
        });
    });
}

/// Paints emoji `id` as a `side` device pixels square at `origin` (logical); text
/// colored emoji take `color`. False while it is not ready (it is asked for, and
/// another frame is requested).
pub fn paint(
    id: i64,
    origin: Point<Pixels>,
    side: u32,
    color: gpui::Hsla,
    window: &mut Window,
) -> bool {
    let _timer = crate::profile::Timer::new("custom_emoji::paint");
    want(id);
    let text_color = with(|s| {
        let counter = s.counter;
        s.touched.insert(id, counter);
        s.info.get(&id).is_some_and(|i| i.text_color)
    });
    let color_key = if text_color {
        let c = gpui::Rgba::from(color);
        let to = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
        (to(c.r) << 24) | (to(c.g) << 16) | (to(c.b) << 8) | 0xff
    } else {
        0
    };
    let key = (id, side, color_key);
    let found = with(|s| {
        if let Some(r) = s.rendered.get(&key) {
            return Ok(Some(r.clone()));
        }
        match s.entries.get(&id) {
            Some(Entry::Ready(source)) => {
                if s.rendering.insert(key) {
                    Err(Some(source.clone()))
                } else {
                    Ok(None)
                }
            }
            Some(Entry::Failed) => Err(None),
            _ => Ok(None),
        }
    });
    let rendered = match found {
        Ok(Some(r)) => r,
        Ok(None) => {
            with(|s| due_in(s, 50));
            return false;
        }
        Err(Some(source)) => {
            let _ = jobs().lock().unwrap().send(Job::Render { key, source });
            with(|s| due_in(s, 50));
            return false;
        }
        Err(None) => return false,
    };
    let total = *rendered.ends.last().unwrap_or(&1);
    // The frame of this moment, at most `FRAME_MS` apart.
    let t = if rendered.ends.len() > 1 {
        (paused_clock() / FRAME_MS * FRAME_MS) % total.max(1)
    } else {
        0
    };
    let frame = rendered
        .ends
        .partition_point(|end| *end <= t)
        .min(rendered.ends.len() - 1);
    let image = with(|s| {
        let counter = s.counter;
        if rendered.ends.len() > 1 {
            let next = rendered.ends[frame].saturating_sub(t).max(FRAME_MS);
            due_in(s, next);
        }
        if let Some(v) = s.shown.get_mut(&key) {
            v.painted = counter;
            if v.frame == frame {
                return v.image.clone();
            }
        }
        let side = rendered.side;
        let img = image::RgbaImage::from_raw(side, side, rendered.frames[frame].clone())
            .unwrap_or_else(|| image::RgbaImage::new(side, side));
        let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
            img
        )]));
        if let Some(old) = s.shown.insert(
            key,
            Shown {
                frame,
                image: image.clone(),
                painted: counter,
            },
        ) {
            s.retired.push(old.image);
        }
        image
    });
    let scale = window.scale_factor();
    let x = (f32::from(origin.x) * scale).round() / scale;
    let y = (f32::from(origin.y) * scale).round() / scale;
    let side_l = side as f32 / scale;
    let bounds = Bounds::new(point(px(x), px(y)), size(px(side_l), px(side_l)));
    let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
    true
}

/// `Ui::Emoji::GetCustomSizeNormal`: device pixels of a custom emoji in text.
pub fn size_normal() -> u32 {
    let esize = (crate::emoji::size_normal() as f32 / crate::scale::get()).floor();
    ((esize * 1.12).round() * crate::scale::get()).round() as u32
}

/// `GetCustomSizeLarge`: in the emoji panel.
pub fn size_large() -> u32 {
    let esize = (crate::emoji::size_large() as f32 / crate::scale::get()).floor();
    ((esize * 1.12).round() * crate::scale::get()).round() as u32
}
