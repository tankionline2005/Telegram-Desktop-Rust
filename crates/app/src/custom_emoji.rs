//! Custom emoji in the UI (tdesktop's `Data::CustomEmojiManager` with
//! `Ui::CustomEmoji::Instance`): asked for by id wherever they are painted, their
//! documents and files come from the data thread (`Update::CustomEmoji`), and each
//! (emoji, size) is one instance shared by every place that paints it, as there.
//!
//! An instance plays at the animation's own rate (one frame per paint at most,
//! `Cache::paintCurrentFrame`) over all its frames (at most `kMaxFrames`).
//! Differences from tdesktop, which shows the first frame as soon as it is
//! rendered and renders ahead of the one on screen (`kPreloadFrames`):
//! - an emoji shows its loading silhouette until all its frames are rendered,
//!   then plays smoothly from the first: with many new emoji at once (a set
//!   just opened) they come one after another, in reading order, instead of
//!   all playing in jerks while the renderers catch up (asked by the user);
//! - a worker renders one emoji to its end; the emoji on screen go first (the
//!   last painted frame, then in the order they were painted), the one in the
//!   long press preview before all (`focus`), and those scrolled away wait;
//! - kept frames are compressed in memory (each frame XOR the previous one, zero
//!   runs, decoded in microseconds; text colored emoji keep only alpha): about
//!   2.5 times less than tdesktop's uncompressed frames (deflate gains little
//!   more on these frames and unpacks several times slower), so scrolling back
//!   shows them at once instead of rendering them again; only the frame on
//!   screen is a GPU image;
//! - an animation rendered to its end goes to a disk cache read through a
//!   memory mapping (`mapped_store`): its frames leave the app's own memory at
//!   once (Windows keeps them in its file cache, read back at memory speed) and
//!   after a restart emoji show without being rendered again.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{Bounds, Corners, Pixels, Point, RenderImage, Window, point, px, size};
use tdr_data::emoji_sets::{EmojiDocument, EmojiFormat};

/// What is known of a document.
#[derive(Clone)]
pub struct Info {
    pub alt: String,
    pub set: Option<(i64, i64)>,
    pub text_color: bool,
    pub free: bool,
    /// The vector thumbnail (`photoPathSize`), drawn while loading (parsed
    /// only when a silhouette image is made: sets bring thousands of documents).
    pub path: Option<Arc<[u8]>>,
}

impl Info {
    fn of(doc: &EmojiDocument) -> Self {
        Self {
            alt: doc.alt.clone(),
            set: doc.set,
            text_color: doc.text_color,
            free: doc.free,
            path: doc.path.as_deref().map(Arc::from),
        }
    }
}

/// What is drawn while an emoji loads (`Ui::CustomEmoji::Loading`).
#[derive(Clone, Copy)]
pub enum Loading {
    /// The thumbnail's silhouette in the text color at an eighth of its alpha
    /// (`PreviewColorFromTextColor`).
    Silhouette,
    /// `PathShiftGradient` (the set box): the silhouette in `bg` with a band of
    /// `fg` sliding over `left..left + width` (logical, window) for a second,
    /// then a second of rest.
    Shimmer {
        left: f32,
        width: f32,
        bg: gpui::Rgba,
        fg: gpui::Rgba,
    },
}

enum Doc {
    Requested,
    File {
        format: EmojiFormat,
        bytes: Arc<[u8]>,
    },
    /// The file was let go to save memory (asked for again when needed).
    Dropped,
    Failed,
}

/// `kMaxFrames`: longer animations are cut (and then loop).
const MAX_FRAMES: usize = 180;
/// The preview's emoji goes first while it was painted this recently.
const FOCUS_KEEP: Duration = Duration::from_millis(300);
/// A frame is stored whole every this many frames (the rest as differences).
const KEY_EVERY: usize = 8;
/// Compressed frames kept in memory for unfinished emoji off screen (finished
/// ones are on disk), in bytes.
const BUDGET: usize = 8 << 20;
/// Files kept in memory (they come back from the file cache when needed).
const FILES_BUDGET: usize = 2 << 20;
/// Instances of finished emoji off screen this long are dropped (they come
/// back from the disk cache at once).
const INSTANCE_KEEP: Duration = Duration::from_secs(10);
/// The disk cache: the blob format's version (a new renderer or format makes
/// a new cache) and its size (segments of 32 MB: 256 MB, started anew when full).
const STORE_VERSION: u64 = 1;
const STORE_SEGMENTS: u32 = 8;
const BLOB_MAGIC: u32 = 0x4543_4454;
/// Workers stop rendering an emoji that was not painted for this long.
const IDLE: Duration = Duration::from_millis(400);
/// GPU images of emoji not painted for this long are dropped.
const SHOWN_KEEP: Duration = Duration::from_secs(2);

/// What a worker renders from (parsed once per instance, dropped when done).
enum Source {
    Lottie(tdr_lottie::Animation),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Idle,
    Queued,
    Running,
}

/// One emoji at one size (`Ui::CustomEmoji::Instance`).
struct Instance {
    /// Frames packed one after another (`pack`), `offsets[k]..offsets[k + 1]`:
    /// in memory while rendered, then in the disk cache.
    data: Vec<u8>,
    disk: Option<&'static [u8]>,
    offsets: Vec<usize>,
    durations: Vec<u16>,
    finished: bool,
    /// Text colored emoji keep only alpha (1 byte a pixel), others BGRA.
    alpha_only: bool,
    side: u32,
    /// The frame on screen and when it was shown.
    frame: usize,
    shown_at: Option<Instant>,
    /// The window paint and the paint order it was last painted in (workers
    /// take the emoji on screen in reading order).
    seen_frame: u64,
    seen_order: u64,
    /// The paint counter of the last advance (once per window paint).
    advanced: u64,
    painted: Instant,
    job: Job,
    source: Option<Arc<Source>>,
    /// The last unpacked frame (the next one is a difference to it).
    unpacked: Option<(usize, Vec<u8>)>,
}

impl Instance {
    fn new(side: u32) -> Self {
        Self {
            data: Vec::new(),
            disk: None,
            offsets: vec![0],
            durations: Vec::new(),
            finished: false,
            alpha_only: false,
            side,
            frame: 0,
            seen_frame: 0,
            seen_order: 0,
            shown_at: None,
            advanced: 0,
            painted: Instant::now(),
            job: Job::Idle,
            source: None,
            unpacked: None,
        }
    }

    fn frames(&self) -> usize {
        self.durations.len()
    }

    /// Memory held (frames on disk are not).
    fn bytes(&self) -> usize {
        self.data.capacity()
            + self.offsets.capacity() * 8
            + self.durations.capacity() * 2
            + self.unpacked.as_ref().map_or(0, |u| u.1.capacity())
    }

    fn packed(&self) -> &[u8] {
        self.disk.unwrap_or(&self.data)
    }

    /// The disk cache's blob: frames, flags, side, durations, offsets, data.
    fn blob_header(&self) -> Vec<u8> {
        let n = self.frames();
        let mut out = Vec::with_capacity(12 + n * 6 + 4);
        out.extend_from_slice(&BLOB_MAGIC.to_le_bytes());
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.push(u8::from(self.alpha_only));
        out.push(0);
        out.extend_from_slice(&self.side.to_le_bytes());
        for d in &self.durations {
            out.extend_from_slice(&d.to_le_bytes());
        }
        for o in &self.offsets {
            out.extend_from_slice(&(*o as u32).to_le_bytes());
        }
        out
    }

    /// A finished instance from the disk cache's blob.
    fn from_blob(blob: &'static [u8], side: u32) -> Option<Self> {
        let u16_at = |at: usize| Some(u16::from_le_bytes(blob.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_le_bytes(blob.get(at..at + 4)?.try_into().ok()?));
        if u32_at(0)? != BLOB_MAGIC || u32_at(8)? != side {
            return None;
        }
        let n = usize::from(u16_at(4)?);
        let alpha_only = *blob.get(6)? != 0;
        let durations: Vec<u16> = (0..n).map(|k| u16_at(12 + 2 * k)).collect::<Option<_>>()?;
        let at = 12 + 2 * n;
        let offsets: Vec<usize> = (0..=n)
            .map(|k| u32_at(at + 4 * k).map(|v| v as usize))
            .collect::<Option<_>>()?;
        let data = blob.get(at + 4 * (n + 1)..)?;
        if n == 0 || offsets.windows(2).any(|w| w[0] > w[1]) || offsets[n] != data.len() {
            return None;
        }
        let mut i = Instance::new(side);
        i.disk = Some(data);
        i.offsets = offsets;
        i.durations = durations;
        i.finished = true;
        i.alpha_only = alpha_only;
        Some(i)
    }

    fn push(&mut self, raw: &[u8], duration: u32) {
        let k = self.frames();
        let prev = if k.is_multiple_of(KEY_EVERY) {
            None
        } else {
            self.unpack(k - 1).map(|p| p.to_vec())
        };
        pack(raw, prev.as_deref(), &mut self.data);
        self.offsets.push(self.data.len());
        self.durations
            .push(duration.clamp(1, u16::MAX as u32) as u16);
        self.unpacked = Some((k, raw.to_vec()));
    }

    /// The pixels of frame `k` (alpha or BGRA).
    fn unpack(&mut self, k: usize) -> Option<&[u8]> {
        if k >= self.frames() {
            return None;
        }
        let len = (self.side * self.side) as usize * if self.alpha_only { 1 } else { 4 };
        let mut from = match &self.unpacked {
            Some((at, _)) if *at == k => return self.unpacked.as_ref().map(|u| &u.1[..]),
            Some((at, _)) if *at < k && k - at < KEY_EVERY && k / KEY_EVERY == at / KEY_EVERY => {
                at + 1
            }
            _ => k - k % KEY_EVERY,
        };
        let mut buf = match self.unpacked.take() {
            Some((_, b)) if b.len() == len => b,
            _ => vec![0; len],
        };
        if from.is_multiple_of(KEY_EVERY) {
            buf.fill(0);
        }
        while from <= k {
            let packed = &self.packed()[self.offsets[from]..self.offsets[from + 1]];
            unpack_into(packed, &mut buf);
            from += 1;
        }
        self.unpacked = Some((k, buf));
        self.unpacked.as_ref().map(|u| &u.1[..])
    }

    /// `Cache::paintCurrentFrame`: the next frame once the current one has been
    /// shown for its duration (one step a paint), back to the first after the last.
    fn advance(&mut self, now: Instant, counter: u64) {
        if self.advanced == counter || self.frames() == 0 {
            return;
        }
        self.advanced = counter;
        let Some(shown) = self.shown_at else {
            self.shown_at = Some(now);
            return;
        };
        let ends = shown + Duration::from_millis(u64::from(self.durations[self.frame]));
        if now < ends {
            return;
        }
        if self.frame + 1 < self.frames() {
            self.frame += 1;
            self.shown_at = Some(now);
        } else if self.finished {
            if self.frames() > 1 {
                self.frame = 0;
            }
            self.shown_at = Some(now);
        }
    }

    fn next_due(&self) -> Option<Instant> {
        if self.frames() <= 1 && self.finished {
            return None;
        }
        let shown = self.shown_at?;
        Some(shown + Duration::from_millis(u64::from(self.durations[self.frame])))
    }
}

/// The frame on screen of one emoji at one size and color, as a GPU image.
struct Shown {
    frame: usize,
    image: Arc<RenderImage>,
    painted: Instant,
}

type Key = (i64, u32);

#[derive(Default)]
struct State {
    docs: HashMap<i64, Doc>,
    info: HashMap<i64, Info>,
    wanted: Vec<i64>,
    instances: HashMap<Key, Instance>,
    shown: HashMap<(i64, u32, u32), Shown>,
    /// Loading silhouettes on screen (`Loading`), by emoji and size.
    loading: HashMap<(i64, u32), Shown>,
    /// Images replaced or dropped, removed from the atlas at the next frame.
    retired: Vec<Arc<RenderImage>>,
    /// Window paints so far (`begin_frame`).
    counter: u64,
    /// Instances waiting for a worker, and the paints so far (their order).
    queued: Vec<Key>,
    paints: u64,
    /// The long press preview's emoji and when it was painted.
    focus: Option<(Key, Instant)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static STORE: OnceLock<Option<Mutex<crate::mapped_store::Store>>> = OnceLock::new();
static STORE_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Where the disk cache lives (before the first emoji is painted).
pub fn set_cache_dir(dir: std::path::PathBuf) {
    let _ = STORE_DIR.set(dir);
}

fn store() -> Option<std::sync::MutexGuard<'static, crate::mapped_store::Store>> {
    STORE
        .get_or_init(|| {
            let dir = STORE_DIR.get()?;
            crate::mapped_store::Store::open(dir, "emoji-frames", STORE_VERSION, STORE_SEGMENTS)
                .map(Mutex::new)
        })
        .as_ref()
        .map(|m| m.lock().unwrap())
}

/// A finished instance goes to the disk cache: its frames leave memory.
fn persist(key: Key, i: &mut Instance) {
    if !i.finished || i.disk.is_some() || i.frames() == 0 {
        return;
    }
    let header = i.blob_header();
    let Some(blob) = store().and_then(|mut st| st.put(key, &[&header, &i.data])) else {
        return;
    };
    i.disk = Some(&blob[header.len()..]);
    i.data = Vec::new();
}

/// An instance from the disk cache, if it is there.
fn from_disk(key: Key) -> Option<Instance> {
    let blob = store()?.get(key)?;
    Instance::from_blob(blob, key.1)
}
static WAKE: Condvar = Condvar::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

fn queue(s: &mut State, key: Key) {
    let Some(i) = s.instances.get_mut(&key) else {
        return;
    };
    if i.job != Job::Idle {
        return;
    }
    i.job = Job::Queued;
    s.queued.push(key);
    start_workers();
    WAKE.notify_one();
}

/// The emoji in the long press preview: rendered before all others.
pub fn focus(id: i64, side: u32) {
    with(|s| s.focus = Some(((id, side), Instant::now())));
}

fn focused(s: &State) -> Option<Key> {
    s.focus
        .filter(|(_, at)| at.elapsed() < FOCUS_KEEP)
        .map(|(key, _)| key)
}

fn start_workers() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get());
        for k in 0..(n.saturating_sub(1)).clamp(1, 4) {
            std::thread::Builder::new()
                .name(format!("custom-emoji-{k}"))
                .spawn(worker)
                .expect("spawn custom emoji worker");
        }
    });
}

/// What a worker took: the instance, its file and source, where to go on.
struct Task {
    key: Key,
    file: Option<(EmojiFormat, Arc<[u8]>)>,
    source: Option<Arc<Source>>,
    from: usize,
    alpha_only: bool,
}

fn take_task(s: &mut State) -> Option<Task> {
    let now = Instant::now();
    // Scrolled away: left for when it is painted again.
    let instances = &mut s.instances;
    s.queued.retain(|key| match instances.get_mut(key) {
        Some(i) if i.job == Job::Queued => {
            if now.duration_since(i.painted) > IDLE {
                i.job = Job::Idle;
                false
            } else {
                true
            }
        }
        _ => false,
    });
    let focus = focused(s);
    let k = s
        .queued
        .iter()
        .enumerate()
        .min_by_key(|(_, key)| {
            let i = &s.instances[*key];
            (
                Some(**key) != focus,
                std::cmp::Reverse(i.seen_frame),
                i.seen_order,
            )
        })
        .map(|(k, _)| k)?;
    let key = s.queued.swap_remove(k);
    {
        let i = s.instances.get_mut(&key)?;
        i.job = Job::Running;
        let task = Task {
            key,
            file: None,
            source: i.source.clone(),
            from: i.frames(),
            alpha_only: s.info.get(&key.0).is_some_and(|f| f.text_color),
        };
        let file = match s.docs.get(&key.0) {
            Some(Doc::File { format, bytes }) => Some((*format, bytes.clone())),
            _ => None,
        };
        Some(Task { file, ..task })
    }
}

fn worker() {
    loop {
        let task = {
            let mut guard = STATE.lock().unwrap();
            loop {
                if let Some(task) = take_task(guard.get_or_insert_with(State::default)) {
                    break task;
                }
                guard = WAKE.wait(guard).unwrap();
            }
        };
        run(task);
        crate::repaint::after_ms(0);
    }
}

fn finish_job(key: Key, f: impl FnOnce(&mut Instance)) {
    with(|s| {
        if let Some(i) = s.instances.get_mut(&key) {
            f(i);
            i.job = Job::Idle;
        }
    });
}

fn run(task: Task) {
    let key = task.key;
    let side = key.1.max(1);
    let source = match (task.source, &task.file) {
        (Some(source), _) => source,
        (None, Some((EmojiFormat::Tgs, bytes))) => match tdr_lottie::Animation::from_tgs(bytes) {
            Ok(anim) => Arc::new(Source::Lottie(anim)),
            Err(_) => return fail(key),
        },
        (None, Some((format, bytes))) => {
            // Videos and images: all frames at once (decoding is the cost).
            let frames = match decode_frames(*format, bytes, side, task.alpha_only) {
                Some(f) if !f.is_empty() => f,
                _ => return fail(key),
            };
            return finish_job(key, |i| {
                if i.frames() == 0 {
                    i.alpha_only = task.alpha_only;
                    for (raw, duration) in &frames {
                        i.push(raw, *duration);
                    }
                    i.finished = true;
                    i.unpacked = None;
                    persist(key, i);
                }
            });
        }
        (None, None) => return finish_job(key, |_| {}),
    };
    let Source::Lottie(anim) = &*source;
    let count = anim.frame_count().min(MAX_FRAMES);
    let step = (1000. / anim.frame_rate.max(1.)).round().max(1.) as u32;
    // The preview's emoji is rendered on every core, a batch of frames at a
    // time (pushed in order: each is packed against the previous one).
    let threads = if with(|s| focused(s) == Some(key)) {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        1
    };
    let mut k = task.from;
    while k < count {
        let batch: Vec<usize> = (k..count.min(k + threads * 2)).collect();
        let frames: Vec<Vec<u8>> = if threads > 1 && batch.len() > 1 {
            std::thread::scope(|scope| {
                let parts: Vec<_> = (0..threads)
                    .map(|t| {
                        let mine: Vec<usize> =
                            batch.iter().copied().skip(t).step_by(threads).collect();
                        scope.spawn(move || {
                            mine.into_iter()
                                .map(|f| (f, render_frame(anim, f, side, task.alpha_only)))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                let mut all: Vec<(usize, Vec<u8>)> = parts
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_default())
                    .collect();
                all.sort_by_key(|(f, _)| *f);
                all.into_iter().map(|(_, raw)| raw).collect()
            })
        } else {
            batch
                .iter()
                .map(|f| render_frame(anim, *f, side, task.alpha_only))
                .collect()
        };
        let mut go_on = true;
        for raw in frames {
            k += 1;
            // To the end while the emoji is on screen, unless the preview's emoji
            // waits for a worker.
            go_on = with(|s| {
                let focus = focused(s).filter(|f| *f != key && s.queued.contains(f));
                let Some(i) = s.instances.get_mut(&key) else {
                    return false;
                };
                if i.frames() + 1 != k {
                    return false;
                }
                i.alpha_only = task.alpha_only;
                i.push(&raw, step);
                if k == count {
                    i.finished = true;
                    i.source = None;
                    persist(key, i);
                    return false;
                }
                focus.is_none() && i.painted.elapsed() <= IDLE
            });
            if !go_on {
                break;
            }
        }
        if !go_on {
            break;
        }
    }
    let keep = source.clone();
    finish_job(key, move |i| {
        if !i.finished {
            i.source = Some(keep);
        }
    });
}

/// One frame as BGRA (or alpha only), straight alpha.
fn render_frame(
    anim: &tdr_lottie::Animation,
    frame: usize,
    side: u32,
    alpha_only: bool,
) -> Vec<u8> {
    let Some(mut pixmap) = tdr_lottie::tiny_skia::Pixmap::new(side, side) else {
        return Vec::new();
    };
    anim.render_into(frame, &mut pixmap);
    let mut raw = Vec::with_capacity((side * side * if alpha_only { 1 } else { 4 }) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        if alpha_only {
            raw.push(c.alpha());
        } else {
            raw.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
        }
    }
    raw
}

fn fail(key: Key) {
    with(|s| {
        s.docs.insert(key.0, Doc::Failed);
        s.instances.remove(&key);
    });
}

/// All frames of a video or image at `side` (BGRA or alpha) with their durations.
fn decode_frames(
    format: EmojiFormat,
    bytes: &[u8],
    side: u32,
    alpha_only: bool,
) -> Option<Vec<(Vec<u8>, u32)>> {
    let convert = |img: image::RgbaImage| -> Vec<u8> {
        let img = if img.width() == side && img.height() == side {
            img
        } else {
            image::imageops::resize(&img, side, side, image::imageops::FilterType::Triangle)
        };
        if alpha_only {
            img.pixels().map(|p| p.0[3]).collect()
        } else {
            let mut raw = img.into_raw();
            for p in raw.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            raw
        }
    };
    match format {
        EmojiFormat::Webm => {
            let d = tdr_webm::decode(bytes, MAX_FRAMES)
                .map_err(|e| tracing::debug!("custom emoji video: {e}"))
                .ok()?;
            Some(
                d.frames
                    .into_iter()
                    .filter_map(|f| {
                        let img = image::RgbaImage::from_raw(d.width, d.height, f.rgba)?;
                        Some((convert(img), f.duration_ms))
                    })
                    .collect(),
            )
        }
        EmojiFormat::Webp => {
            let img = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
                .ok()?
                .to_rgba8();
            Some(vec![(convert(img), 1000)])
        }
        EmojiFormat::Tgs => None,
    }
}

fn varint(mut v: usize, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_varint(data: &[u8], at: &mut usize) -> usize {
    let mut v = 0usize;
    let mut shift = 0;
    while let Some(&b) = data.get(*at) {
        *at += 1;
        v |= usize::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    v
}

/// Packs `cur` (XOR `prev`, if any) as runs: zeros count, literals count, literals.
fn pack(cur: &[u8], prev: Option<&[u8]>, out: &mut Vec<u8>) {
    let byte = |i: usize| cur[i] ^ prev.map_or(0, |p| p[i]);
    let n = cur.len();
    let mut i = 0;
    while i < n {
        let start = i;
        while i < n && byte(i) == 0 {
            i += 1;
        }
        let zeros = i - start;
        let lit = i;
        // Literals until a run of at least 3 zeros (or the end).
        while i < n {
            if byte(i) == 0 && i + 2 < n && byte(i + 1) == 0 && byte(i + 2) == 0 {
                break;
            }
            i += 1;
        }
        varint(zeros, out);
        varint(i - lit, out);
        out.extend((lit..i).map(byte));
    }
}

/// Applies a packed frame to `buf` (the previous frame, or zeros for a whole one).
fn unpack_into(packed: &[u8], buf: &mut [u8]) {
    let mut at = 0;
    let mut i = 0;
    while at < packed.len() && i < buf.len() {
        i += read_varint(packed, &mut at);
        let lits = read_varint(packed, &mut at);
        let end = (i + lits).min(buf.len());
        for (b, x) in buf[i..end].iter_mut().zip(&packed[at..]) {
            *b ^= x;
        }
        at += lits;
        i = end;
    }
}

/// Called once per frame of the window, before painting: drops from the GPU
/// atlas the images replaced since, and the emoji not painted for a while.
pub fn begin_frame(window: &mut Window) {
    let retired = with(|s| {
        s.counter += 1;
        let now = Instant::now();
        let stale: Vec<_> = s
            .shown
            .iter()
            .filter(|(_, v)| now.duration_since(v.painted) > SHOWN_KEEP)
            .map(|(k, _)| *k)
            .collect();
        for key in stale {
            if let Some(v) = s.shown.remove(&key) {
                s.retired.push(v.image);
            }
        }
        // Silhouettes of emoji that loaded or went away.
        let stale: Vec<_> = s
            .loading
            .iter()
            .filter(|(_, v)| now.duration_since(v.painted) > Duration::from_millis(100))
            .map(|(k, _)| *k)
            .collect();
        for key in stale {
            if let Some(v) = s.loading.remove(&key) {
                s.retired.push(v.image);
            }
        }
        if s.counter.is_multiple_of(60) {
            trim(s, now);
        }
        std::mem::take(&mut s.retired)
    });
    for image in retired {
        let _ = window.drop_image(image);
    }
}

/// Keeps the frames of emoji off screen within `BUDGET`, the least recently
/// painted go first; sources of unfinished ones off screen are dropped.
fn trim(s: &mut State, now: Instant) {
    let mut total = 0;
    let mut off: Vec<(Instant, Key, usize)> = Vec::new();
    let mut drop_now = Vec::new();
    for (key, i) in s.instances.iter_mut() {
        if now.duration_since(i.painted) > SHOWN_KEEP && i.job == Job::Idle {
            i.source = None;
            i.unpacked = None;
            persist(*key, i);
            if i.disk.is_some() && now.duration_since(i.painted) > INSTANCE_KEEP {
                drop_now.push(*key);
                continue;
            }
            off.push((i.painted, *key, i.bytes()));
        }
        total += i.bytes();
    }
    for key in drop_now {
        s.instances.remove(&key);
    }
    off.sort_by_key(|o| o.0);
    for (_, key, bytes) in off {
        if total <= BUDGET {
            break;
        }
        s.instances.remove(&key);
        total -= bytes;
    }
    // Files of emoji rendered (or not shown) go first; a dropped one is asked
    // for again only if it has to be rendered.
    let rendering: std::collections::HashSet<i64> = s
        .instances
        .iter()
        .filter(|(_, i)| !i.finished)
        .map(|(k, _)| k.0)
        .collect();
    let mut files = 0;
    let mut spare = Vec::new();
    for (id, doc) in &s.docs {
        if let Doc::File { bytes, .. } = doc {
            files += bytes.len();
            if !rendering.contains(id) {
                spare.push((*id, bytes.len()));
            }
        }
    }
    for (id, len) in spare {
        if files <= FILES_BUDGET {
            break;
        }
        s.docs.insert(id, Doc::Dropped);
        files -= len;
    }
}

/// Sizes for the memory report (`TDR_MEMORY`).
pub fn stats() -> String {
    with(|s| {
        let frames: usize = s.instances.values().map(Instance::bytes).sum();
        let on_disk = s.instances.values().filter(|i| i.disk.is_some()).count();
        let sources = s.instances.values().filter(|i| i.source.is_some()).count();
        let unpacked: usize = s
            .instances
            .values()
            .filter_map(|i| i.unpacked.as_ref().map(|u| u.1.capacity()))
            .sum();
        let files: usize = s
            .docs
            .values()
            .map(|d| match d {
                Doc::File { bytes, .. } => bytes.len(),
                _ => 0,
            })
            .sum();
        let shown: usize = s.shown.values().map(|v| image_bytes(&v.image)).sum();
        format!(
            "custom emoji: {} instances ({} on disk), in memory {:.1} MB, unpacked {:.1} MB, \
             {} sources, files {:.1} MB ({} docs), shown {} ({:.1} MB), info {}",
            s.instances.len(),
            on_disk,
            frames as f64 / 1048576.,
            unpacked as f64 / 1048576.,
            sources,
            files as f64 / 1048576.,
            s.docs.len(),
            s.shown.len(),
            shown as f64 / 1048576.,
            s.info.len(),
        )
    })
}

fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .filter_map(|k| image.as_bytes(k).map(<[u8]>::len))
        .sum()
}

/// The ids to ask the data thread for (since the last call).
pub fn take_wanted() -> Vec<i64> {
    with(|s| std::mem::take(&mut s.wanted))
}

/// Asks for an emoji's document and file (once).
fn want_in(s: &mut State, id: i64) {
    if let std::collections::hash_map::Entry::Vacant(e) = s.docs.entry(id) {
        e.insert(Doc::Requested);
        s.wanted.push(id);
    }
}

/// The document and file came.
pub fn loaded(doc: &EmojiDocument, bytes: Vec<u8>) {
    with(|s| {
        s.info.insert(doc.id, Info::of(doc));
        s.docs.insert(
            doc.id,
            Doc::File {
                format: doc.format,
                bytes: bytes.into(),
            },
        );
    });
    crate::repaint::after_ms(0);
}

/// The document could not be loaded.
pub fn failed(id: i64) {
    with(|s| {
        s.docs.insert(id, Doc::Failed);
    });
}

/// Known details of an emoji (after its document came).
pub fn info(id: i64) -> Option<Info> {
    with(|s| s.info.get(&id).cloned())
}

/// Remembers a document's details without its file (from a set's list).
pub fn remember(doc: &EmojiDocument) {
    with(|s| {
        s.info.entry(doc.id).or_insert_with(|| Info::of(doc));
    });
}

/// Paints emoji `id` as a `side` device pixels square at `origin` (logical); text
/// colored emoji take `color`. False while it is not ready (it is asked for).
pub fn paint(
    id: i64,
    origin: Point<Pixels>,
    side: u32,
    color: gpui::Hsla,
    window: &mut Window,
) -> bool {
    paint_ex(id, origin, side, color, Loading::Silhouette, window)
}

/// The first frame of `id`, not playing (`AnimatedIcon::frame`: a reaction's
/// icon under a message).
pub fn paint_still(id: i64, origin: Point<Pixels>, side: u32, window: &mut Window) -> bool {
    paint_inner(
        id,
        origin,
        side,
        gpui::Hsla::default(),
        Loading::Silhouette,
        Some(0),
        None,
        window,
    )
}

/// Frame `frame` of `id` rendered at `side` device pixels, drawn into `bounds`
/// (any size: one-shot animations that grow and shrink keep their frames;
/// `AnimatedIcon::frame` drawn scaled). False while it is not ready.
pub fn paint_frame(
    id: i64,
    side: u32,
    frame: usize,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) -> bool {
    paint_inner(
        id,
        bounds.origin,
        side,
        gpui::Hsla::default(),
        Loading::Silhouette,
        Some(frame),
        Some(bounds),
        window,
    )
}

/// The durations of `id`'s frames at `side` once all are rendered (asked for
/// otherwise): one-shot animations play them on their own clock.
pub fn timeline(id: i64, side: u32) -> Option<Vec<u16>> {
    with(|s| {
        want_in(s, id);
        let has_file = matches!(s.docs.get(&id), Some(Doc::File { .. }));
        let key = (id, side);
        let i = s
            .instances
            .entry(key)
            .or_insert_with(|| from_disk(key).unwrap_or_else(|| Instance::new(side)));
        i.painted = Instant::now();
        if i.finished {
            return Some(i.durations.clone());
        }
        if has_file && i.job == Job::Idle {
            queue(s, key);
        } else if matches!(s.docs.get(&id), Some(Doc::Dropped)) {
            s.docs.insert(id, Doc::Requested);
            s.wanted.push(id);
        }
        None
    })
}

/// `paint` with what to draw while it loads.
pub fn paint_ex(
    id: i64,
    origin: Point<Pixels>,
    side: u32,
    color: gpui::Hsla,
    loading: Loading,
    window: &mut Window,
) -> bool {
    paint_inner(id, origin, side, color, loading, None, None, window)
}

/// `frame`: a fixed frame (else the shared playback); `draw`: where to draw it
/// (else `side` device pixels at `origin`).
#[allow(clippy::too_many_arguments)]
fn paint_inner(
    id: i64,
    origin: Point<Pixels>,
    side: u32,
    color: gpui::Hsla,
    loading: Loading,
    fixed: Option<usize>,
    draw: Option<Bounds<Pixels>>,
    window: &mut Window,
) -> bool {
    let still = fixed.is_some();
    let _timer = crate::profile::Timer::new("custom_emoji::paint");
    let paused = crate::repaint::paused();
    let image = with(|s| {
        want_in(s, id);
        let has_file = matches!(s.docs.get(&id), Some(Doc::File { .. }));
        let text_color = s.info.get(&id).is_some_and(|i| i.text_color);
        let counter = s.counter;
        let now = Instant::now();
        let key = (id, side);
        s.paints += 1;
        let order = s.paints;
        let i = s
            .instances
            .entry(key)
            .or_insert_with(|| from_disk(key).unwrap_or_else(|| Instance::new(side)));
        i.painted = now;
        i.seen_frame = counter;
        i.seen_order = order;
        let (job, finished) = (i.job, i.finished);
        // The silhouette until every frame is rendered.
        if !finished {
            if has_file && job == Job::Idle {
                queue(s, key);
            } else if matches!(s.docs.get(&id), Some(Doc::Dropped)) {
                s.docs.insert(id, Doc::Requested);
                s.wanted.push(id);
            }
            return Err(s.info.get(&id).and_then(|i| i.path.clone()));
        }
        if !paused && !still {
            i.advance(now, counter);
        }
        if !still && let Some(due) = i.next_due() {
            crate::repaint::at(due.max(now + Duration::from_millis(8)));
        }
        let frame = match fixed {
            Some(k) => k.min(i.frames().saturating_sub(1)),
            None => i.frame,
        };
        let color_key = if text_color && i.alpha_only {
            let c = gpui::Rgba::from(color);
            let to = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
            (to(c.r) << 24) | (to(c.g) << 16) | (to(c.b) << 8) | 0xff
        } else {
            0
        };
        // Fixed frames keep their own image (the shared playback goes on).
        let shown_key = (id, side, color_key | u32::from(still));
        let image = match s.shown.get_mut(&shown_key) {
            Some(v) if v.frame == frame => {
                v.painted = now;
                v.image.clone()
            }
            _ => {
                let Some(i) = s.instances.get_mut(&key) else {
                    return Err(None);
                };
                let alpha_only = i.alpha_only;
                let Some(pixels) = i.unpack(frame) else {
                    return Err(None);
                };
                let bgra: Vec<u8> = if alpha_only {
                    let (r, g, b) = (
                        (color_key >> 24) as u8,
                        (color_key >> 16) as u8,
                        (color_key >> 8) as u8,
                    );
                    pixels.iter().flat_map(|&a| [b, g, r, a]).collect()
                } else {
                    pixels.to_vec()
                };
                let img = image::RgbaImage::from_raw(side, side, bgra)
                    .unwrap_or_else(|| image::RgbaImage::new(side, side));
                let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                    img
                )]));
                if let Some(old) = s.shown.insert(
                    shown_key,
                    Shown {
                        frame,
                        image: image.clone(),
                        painted: now,
                    },
                ) {
                    s.retired.push(old.image);
                }
                image
            }
        };
        Ok(image)
    });
    let scale = window.scale_factor();
    let x = (f32::from(origin.x) * scale).round() / scale;
    let y = (f32::from(origin.y) * scale).round() / scale;
    let side_l = side as f32 / scale;
    let bounds =
        draw.unwrap_or_else(|| Bounds::new(point(px(x), px(y)), size(px(side_l), px(side_l))));
    match image {
        Ok(image) => {
            let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
            true
        }
        Err(Some(path)) => {
            if let Some(image) = loading_image(id, side, &path, color, loading, x, scale) {
                let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
            }
            false
        }
        Err(None) => false,
    }
}

/// `PathShiftGradient`'s slide and rest.
const SHIMMER_SLIDE_MS: u128 = 1000;
const SHIMMER_FULL_MS: u128 = 2000;

static SHIMMER_START: OnceLock<Instant> = OnceLock::new();

/// The loading silhouette of `id` (cached while it looks the same).
fn loading_image(
    id: i64,
    side: u32,
    path: &[u8],
    color: gpui::Hsla,
    loading: Loading,
    x: f32,
    scale: f32,
) -> Option<Arc<RenderImage>> {
    use resvg::tiny_skia::{
        Color, FillRule, GradientStop, LinearGradient, Paint, Pixmap, Shader, SpreadMode, Transform,
    };
    let to_color = |c: gpui::Rgba, a: f32| {
        Color::from_rgba(c.r, c.g, c.b, a.clamp(0., 1.)).unwrap_or(Color::TRANSPARENT)
    };
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u64;
    let now = SHIMMER_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis();
    // What the image depends on: the colors and the band's place.
    let (look, shader): (u64, Shader) = match loading {
        Loading::Silhouette => {
            let c = gpui::Rgba::from(color);
            // `PreviewColorFromTextColor`: an eighth of the alpha.
            let a = ((c.a * 255. + 1.) / 8.).floor() / 255.;
            let look = byte(c.r) << 24 | byte(c.g) << 16 | byte(c.b) << 8 | byte(a);
            (look, Shader::SolidColor(to_color(c, a)))
        }
        Loading::Shimmer {
            left,
            width,
            bg,
            fg,
        } => {
            let period = now % SHIMMER_FULL_MS;
            let colors = byte(bg.r) << 40 | byte(bg.g) << 32 | byte(fg.r) << 24;
            if period >= SHIMMER_SLIDE_MS {
                crate::repaint::after_ms((SHIMMER_FULL_MS - period) as u64);
                (colors, Shader::SolidColor(to_color(bg, bg.a)))
            } else {
                crate::repaint::after_ms(16);
                let progress = period as f32 / SHIMMER_SLIDE_MS as f32;
                let gw = width / 2.;
                let start = (left - gw) + (width + gw) * progress;
                let sx = ((start - x) * scale).round();
                let shader = LinearGradient::new(
                    resvg::tiny_skia::Point::from_xy(sx, 0.),
                    resvg::tiny_skia::Point::from_xy(sx + gw * scale, 0.),
                    vec![
                        GradientStop::new(0., to_color(bg, bg.a)),
                        GradientStop::new(0.5, to_color(fg, fg.a)),
                        GradientStop::new(1., to_color(bg, bg.a)),
                    ],
                    SpreadMode::Pad,
                    Transform::identity(),
                )
                .unwrap_or(Shader::SolidColor(to_color(bg, bg.a)));
                (colors | 1 << 63 | ((sx as i64 as u64) & 0xFFFF), shader)
            }
        }
    };
    let look = look as usize;
    let now_i = Instant::now();
    let cached = with(|s| {
        let v = s.loading.get_mut(&(id, side))?;
        v.painted = now_i;
        (v.frame == look).then(|| v.image.clone())
    });
    if cached.is_some() {
        return cached;
    }
    let path = crate::vector_thumb::parse(path)?;
    let mut pixmap = Pixmap::new(side.max(1), side.max(1))?;
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    let k = side as f32 / 512.;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::EvenOdd,
        Transform::from_scale(k, k),
        None,
    );
    let image = crate::vector::to_image(&pixmap);
    with(|s| {
        if let Some(old) = s.loading.insert(
            (id, side),
            Shown {
                frame: look,
                image: image.clone(),
                painted: now_i,
            },
        ) {
            s.retired.push(old.image);
        }
    });
    Some(image)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_pack_and_unpack() {
        let side = 5u32;
        let mut i = Instance::new(side);
        let frames: Vec<Vec<u8>> = (0..20u8)
            .map(|k| {
                (0..side * side * 4)
                    .map(|p| {
                        if p % 7 == 0 {
                            k.wrapping_mul(p as u8)
                        } else {
                            0
                        }
                    })
                    .collect()
            })
            .collect();
        for f in &frames {
            i.push(f, 16);
        }
        i.unpacked = None;
        for k in [0usize, 1, 2, 9, 3, 19, 18, 8, 7] {
            assert_eq!(i.unpack(k).unwrap(), &frames[k][..], "frame {k}");
        }
    }

    #[test]
    fn plays_from_the_first_frame_one_step_a_paint() {
        let mut i = Instance::new(1);
        for _ in 0..3 {
            i.push(&[0, 0, 0, 0], 10);
        }
        let t = Instant::now();
        i.advance(t, 1);
        assert_eq!(i.frame, 0);
        i.advance(t + Duration::from_millis(50), 2);
        assert_eq!(i.frame, 1);
        i.advance(t + Duration::from_millis(100), 3);
        assert_eq!(i.frame, 2);
        // Not finished: waits for the next frame.
        i.advance(t + Duration::from_millis(150), 4);
        assert_eq!(i.frame, 2);
        i.finished = true;
        i.advance(t + Duration::from_millis(200), 5);
        assert_eq!(i.frame, 0);
    }
}
