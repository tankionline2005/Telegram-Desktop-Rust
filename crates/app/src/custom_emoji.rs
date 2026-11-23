//! Custom emoji in the UI (tdesktop's `Data::CustomEmojiManager` with
//! `Ui::CustomEmoji::Instance`): asked for by id wherever they are painted, their
//! documents and files come from the data thread (`Update::CustomEmoji`), and each
//! (emoji, size) is one instance shared by every place that paints it, as there.
//!
//! Like tdesktop's `Renderer`, an instance shows its first frame as soon as it is
//! rendered and plays from it at the animation's own rate (one frame per paint at
//! most, `Cache::paintCurrentFrame`), while workers render ahead of the frame on
//! screen (`kPreloadFrames`) until all frames (at most `kMaxFrames`) are kept.
//! Differences that make it faster and lighter:
//! - the workers serve the emoji painted last first (a stack, not a queue) and
//!   skip the ones scrolled away, so the visible ones never wait behind them;
//! - kept frames are compressed in memory (each frame XOR the previous one, zero
//!   runs, decoded in microseconds; text colored emoji keep only alpha): about
//!   2.5 times less than tdesktop's uncompressed frames (deflate gains little
//!   more on these frames and unpacks several times slower), so scrolling back
//!   shows them at once instead of rendering them again; only the frame on
//!   screen is a GPU image.

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
    /// The vector thumbnail (`photoPathSize`), drawn while loading.
    pub path: Option<Arc<resvg::tiny_skia::Path>>,
}

impl Info {
    fn of(doc: &EmojiDocument) -> Self {
        Self {
            alt: doc.alt.clone(),
            set: doc.set,
            text_color: doc.text_color,
            free: doc.free,
            path: doc
                .path
                .as_deref()
                .and_then(crate::vector_thumb::parse)
                .map(Arc::new),
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
    Failed,
}

/// `kMaxFrames`: longer animations are cut (and then loop).
const MAX_FRAMES: usize = 180;
/// `kPreloadFrames`, a bit more: frames rendered ahead of the one on screen.
const PRELOAD_FRAMES: usize = 8;
/// A frame is stored whole every this many frames (the rest as differences).
const KEY_EVERY: usize = 8;
/// Compressed frames kept for emoji off screen, in bytes.
const BUDGET: usize = 48 << 20;
/// Files kept in memory for emoji without frames (they come back from the
/// file cache when painted again).
const FILES_BUDGET: usize = 8 << 20;
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
    /// Frames packed one after another (`pack`), `offsets[k]..offsets[k + 1]`.
    data: Vec<u8>,
    offsets: Vec<usize>,
    durations: Vec<u16>,
    finished: bool,
    /// Text colored emoji keep only alpha (1 byte a pixel), others BGRA.
    alpha_only: bool,
    side: u32,
    /// The frame on screen and when it was shown.
    frame: usize,
    shown_at: Option<Instant>,
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
            offsets: vec![0],
            durations: Vec::new(),
            finished: false,
            alpha_only: false,
            side,
            frame: 0,
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

    fn bytes(&self) -> usize {
        self.data.capacity() + self.offsets.capacity() * 8 + self.durations.capacity() * 2
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
            let packed = &self.data[self.offsets[from]..self.offsets[from + 1]];
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
    /// New instances (served last in, first out) and ones that need more frames.
    fresh: Vec<Key>,
    more: std::collections::VecDeque<Key>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap();
    f(guard.get_or_insert_with(State::default))
}

fn queue(s: &mut State, key: Key, fresh: bool) {
    let Some(i) = s.instances.get_mut(&key) else {
        return;
    };
    if i.job != Job::Idle {
        return;
    }
    i.job = Job::Queued;
    if fresh {
        s.fresh.push(key);
    } else {
        s.more.push_back(key);
    }
    start_workers();
    WAKE.notify_one();
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
    loop {
        let key = s.fresh.pop().or_else(|| s.more.pop_front())?;
        let Some(i) = s.instances.get_mut(&key) else {
            continue;
        };
        if i.job != Job::Queued {
            continue;
        }
        // Scrolled away: left for when it is painted again.
        if now.duration_since(i.painted) > IDLE {
            i.job = Job::Idle;
            continue;
        }
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
        return Some(Task { file, ..task });
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
                }
            });
        }
        (None, None) => return finish_job(key, |_| {}),
    };
    let Source::Lottie(anim) = &*source;
    let count = anim.frame_count().min(MAX_FRAMES);
    let step = (1000. / anim.frame_rate.max(1.)).round().max(1.) as u32;
    let Some(mut pixmap) = tdr_lottie::tiny_skia::Pixmap::new(side, side) else {
        return fail(key);
    };
    let mut k = task.from;
    let mut raw = Vec::with_capacity((side * side * 4) as usize);
    while k < count {
        pixmap.fill(tdr_lottie::tiny_skia::Color::TRANSPARENT);
        anim.render_into(k, &mut pixmap);
        raw.clear();
        for p in pixmap.pixels() {
            let c = p.demultiply();
            if task.alpha_only {
                raw.push(c.alpha());
            } else {
                raw.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
            }
        }
        k += 1;
        // Stored at once (the first frame shows as soon as it is ready); goes on
        // while the emoji is on screen and not far enough ahead.
        let go_on = with(|s| {
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
                return false;
            }
            let ahead = i.frames() > i.frame + PRELOAD_FRAMES;
            let first = i.frames() == 1;
            !first && !ahead && i.painted.elapsed() <= IDLE
        });
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
    for (key, i) in s.instances.iter_mut() {
        let bytes = i.bytes();
        total += bytes;
        if now.duration_since(i.painted) > SHOWN_KEEP && i.job == Job::Idle {
            i.source = None;
            i.unpacked = None;
            off.push((i.painted, *key, bytes));
        }
    }
    off.sort_by_key(|o| o.0);
    for (_, key, bytes) in off {
        if total <= BUDGET {
            break;
        }
        s.instances.remove(&key);
        total -= bytes;
    }
    let with_frames: std::collections::HashSet<i64> = s.instances.keys().map(|k| k.0).collect();
    let mut files = 0;
    let mut spare = Vec::new();
    for (id, doc) in &s.docs {
        if let Doc::File { bytes, .. } = doc {
            files += bytes.len();
            if !with_frames.contains(id) {
                spare.push((*id, bytes.len()));
            }
        }
    }
    for (id, len) in spare {
        if files <= FILES_BUDGET {
            break;
        }
        s.docs.remove(&id);
        files -= len;
    }
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

/// `paint` with what to draw while it loads.
pub fn paint_ex(
    id: i64,
    origin: Point<Pixels>,
    side: u32,
    color: gpui::Hsla,
    loading: Loading,
    window: &mut Window,
) -> bool {
    let _timer = crate::profile::Timer::new("custom_emoji::paint");
    let paused = crate::repaint::paused();
    let image = with(|s| {
        want_in(s, id);
        let has_file = matches!(s.docs.get(&id), Some(Doc::File { .. }));
        let text_color = s.info.get(&id).is_some_and(|i| i.text_color);
        let counter = s.counter;
        let now = Instant::now();
        let key = (id, side);
        let i = s
            .instances
            .entry(key)
            .or_insert_with(|| Instance::new(side));
        i.painted = now;
        let (frames, job, finished) = (i.frames(), i.job, i.finished);
        if frames == 0 {
            if has_file && job == Job::Idle {
                queue(s, key, true);
            }
            return Err(s.info.get(&id).and_then(|i| i.path.clone()));
        }
        if !paused {
            i.advance(now, counter);
        }
        if let Some(due) = i.next_due() {
            crate::repaint::at(due.max(now + Duration::from_millis(8)));
        }
        let frame = i.frame;
        let more = !finished && i.frames() < frame + PRELOAD_FRAMES && job == Job::Idle;
        let color_key = if text_color && i.alpha_only {
            let c = gpui::Rgba::from(color);
            let to = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
            (to(c.r) << 24) | (to(c.g) << 16) | (to(c.b) << 8) | 0xff
        } else {
            0
        };
        let shown_key = (id, side, color_key);
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
        if more {
            queue(s, key, false);
        }
        Ok(image)
    });
    let scale = window.scale_factor();
    let x = (f32::from(origin.x) * scale).round() / scale;
    let y = (f32::from(origin.y) * scale).round() / scale;
    let side_l = side as f32 / scale;
    let bounds = Bounds::new(point(px(x), px(y)), size(px(side_l), px(side_l)));
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
    path: &resvg::tiny_skia::Path,
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
    let mut pixmap = Pixmap::new(side.max(1), side.max(1))?;
    let paint = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    let k = side as f32 / 512.;
    pixmap.fill_path(
        path,
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
