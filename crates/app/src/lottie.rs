//! A small Lottie player for tdesktop's toast icons (`animations/toast/*.tgs`, gzipped
//! Lottie JSON): shape layers with filled paths, null layers as parents, keyframed
//! transforms and paths with bezier easing. That is all those icons use; tdesktop
//! plays them with rlottie. Frames are rendered once with tiny-skia.

use std::io::Read;

use resvg::tiny_skia::{self, FillRule, Paint, PathBuilder, Pixmap, Transform};
use serde_json::Value;

type Vec2 = (f64, f64);

trait Lerp: Clone {
    fn lerp(&self, to: &Self, t: f64) -> Self;
}

impl Lerp for f64 {
    fn lerp(&self, to: &Self, t: f64) -> Self {
        self + (to - self) * t
    }
}

impl Lerp for Vec2 {
    fn lerp(&self, to: &Self, t: f64) -> Self {
        (self.0.lerp(&to.0, t), self.1.lerp(&to.1, t))
    }
}

#[derive(Clone)]
struct Bezier {
    closed: bool,
    v: Vec<Vec2>,
    i: Vec<Vec2>,
    o: Vec<Vec2>,
}

impl Lerp for Bezier {
    fn lerp(&self, to: &Self, t: f64) -> Self {
        let mix = |a: &[Vec2], b: &[Vec2]| -> Vec<Vec2> {
            a.iter().zip(b).map(|(a, b)| a.lerp(b, t)).collect()
        };
        Bezier {
            closed: self.closed,
            v: mix(&self.v, &to.v),
            i: mix(&self.i, &to.i),
            o: mix(&self.o, &to.o),
        }
    }
}

struct Keyframe<T> {
    time: f64,
    value: T,
    /// Easing toward the next keyframe: out and in control points.
    out: Vec2,
    into: Vec2,
    hold: bool,
}

enum Prop<T> {
    Static(T),
    Animated(Vec<Keyframe<T>>),
}

/// The bezier easing curve (0,0) (o) (i) (1,1): its y where its x is `x`.
fn ease(out: Vec2, into: Vec2, x: f64) -> f64 {
    let curve = |a: f64, b: f64, t: f64| {
        let mt = 1. - t;
        3. * mt * mt * t * a + 3. * mt * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0., 1.);
    let mut t = x;
    for _ in 0..40 {
        let cx = curve(out.0, into.0, t);
        if (cx - x).abs() < 1e-6 {
            break;
        }
        if cx < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.;
    }
    curve(out.1, into.1, t)
}

impl<T: Lerp> Prop<T> {
    fn at(&self, frame: f64) -> T {
        match self {
            Prop::Static(v) => v.clone(),
            Prop::Animated(keys) => {
                let first = &keys[0];
                if frame <= first.time {
                    return first.value.clone();
                }
                for pair in keys.windows(2) {
                    let (a, b) = (&pair[0], &pair[1]);
                    if frame < b.time {
                        if a.hold || b.time <= a.time {
                            return a.value.clone();
                        }
                        let x = (frame - a.time) / (b.time - a.time);
                        return a.value.lerp(&b.value, ease(a.out, a.into, x));
                    }
                }
                keys.last().unwrap().value.clone()
            }
        }
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Array(a) => a.first().map_or(0., num),
        v => v.as_f64().unwrap_or(0.),
    }
}

fn vec2(v: &Value) -> Vec2 {
    match v {
        Value::Array(a) => (
            a.first().map_or(0., num),
            a.get(1).map_or_else(|| a.first().map_or(0., num), num),
        ),
        v => (num(v), num(v)),
    }
}

fn bezier(v: &Value) -> Bezier {
    let v = match v {
        Value::Array(a) => a.first().unwrap_or(&Value::Null),
        v => v,
    };
    let points = |key: &str| -> Vec<Vec2> {
        v[key]
            .as_array()
            .map(|a| a.iter().map(vec2).collect())
            .unwrap_or_default()
    };
    Bezier {
        closed: v["c"].as_bool().unwrap_or(false),
        v: points("v"),
        i: points("i"),
        o: points("o"),
    }
}

fn prop<T>(v: &Value, parse: fn(&Value) -> T, default: T) -> Prop<T> {
    if v.is_null() {
        return Prop::Static(default);
    }
    let animated = v["a"].as_i64() == Some(1);
    if !animated {
        return Prop::Static(parse(&v["k"]));
    }
    let keys: Vec<Keyframe<T>> = v["k"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|k| !k["s"].is_null())
                .map(|k| Keyframe {
                    time: num(&k["t"]),
                    value: parse(&k["s"]),
                    out: (num(&k["o"]["x"]), num(&k["o"]["y"])),
                    into: (num(&k["i"]["x"]), num(&k["i"]["y"])),
                    hold: k["h"].as_i64() == Some(1),
                })
                .collect()
        })
        .unwrap_or_default();
    if keys.is_empty() {
        Prop::Static(default)
    } else {
        Prop::Animated(keys)
    }
}

struct Transform2d {
    anchor: Prop<Vec2>,
    position: Prop<Vec2>,
    scale: Prop<Vec2>,
    rotation: Prop<f64>,
    opacity: Prop<f64>,
}

impl Transform2d {
    fn parse(v: &Value) -> Self {
        Self {
            anchor: prop(&v["a"], vec2, (0., 0.)),
            position: prop(&v["p"], vec2, (0., 0.)),
            scale: prop(&v["s"], vec2, (100., 100.)),
            rotation: prop(&v["r"], num, 0.),
            opacity: prop(&v["o"], num, 100.),
        }
    }

    fn matrix(&self, frame: f64) -> Transform {
        let (ax, ay) = self.anchor.at(frame);
        let (px, py) = self.position.at(frame);
        let (sx, sy) = self.scale.at(frame);
        let r = self.rotation.at(frame);
        Transform::from_translate(px as f32, py as f32)
            .pre_concat(Transform::from_rotate(r as f32))
            .pre_concat(Transform::from_scale(
                (sx / 100.) as f32,
                (sy / 100.) as f32,
            ))
            .pre_concat(Transform::from_translate(-ax as f32, -ay as f32))
    }
}

enum Shape {
    Group {
        items: Vec<Shape>,
        transform: Option<Transform2d>,
    },
    Path(Prop<Bezier>),
    Fill {
        opacity: Prop<f64>,
    },
}

fn shapes(list: &Value) -> Vec<Shape> {
    let Some(list) = list.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter(|s| s["hd"].as_bool() != Some(true))
        .filter_map(|s| match s["ty"].as_str()? {
            "gr" => {
                let items = &s["it"];
                let transform = items
                    .as_array()
                    .and_then(|a| a.iter().find(|i| i["ty"] == "tr").map(Transform2d::parse));
                Some(Shape::Group {
                    items: shapes(items),
                    transform,
                })
            }
            "sh" => Some(Shape::Path(prop(
                &s["ks"],
                bezier,
                Bezier {
                    closed: false,
                    v: vec![],
                    i: vec![],
                    o: vec![],
                },
            ))),
            "fl" => Some(Shape::Fill {
                opacity: prop(&s["o"], num, 100.),
            }),
            _ => None,
        })
        .collect()
}

struct Layer {
    index: i64,
    parent: Option<i64>,
    shape_layer: bool,
    in_point: f64,
    out_point: f64,
    transform: Transform2d,
    shapes: Vec<Shape>,
}

pub struct Animation {
    width: f64,
    height: f64,
    pub frames: usize,
    layers: Vec<Layer>,
}

impl Animation {
    /// A `.tgs` file (gzipped JSON).
    pub fn from_tgs(bytes: &[u8]) -> Option<Self> {
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut json)
            .ok()?;
        let v: Value = serde_json::from_slice(&json).ok()?;
        let layers = v["layers"]
            .as_array()?
            .iter()
            .map(|l| Layer {
                index: l["ind"].as_i64().unwrap_or(0),
                parent: l["parent"].as_i64(),
                shape_layer: l["ty"].as_i64() == Some(4),
                in_point: num(&l["ip"]),
                out_point: num(&l["op"]),
                transform: Transform2d::parse(&l["ks"]),
                shapes: shapes(&l["shapes"]),
            })
            .collect();
        let ip = num(&v["ip"]);
        let op = num(&v["op"]);
        Some(Self {
            width: num(&v["w"]).max(1.),
            height: num(&v["h"]).max(1.),
            frames: (op - ip).max(1.) as usize,
            layers,
        })
    }

    fn layer_matrix(&self, layer: &Layer, frame: f64, depth: usize) -> Transform {
        let own = layer.transform.matrix(frame);
        match layer.parent.filter(|_| depth < 16) {
            Some(parent) => match self.layers.iter().find(|l| l.index == parent) {
                Some(p) => self.layer_matrix(p, frame, depth + 1).pre_concat(own),
                None => own,
            },
            None => own,
        }
    }

    /// Coverage of frame `frame` at `size` x `size` pixels.
    pub fn render(&self, frame: f64, size: u32) -> Vec<u8> {
        let Some(mut pixmap) = Pixmap::new(size, size) else {
            return Vec::new();
        };
        let base = Transform::from_scale(
            (size as f64 / self.width) as f32,
            (size as f64 / self.height) as f32,
        );
        // The first layer is on top.
        for layer in self.layers.iter().rev() {
            if !layer.shape_layer || frame < layer.in_point || frame >= layer.out_point {
                continue;
            }
            let m = base.pre_concat(self.layer_matrix(layer, frame, 0));
            let opacity = layer.transform.opacity.at(frame) / 100.;
            paint(&layer.shapes, m, opacity, frame, &mut pixmap);
        }
        pixmap.pixels().iter().map(|p| p.alpha()).collect()
    }
}

fn paint(items: &[Shape], m: Transform, opacity: f64, frame: f64, pixmap: &mut Pixmap) {
    let mut paths = Vec::new();
    for item in items {
        match item {
            Shape::Group { items, transform } => {
                let (gm, go) = match transform {
                    Some(t) => (m.pre_concat(t.matrix(frame)), t.opacity.at(frame) / 100.),
                    None => (m, 1.),
                };
                paint(items, gm, opacity * go, frame, pixmap);
            }
            Shape::Path(p) => {
                if let Some(path) = build(&p.at(frame)) {
                    paths.push(path);
                }
            }
            Shape::Fill { opacity: o } => {
                let alpha = (opacity * o.at(frame) / 100.).clamp(0., 1.);
                let mut paint = Paint::default();
                paint.set_color(tiny_skia::Color::from_rgba(1., 1., 1., alpha as f32).unwrap());
                paint.anti_alias = true;
                for path in &paths {
                    pixmap.fill_path(path, &paint, FillRule::Winding, m, None);
                }
            }
        }
    }
}

fn build(b: &Bezier) -> Option<tiny_skia::Path> {
    let n = b.v.len();
    if n == 0 || b.i.len() != n || b.o.len() != n {
        return None;
    }
    let mut pb = PathBuilder::new();
    let p = |v: Vec2| (v.0 as f32, v.1 as f32);
    let (x, y) = p(b.v[0]);
    pb.move_to(x, y);
    let segment = |pb: &mut PathBuilder, a: usize, z: usize| {
        let c1 = p((b.v[a].0 + b.o[a].0, b.v[a].1 + b.o[a].1));
        let c2 = p((b.v[z].0 + b.i[z].0, b.v[z].1 + b.i[z].1));
        let end = p(b.v[z]);
        pb.cubic_to(c1.0, c1.1, c2.0, c2.1, end.0, end.1);
    };
    for k in 0..n - 1 {
        segment(&mut pb, k, k + 1);
    }
    if b.closed {
        segment(&mut pb, n - 1, 0);
        pb.close();
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_icon_animates_and_settles() {
        let a = Animation::from_tgs(include_bytes!(
            "../../../assets/tdesktop-animations/toast__copy.tgs"
        ))
        .unwrap();
        assert_eq!(a.frames, 61);
        let covered = |frame: f64| a.render(frame, 64).iter().filter(|c| **c > 128).count();
        // The two sheets grow in: the first frame is (almost) empty, the last full.
        assert!(covered(0.) < covered(60.));
        assert!(covered(60.) > 400, "{}", covered(60.));
        assert_eq!(a.render(60., 64), a.render(70., 64));
    }

    #[test]
    fn easing_ends() {
        assert!((ease((0.4, 0.), (0.2, 1.), 0.) - 0.).abs() < 1e-6);
        assert!((ease((0.4, 0.), (0.2, 1.), 1.) - 1.).abs() < 1e-6);
        assert!(ease((0.4, 0.), (0.2, 1.), 0.5) > 0.5);
    }
}
