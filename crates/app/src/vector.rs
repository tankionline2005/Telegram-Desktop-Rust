//! Small vector drawings (strokes) painted as images rasterized on the CPU.
//!
//! GPUI 0.2.2 draws every batch of paths through a window-sized 4x MSAA texture
//! that is cleared and resolved each time; paths interleaved with other primitives
//! become one such batch each. On a weak GPU a few of them made every frame cost
//! tens of milliseconds, so lines, arcs and the like are drawn with tiny-skia into
//! a small image instead (cached while painted, dropped from the atlas after).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};

use gpui::{Bounds, Corners, RenderImage, Rgba, Window, point, px, size};
use resvg::tiny_skia::{
    self, LineCap as SkCap, LineJoin as SkJoin, Paint, PathBuilder, Pixmap, Stroke as SkStroke,
    Transform,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cap {
    Butt,
    Round,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Join {
    Miter,
    Round,
}

/// A stroke's width (logical pixels), caps and joins.
#[derive(Clone, Copy)]
pub struct Stroke {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
}

impl Stroke {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            cap: Cap::Butt,
            join: Join::Miter,
        }
    }

    pub fn cap(mut self, cap: Cap) -> Self {
        self.cap = cap;
        self
    }

    pub fn join(mut self, join: Join) -> Self {
        self.join = join;
        self
    }
}

struct Cached {
    image: Arc<RenderImage>,
    painted: u64,
}

#[derive(Default)]
struct State {
    images: HashMap<u64, Cached>,
    counter: u64,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Images not painted for this many frames leave the atlas (animated drawings
/// make a new image each frame).
const KEEP_UNUSED: u64 = 30;

/// Called once per frame of the main window, before painting.
pub fn begin_frame(window: &mut Window) {
    let dropped: Vec<Arc<RenderImage>> = {
        let mut s = STATE.lock().unwrap();
        s.counter += 1;
        let counter = s.counter;
        let stale: Vec<u64> = s
            .images
            .iter()
            .filter(|(_, v)| counter - v.painted > KEEP_UNUSED)
            .map(|(k, _)| *k)
            .collect();
        stale
            .into_iter()
            .filter_map(|k| s.images.remove(&k).map(|v| v.image))
            .collect()
    };
    for image in dropped {
        let _ = window.drop_image(image);
    }
}

/// Strokes the polylines `lines` (logical window coordinates) with `stroke` in
/// `color`, as one image covering their bounds.
pub fn paint_strokes(lines: &[&[(f32, f32)]], stroke: Stroke, color: Rgba, window: &mut Window) {
    let scale = window.scale_factor();
    let points = lines.iter().flat_map(|l| l.iter());
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x0 > x1 || color.a <= 0. {
        return;
    }
    // The image's device rectangle: the bounds with the stroke and a pixel around.
    let margin = stroke.width * scale / 2. + 2.;
    let left = (x0 * scale - margin).floor();
    let top = (y0 * scale - margin).floor();
    let w = ((x1 * scale + margin).ceil() - left).max(1.) as u32;
    let h = ((y1 * scale + margin).ceil() - top).max(1.) as u32;
    if w > 4096 || h > 4096 {
        return;
    }
    let local: Vec<Vec<(f32, f32)>> = lines
        .iter()
        .map(|l| {
            l.iter()
                .map(|&(x, y)| (x * scale - left, y * scale - top))
                .collect()
        })
        .collect();
    let to = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    let rgba = [to(color.r), to(color.g), to(color.b), to(color.a)];

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (w, h, rgba, stroke.cap, stroke.join).hash(&mut hasher);
    ((stroke.width * scale * 64.).round() as i32).hash(&mut hasher);
    for line in &local {
        line.len().hash(&mut hasher);
        for &(x, y) in line {
            ((x * 64.).round() as i32, (y * 64.).round() as i32).hash(&mut hasher);
        }
    }
    let key = hasher.finish();

    let image = {
        let mut s = STATE.lock().unwrap();
        let counter = s.counter;
        if let Some(v) = s.images.get_mut(&key) {
            v.painted = counter;
            Some(v.image.clone())
        } else {
            None
        }
    };
    let image = match image {
        Some(image) => image,
        None => {
            let Some(image) = rasterize(&local, stroke.width * scale, stroke, rgba, w, h) else {
                return;
            };
            let mut s = STATE.lock().unwrap();
            let counter = s.counter;
            s.images.insert(
                key,
                Cached {
                    image: image.clone(),
                    painted: counter,
                },
            );
            image
        }
    };
    let bounds = Bounds::new(
        point(px(left / scale), px(top / scale)),
        size(px(w as f32 / scale), px(h as f32 / scale)),
    );
    let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
}

fn rasterize(
    lines: &[Vec<(f32, f32)>],
    width: f32,
    stroke: Stroke,
    [r, g, b, a]: [u8; 4],
    w: u32,
    h: u32,
) -> Option<Arc<RenderImage>> {
    let mut pixmap = Pixmap::new(w, h)?;
    let mut pb = PathBuilder::new();
    for line in lines {
        for (k, &(x, y)) in line.iter().enumerate() {
            if k == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
    }
    let path = pb.finish()?;
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(r, g, b, a);
    let sk = SkStroke {
        width,
        line_cap: match stroke.cap {
            Cap::Butt => SkCap::Butt,
            Cap::Round => SkCap::Round,
        },
        line_join: match stroke.join {
            Join::Miter => SkJoin::Miter,
            Join::Round => SkJoin::Round,
        },
        ..SkStroke::default()
    };
    pixmap.stroke_path(&path, &paint, &sk, Transform::identity(), None);
    Some(to_image(&pixmap))
}

/// A tiny-skia pixmap as a GPUI image (straight BGRA).
pub fn to_image(pixmap: &Pixmap) -> Arc<RenderImage> {
    let mut data = Vec::with_capacity(pixmap.data().len());
    for p in pixmap.pixels() {
        let c: tiny_skia::ColorU8 = p.demultiply();
        data.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
    }
    let out = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), data)
        .unwrap_or_else(|| image::RgbaImage::new(pixmap.width(), pixmap.height()));
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        out
    )]))
}
