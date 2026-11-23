//! Lottie animations as Telegram uses them (`.tgs`: gzipped Lottie JSON, the subset
//! of the "Telegram animated stickers" rules) drawn on the CPU with tiny-skia, the
//! way tdesktop draws them with rlottie: shape, null, solid and precomposition
//! layers with parents, masks and track mattes; groups, paths, rectangles,
//! ellipses, stars and polygons; solid and gradient fills and strokes (with
//! dashes); trim paths; keyframes with bezier easing, hold keys and spatial
//! tangents; time stretch and time remapping.
//!
//! Drawing order follows rlottie: a fill or a stroke paints the paths listed above
//! it in its group (nested groups included), items higher in the list are drawn on
//! top, and a layer's opacity multiplies its paints (no offscreen layer, except for
//! masks and mattes).

use std::collections::HashMap;
use std::io::Read;

use serde_json::Value;
pub use tiny_skia;
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Mask, MaskType, Paint, PathBuilder,
    Pixmap, Point, RadialGradient, Shader, SpreadMode, Stroke, StrokeDash, Transform,
};

#[derive(Debug)]
pub enum Error {
    Gzip,
    Json,
    Format,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Gzip => "not gzip",
            Error::Json => "bad json",
            Error::Format => "not a lottie animation",
        })
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------- properties ----

/// The easing between two keys of one dimension: the out tangent of the first and
/// the in tangent of the second (a CSS-like cubic bezier from (0,0) to (1,1)).
#[derive(Clone, Copy, Debug)]
struct Ease {
    ox: f64,
    oy: f64,
    ix: f64,
    iy: f64,
}

impl Ease {
    const LINEAR: Ease = Ease {
        ox: 0.,
        oy: 0.,
        ix: 1.,
        iy: 1.,
    };

    fn at(&self, t: f64) -> f64 {
        if t <= 0. {
            return 0.;
        }
        if t >= 1. {
            return 1.;
        }
        let (x1, y1, x2, y2) = (self.ox, self.oy, self.ix, self.iy);
        if (x1 - y1).abs() < 1e-9 && (x2 - y2).abs() < 1e-9 {
            return t;
        }
        let bez = |a: f64, b: f64, u: f64| {
            let v = 1. - u;
            3. * v * v * u * a + 3. * v * u * u * b + u * u * u
        };
        // Find u with x(u) = t (bisection, x is monotonic for x1, x2 in [0, 1]).
        let (mut lo, mut hi) = (0., 1.);
        let mut u = t;
        for _ in 0..40 {
            let x = bez(x1, x2, u);
            if (x - t).abs() < 1e-7 {
                break;
            }
            if x < t {
                lo = u;
            } else {
                hi = u;
            }
            u = (lo + hi) / 2.;
        }
        bez(y1, y2, u)
    }
}

#[derive(Clone, Debug, Default)]
struct Bezier {
    closed: bool,
    v: Vec<[f64; 2]>,
    i: Vec<[f64; 2]>,
    o: Vec<[f64; 2]>,
}

trait Animatable: Clone {
    fn mix(a: &Self, b: &Self, t: &dyn Fn(usize) -> f64) -> Self;
}

impl Animatable for Vec<f64> {
    fn mix(a: &Self, b: &Self, t: &dyn Fn(usize) -> f64) -> Self {
        a.iter()
            .enumerate()
            .map(|(k, x)| {
                let y = b.get(k).copied().unwrap_or(*x);
                x + (y - x) * t(k)
            })
            .collect()
    }
}

impl Animatable for Bezier {
    fn mix(a: &Self, b: &Self, t: &dyn Fn(usize) -> f64) -> Self {
        let t = t(0);
        let lerp = |p: &[[f64; 2]], q: &[[f64; 2]]| -> Vec<[f64; 2]> {
            p.iter()
                .zip(q.iter().chain(std::iter::repeat(&[0., 0.])))
                .map(|(p, q)| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t])
                .collect()
        };
        if a.v.len() != b.v.len() {
            return if t < 1. { a.clone() } else { b.clone() };
        }
        Bezier {
            closed: a.closed,
            v: lerp(&a.v, &b.v),
            i: lerp(&a.i, &b.i),
            o: lerp(&a.o, &b.o),
        }
    }
}

#[derive(Clone, Debug)]
struct Key<T> {
    t: f64,
    s: T,
    e: Option<T>,
    hold: bool,
    ease: Vec<Ease>,
    /// Spatial tangents of a position key (`to`, `ti`).
    to: Option<[f64; 2]>,
    ti: Option<[f64; 2]>,
}

#[derive(Clone, Debug)]
enum Prop<T> {
    Static(T),
    Animated(Vec<Key<T>>),
}

impl<T: Animatable> Prop<T> {
    fn at(&self, frame: f64) -> T {
        match self {
            Prop::Static(v) => v.clone(),
            Prop::Animated(keys) => {
                if keys.is_empty() {
                    unreachable!()
                }
                if frame <= keys[0].t || keys.len() == 1 {
                    return keys[0].s.clone();
                }
                let k = keys.partition_point(|k| k.t <= frame).saturating_sub(1);
                if k + 1 >= keys.len() {
                    let last = &keys[keys.len() - 1];
                    return last.s.clone();
                }
                let (a, b) = (&keys[k], &keys[k + 1]);
                if a.hold {
                    return a.s.clone();
                }
                let span = (b.t - a.t).max(1e-9);
                let p = ((frame - a.t) / span).clamp(0., 1.);
                let end = a.e.as_ref().unwrap_or(&b.s);
                let ease = &a.ease;
                T::mix(&a.s, end, &|dim| {
                    let e = ease
                        .get(dim)
                        .or(ease.first())
                        .copied()
                        .unwrap_or(Ease::LINEAR);
                    e.at(p)
                })
            }
        }
    }
}

impl Prop<Vec<f64>> {
    /// A position with spatial tangents moves along the curve between its keys.
    fn at_spatial(&self, frame: f64) -> Vec<f64> {
        let Prop::Animated(keys) = self else {
            return self.at(frame);
        };
        if keys.len() < 2 || frame <= keys[0].t {
            return self.at(frame);
        }
        let k = keys.partition_point(|k| k.t <= frame).saturating_sub(1);
        if k + 1 >= keys.len() || keys[k].hold {
            return self.at(frame);
        }
        let (a, b) = (&keys[k], &keys[k + 1]);
        let (Some(to), Some(ti)) = (a.to, a.ti) else {
            return self.at(frame);
        };
        if to == [0., 0.] && ti == [0., 0.] {
            return self.at(frame);
        }
        let end = a.e.as_ref().unwrap_or(&b.s);
        if a.s.len() < 2 || end.len() < 2 {
            return self.at(frame);
        }
        let p0 = [a.s[0], a.s[1]];
        let p3 = [end[0], end[1]];
        let p1 = [p0[0] + to[0], p0[1] + to[1]];
        let p2 = [p3[0] + ti[0], p3[1] + ti[1]];
        let span = (b.t - a.t).max(1e-9);
        let p = a
            .ease
            .first()
            .copied()
            .unwrap_or(Ease::LINEAR)
            .at(((frame - a.t) / span).clamp(0., 1.));
        // Along the curve by its length.
        const N: usize = 40;
        let point = |u: f64| -> [f64; 2] {
            let v = 1. - u;
            let w = [v * v * v, 3. * v * v * u, 3. * v * u * u, u * u * u];
            [
                w[0] * p0[0] + w[1] * p1[0] + w[2] * p2[0] + w[3] * p3[0],
                w[0] * p0[1] + w[1] * p1[1] + w[2] * p2[1] + w[3] * p3[1],
            ]
        };
        let mut lengths = [0f64; N + 1];
        let mut prev = p0;
        for (i, l) in lengths.iter_mut().enumerate().skip(1) {
            let q = point(i as f64 / N as f64);
            *l = ((q[0] - prev[0]).powi(2) + (q[1] - prev[1]).powi(2)).sqrt();
            prev = q;
        }
        for i in 1..=N {
            lengths[i] += lengths[i - 1];
        }
        let target = lengths[N] * p;
        let i = lengths.partition_point(|l| *l < target).clamp(1, N);
        let seg = (lengths[i] - lengths[i - 1]).max(1e-9);
        let u = ((i - 1) as f64 + (target - lengths[i - 1]) / seg) / N as f64;
        let q = point(u);
        let mut out = vec![q[0], q[1]];
        if a.s.len() > 2 {
            out.push(a.s[2]);
        }
        out
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Array(a) => a.first().map_or(0., num),
        v => v.as_f64().unwrap_or(0.),
    }
}

fn nums(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().map(num).collect(),
        v => vec![num(v)],
    }
}

fn point2(v: &Value) -> [f64; 2] {
    let n = nums(v);
    [
        n.first().copied().unwrap_or(0.),
        n.get(1).copied().unwrap_or(0.),
    ]
}

fn ease_list(o: &Value, i: &Value) -> Vec<Ease> {
    let ox = nums(&o["x"]);
    let oy = nums(&o["y"]);
    let ix = nums(&i["x"]);
    let iy = nums(&i["y"]);
    let dims = ox.len().max(oy.len()).max(ix.len()).max(iy.len()).max(1);
    let get = |v: &Vec<f64>, k: usize, d: f64| v.get(k).or(v.first()).copied().unwrap_or(d);
    (0..dims)
        .map(|k| Ease {
            ox: get(&ox, k, 0.),
            oy: get(&oy, k, 0.),
            ix: get(&ix, k, 1.),
            iy: get(&iy, k, 1.),
        })
        .collect()
}

fn is_keyframes(k: &Value) -> bool {
    k.as_array()
        .and_then(|a| a.first())
        .is_some_and(|f| f.is_object() && f.get("t").is_some())
}

fn parse_prop<T: Clone>(v: &Value, value: &dyn Fn(&Value) -> Option<T>, default: T) -> Prop<T> {
    let k = &v["k"];
    if !is_keyframes(k) {
        return Prop::Static(value(k).unwrap_or(default));
    }
    let arr = k.as_array().unwrap();
    let mut keys: Vec<Key<T>> = Vec::with_capacity(arr.len());
    for (n, f) in arr.iter().enumerate() {
        let s = f.get("s").and_then(value);
        let e = f.get("e").and_then(value);
        let s = match s {
            Some(s) => s,
            // Old files: the last key has no value, the previous key's end.
            None => match keys.last() {
                Some(prev) => prev.e.clone().unwrap_or_else(|| prev.s.clone()),
                None => default.clone(),
            },
        };
        let _ = n;
        keys.push(Key {
            t: num(&f["t"]),
            s,
            e,
            hold: f["h"].as_i64() == Some(1) || f["h"].as_bool() == Some(true),
            ease: if f.get("o").is_some() {
                ease_list(&f["o"], &f["i"])
            } else {
                vec![Ease::LINEAR]
            },
            to: f.get("to").filter(|v| v.is_array()).map(point2),
            ti: f.get("ti").filter(|v| v.is_array()).map(point2),
        });
    }
    if keys.is_empty() {
        return Prop::Static(default);
    }
    Prop::Animated(keys)
}

fn numbers(v: &Value, default: Vec<f64>) -> Prop<Vec<f64>> {
    if v.is_null() {
        return Prop::Static(default);
    }
    parse_prop(v, &|k| Some(nums(k)), default)
}

fn scalar(v: &Value, default: f64) -> Prop<Vec<f64>> {
    numbers(v, vec![default])
}

fn bezier_of(v: &Value) -> Option<Bezier> {
    let v = match v {
        Value::Array(a) => a.first()?,
        v => v,
    };
    let pts = |key: &str| -> Vec<[f64; 2]> {
        v[key]
            .as_array()
            .map(|a| a.iter().map(point2).collect())
            .unwrap_or_default()
    };
    Some(Bezier {
        closed: v["c"].as_bool().unwrap_or(false),
        v: pts("v"),
        i: pts("i"),
        o: pts("o"),
    })
}

fn shape_prop(v: &Value) -> Prop<Bezier> {
    parse_prop(v, &bezier_of, Bezier::default())
}

trait PropExt {
    fn f(&self, frame: f64) -> f64;
    fn p(&self, frame: f64) -> [f64; 2];
}

impl PropExt for Prop<Vec<f64>> {
    fn f(&self, frame: f64) -> f64 {
        self.at(frame).first().copied().unwrap_or(0.)
    }
    fn p(&self, frame: f64) -> [f64; 2] {
        let v = self.at(frame);
        [
            v.first().copied().unwrap_or(0.),
            v.get(1).copied().unwrap_or(0.),
        ]
    }
}

// ----------------------------------------------------------------- the model ----

#[derive(Clone, Debug)]
enum Position {
    Joined(Prop<Vec<f64>>),
    Split(Prop<Vec<f64>>, Prop<Vec<f64>>),
}

#[derive(Clone, Debug)]
struct Tr {
    a: Prop<Vec<f64>>,
    p: Position,
    s: Prop<Vec<f64>>,
    r: Prop<Vec<f64>>,
    o: Prop<Vec<f64>>,
    sk: Prop<Vec<f64>>,
    sa: Prop<Vec<f64>>,
}

impl Tr {
    fn parse(v: &Value) -> Tr {
        let p = &v["p"];
        let position =
            if p["s"].as_bool() == Some(true) || p.get("x").is_some() && p.get("k").is_none() {
                Position::Split(scalar(&p["x"], 0.), scalar(&p["y"], 0.))
            } else {
                Position::Joined(numbers(p, vec![0., 0.]))
            };
        let r = if v.get("r").is_some() {
            &v["r"]
        } else {
            &v["rz"]
        };
        Tr {
            a: numbers(&v["a"], vec![0., 0.]),
            p: position,
            s: numbers(&v["s"], vec![100., 100.]),
            r: scalar(r, 0.),
            o: scalar(&v["o"], 100.),
            sk: scalar(&v["sk"], 0.),
            sa: scalar(&v["sa"], 0.),
        }
    }

    fn matrix(&self, f: f64) -> Transform {
        let a = self.a.p(f);
        let p = match &self.p {
            Position::Joined(p) => {
                let v = p.at_spatial(f);
                [
                    v.first().copied().unwrap_or(0.),
                    v.get(1).copied().unwrap_or(0.),
                ]
            }
            Position::Split(x, y) => [x.f(f), y.f(f)],
        };
        let s = self.s.p(f);
        let r = self.r.f(f);
        let sk = self.sk.f(f);
        let mut m = Transform::from_translate(-a[0] as f32, -a[1] as f32)
            .post_scale((s[0] / 100.) as f32, (s[1] / 100.) as f32);
        if sk != 0. {
            let sa = self.sa.f(f) as f32;
            let tan = (-sk.to_radians()).tan() as f32;
            m = m
                .post_rotate(sa)
                .post_concat(Transform::from_row(1., 0., tan, 1., 0., 0.))
                .post_rotate(-sa);
        }
        m.post_rotate(r as f32)
            .post_translate(p[0] as f32, p[1] as f32)
    }

    fn opacity(&self, f: f64) -> f32 {
        (self.o.f(f) / 100.).clamp(0., 1.) as f32
    }
}

#[derive(Clone, Debug)]
struct Gradient {
    linear: bool,
    s: Prop<Vec<f64>>,
    e: Prop<Vec<f64>>,
    h: Prop<Vec<f64>>,
    a: Prop<Vec<f64>>,
    count: usize,
    k: Prop<Vec<f64>>,
}

impl Gradient {
    fn parse(v: &Value) -> Gradient {
        Gradient {
            linear: v["t"].as_i64() != Some(2),
            s: numbers(&v["s"], vec![0., 0.]),
            e: numbers(&v["e"], vec![0., 0.]),
            h: scalar(&v["h"], 0.),
            a: scalar(&v["a"], 0.),
            count: v["g"]["p"].as_u64().unwrap_or(0) as usize,
            k: numbers(&v["g"]["k"], Vec::new()),
        }
    }

    fn stops(&self, f: f64, alpha: f32) -> Vec<GradientStop> {
        let k = self.k.at(f);
        let n = self.count.min(k.len() / 4);
        let alphas: Vec<(f64, f64)> = k[n * 4..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0], c[1]))
            .collect();
        let alpha_at = |pos: f64| -> f64 {
            if alphas.is_empty() {
                return 1.;
            }
            if pos <= alphas[0].0 {
                return alphas[0].1;
            }
            for w in alphas.windows(2) {
                if pos <= w[1].0 {
                    let t = (pos - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
                    return w[0].1 + (w[1].1 - w[0].1) * t;
                }
            }
            alphas[alphas.len() - 1].1
        };
        (0..n)
            .map(|i| {
                let c = &k[i * 4..i * 4 + 4];
                let a = (alpha_at(c[0]) as f32 * alpha).clamp(0., 1.);
                GradientStop::new(
                    c[0] as f32,
                    tiny_skia::Color::from_rgba(
                        c[1].clamp(0., 1.) as f32,
                        c[2].clamp(0., 1.) as f32,
                        c[3].clamp(0., 1.) as f32,
                        a,
                    )
                    .unwrap_or(tiny_skia::Color::TRANSPARENT),
                )
            })
            .collect()
    }

    fn shader(&self, f: f64, alpha: f32, m: Transform) -> Option<Shader<'static>> {
        let mut stops = self.stops(f, alpha);
        if stops.is_empty() {
            return None;
        }
        if stops.len() == 1 {
            stops.push(stops[0]);
        }
        let s = self.s.p(f);
        let e = self.e.p(f);
        let sp = Point::from_xy(s[0] as f32, s[1] as f32);
        let ep = Point::from_xy(e[0] as f32, e[1] as f32);
        if self.linear {
            LinearGradient::new(sp, ep, stops, SpreadMode::Pad, m)
        } else {
            let r = ((e[0] - s[0]).powi(2) + (e[1] - s[1]).powi(2)).sqrt();
            let angle = (e[1] - s[1]).atan2(e[0] - s[0]) + self.a.f(f).to_radians();
            let h = (self.h.f(f) / 100.).clamp(-0.99, 0.99);
            let focal = Point::from_xy(
                (s[0] + angle.cos() * r * h) as f32,
                (s[1] + angle.sin() * r * h) as f32,
            );
            RadialGradient::new(focal, sp, r.max(0.01) as f32, stops, SpreadMode::Pad, m)
        }
    }
}

#[derive(Clone, Debug)]
struct StrokeStyle {
    w: Prop<Vec<f64>>,
    cap: LineCap,
    join: LineJoin,
    miter: f32,
    dashes: Vec<(char, Prop<Vec<f64>>)>,
}

impl StrokeStyle {
    fn parse(v: &Value) -> StrokeStyle {
        StrokeStyle {
            w: scalar(&v["w"], 1.),
            cap: match v["lc"].as_i64() {
                Some(2) => LineCap::Round,
                Some(3) => LineCap::Square,
                _ => LineCap::Butt,
            },
            join: match v["lj"].as_i64() {
                Some(2) => LineJoin::Round,
                Some(3) => LineJoin::Bevel,
                _ => LineJoin::Miter,
            },
            miter: v["ml"].as_f64().unwrap_or(4.) as f32,
            dashes: v["d"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|d| {
                            (
                                d["n"]
                                    .as_str()
                                    .and_then(|n| n.chars().next())
                                    .unwrap_or('d'),
                                scalar(&d["v"], 0.),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn stroke(&self, f: f64) -> Option<Stroke> {
        let width = self.w.f(f) as f32;
        if width <= 0. {
            return None;
        }
        let mut stroke = Stroke {
            width,
            miter_limit: self.miter,
            line_cap: self.cap,
            line_join: self.join,
            dash: None,
        };
        if !self.dashes.is_empty() {
            let mut array: Vec<f32> = Vec::new();
            let mut offset = 0.;
            for (kind, v) in &self.dashes {
                match kind {
                    'o' => offset = v.f(f) as f32,
                    _ => array.push(v.f(f).max(0.) as f32),
                }
            }
            if array.len() % 2 == 1 {
                array.extend(array.clone());
            }
            if array.iter().any(|d| *d > 0.) {
                stroke.dash = StrokeDash::new(array, offset);
            }
        }
        Some(stroke)
    }
}

#[derive(Clone, Debug)]
enum Item {
    Group {
        items: Vec<Item>,
        tr: Option<Tr>,
    },
    Path {
        shape: Prop<Bezier>,
        reversed: bool,
    },
    Rect {
        p: Prop<Vec<f64>>,
        s: Prop<Vec<f64>>,
        r: Prop<Vec<f64>>,
        reversed: bool,
    },
    Ellipse {
        p: Prop<Vec<f64>>,
        s: Prop<Vec<f64>>,
        reversed: bool,
    },
    Star {
        star: bool,
        p: Prop<Vec<f64>>,
        or: Prop<Vec<f64>>,
        ir: Prop<Vec<f64>>,
        pt: Prop<Vec<f64>>,
        r: Prop<Vec<f64>>,
        reversed: bool,
    },
    Fill {
        c: Prop<Vec<f64>>,
        o: Prop<Vec<f64>>,
        rule: FillRule,
    },
    Stroke {
        c: Prop<Vec<f64>>,
        o: Prop<Vec<f64>>,
        style: StrokeStyle,
    },
    GradientFill {
        g: Gradient,
        o: Prop<Vec<f64>>,
        rule: FillRule,
    },
    GradientStroke {
        g: Gradient,
        o: Prop<Vec<f64>>,
        style: StrokeStyle,
    },
    Trim {
        s: Prop<Vec<f64>>,
        e: Prop<Vec<f64>>,
        o: Prop<Vec<f64>>,
    },
}

fn parse_items(list: &Value) -> Vec<Item> {
    let Some(list) = list.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter(|v| v["hd"].as_bool() != Some(true))
        .filter_map(|v| {
            let reversed = v["d"].as_i64() == Some(3);
            let rule = if v["r"].as_i64() == Some(2) {
                FillRule::EvenOdd
            } else {
                FillRule::Winding
            };
            Some(match v["ty"].as_str()? {
                "gr" => {
                    let items = v["it"].as_array().cloned().unwrap_or_default();
                    let tr = items.iter().find(|i| i["ty"] == "tr").map(Tr::parse);
                    Item::Group {
                        items: parse_items(&Value::Array(items)),
                        tr,
                    }
                }
                "sh" => Item::Path {
                    shape: shape_prop(&v["ks"]),
                    reversed,
                },
                "rc" => Item::Rect {
                    p: numbers(&v["p"], vec![0., 0.]),
                    s: numbers(&v["s"], vec![0., 0.]),
                    r: scalar(&v["r"], 0.),
                    reversed,
                },
                "el" => Item::Ellipse {
                    p: numbers(&v["p"], vec![0., 0.]),
                    s: numbers(&v["s"], vec![0., 0.]),
                    reversed,
                },
                "sr" => Item::Star {
                    star: v["sy"].as_i64() != Some(2),
                    p: numbers(&v["p"], vec![0., 0.]),
                    or: scalar(&v["or"], 0.),
                    ir: scalar(&v["ir"], 0.),
                    pt: scalar(&v["pt"], 5.),
                    r: scalar(&v["r"], 0.),
                    reversed,
                },
                "fl" => Item::Fill {
                    c: numbers(&v["c"], vec![0., 0., 0.]),
                    o: scalar(&v["o"], 100.),
                    rule,
                },
                "st" => Item::Stroke {
                    c: numbers(&v["c"], vec![0., 0., 0.]),
                    o: scalar(&v["o"], 100.),
                    style: StrokeStyle::parse(v),
                },
                "gf" => Item::GradientFill {
                    g: Gradient::parse(v),
                    o: scalar(&v["o"], 100.),
                    rule,
                },
                "gs" => Item::GradientStroke {
                    g: Gradient::parse(v),
                    o: scalar(&v["o"], 100.),
                    style: StrokeStyle::parse(v),
                },
                "tm" => Item::Trim {
                    s: scalar(&v["s"], 0.),
                    e: scalar(&v["e"], 100.),
                    o: scalar(&v["o"], 0.),
                },
                _ => return None,
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
struct MaskDef {
    mode: char,
    inverted: bool,
    path: Prop<Bezier>,
    opacity: Prop<Vec<f64>>,
}

#[derive(Clone, Debug)]
struct Layer {
    ty: i64,
    ind: Option<i64>,
    parent: Option<i64>,
    ip: f64,
    op: f64,
    st: f64,
    sr: f64,
    ks: Tr,
    shapes: Vec<Item>,
    ref_id: Option<String>,
    tm: Option<Prop<Vec<f64>>>,
    masks: Vec<MaskDef>,
    matte: i64,
    matte_source: bool,
    hidden: bool,
    solid: Option<(f64, f64, [f32; 4])>,
}

fn parse_color_hex(s: &str) -> [f32; 4] {
    let s = s.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(s.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [
        f32::from(c(0)) / 255.,
        f32::from(c(2)) / 255.,
        f32::from(c(4)) / 255.,
        1.,
    ]
}

fn parse_layers(list: &Value) -> Vec<Layer> {
    let Some(list) = list.as_array() else {
        return Vec::new();
    };
    list.iter()
        .map(|v| Layer {
            ty: v["ty"].as_i64().unwrap_or(-1),
            ind: v["ind"].as_i64(),
            parent: v["parent"].as_i64(),
            ip: num(&v["ip"]),
            op: v["op"].as_f64().unwrap_or(f64::MAX),
            st: num(&v["st"]),
            sr: v["sr"].as_f64().filter(|s| *s != 0.).unwrap_or(1.),
            ks: Tr::parse(&v["ks"]),
            shapes: parse_items(&v["shapes"]),
            ref_id: v["refId"].as_str().map(str::to_string),
            tm: v.get("tm").map(|t| scalar(t, 0.)),
            masks: v["masksProperties"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| MaskDef {
                            mode: m["mode"]
                                .as_str()
                                .and_then(|s| s.chars().next())
                                .unwrap_or('a'),
                            inverted: m["inv"].as_bool().unwrap_or(false),
                            path: shape_prop(&m["pt"]),
                            opacity: scalar(&m["o"], 100.),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            matte: v["tt"].as_i64().unwrap_or(0),
            matte_source: v["td"].as_i64().unwrap_or(0) != 0,
            hidden: v["hd"].as_bool() == Some(true),
            solid: (v["ty"].as_i64() == Some(1)).then(|| {
                (
                    num(&v["sw"]),
                    num(&v["sh"]),
                    parse_color_hex(v["sc"].as_str().unwrap_or("#000000")),
                )
            }),
        })
        .collect()
}

/// A parsed animation.
#[derive(Clone, Debug)]
pub struct Animation {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub in_point: f64,
    pub out_point: f64,
    layers: Vec<Layer>,
    assets: HashMap<String, Vec<Layer>>,
}

impl Animation {
    /// A `.tgs` file: gzipped Lottie JSON.
    pub fn from_tgs(bytes: &[u8]) -> Result<Self, Error> {
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut json)
            .map_err(|_| Error::Gzip)?;
        Self::from_json(&json)
    }

    pub fn from_json(json: &[u8]) -> Result<Self, Error> {
        let v: Value = serde_json::from_slice(json).map_err(|_| Error::Json)?;
        let width = v["w"].as_u64().ok_or(Error::Format)? as u32;
        let height = v["h"].as_u64().ok_or(Error::Format)? as u32;
        let mut assets = HashMap::new();
        if let Some(list) = v["assets"].as_array() {
            for a in list {
                if let (Some(id), Some(_)) = (a["id"].as_str(), a["layers"].as_array()) {
                    assets.insert(id.to_string(), parse_layers(&a["layers"]));
                }
            }
        }
        Ok(Animation {
            width,
            height,
            frame_rate: v["fr"].as_f64().filter(|f| *f > 0.).unwrap_or(60.),
            in_point: num(&v["ip"]),
            out_point: num(&v["op"]),
            layers: parse_layers(&v["layers"]),
            assets,
        })
    }

    /// Frames from the in point to the out point.
    pub fn frame_count(&self) -> usize {
        (self.out_point - self.in_point).max(1.).round() as usize
    }

    /// Frame `index` (from 0) drawn into a `width` x `height` pixmap (premultiplied).
    pub fn render(&self, index: usize, width: u32, height: u32) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(width, height)?;
        self.render_into(index, &mut pixmap);
        Some(pixmap)
    }

    pub fn render_into(&self, index: usize, pixmap: &mut Pixmap) {
        let frame = self.in_point + index as f64;
        let base = Transform::from_scale(
            pixmap.width() as f32 / self.width.max(1) as f32,
            pixmap.height() as f32 / self.height.max(1) as f32,
        );
        self.draw_layers(&self.layers, frame, base, 1., pixmap, 0);
    }

    fn draw_layers(
        &self,
        layers: &[Layer],
        frame: f64,
        base: Transform,
        alpha: f32,
        pixmap: &mut Pixmap,
        depth: usize,
    ) {
        if depth > 8 {
            return;
        }
        for i in (0..layers.len()).rev() {
            let layer = &layers[i];
            if layer.hidden || layer.matte_source {
                continue;
            }
            if frame < layer.ip || frame >= layer.op {
                continue;
            }
            let matte =
                (layer.matte > 0 && i > 0 && layers[i - 1].matte_source).then(|| &layers[i - 1]);
            if matte.is_none() && layer.masks.is_empty() {
                self.draw_layer(layers, layer, frame, base, alpha, pixmap, depth);
                continue;
            }
            let (w, h) = (pixmap.width(), pixmap.height());
            let Some(mut own) = Pixmap::new(w, h) else {
                continue;
            };
            self.draw_layer(layers, layer, frame, base, alpha, &mut own, depth);
            if !layer.masks.is_empty()
                && let Some(mask) = self.layer_mask(layers, layer, frame, base, w, h)
            {
                own.apply_mask(&mask);
            }
            if let Some(source) = matte
                && let Some(mut matte_pixmap) = Pixmap::new(w, h)
            {
                if frame >= source.ip && frame < source.op {
                    self.draw_layer(layers, source, frame, base, 1., &mut matte_pixmap, depth);
                }
                let kind = if layer.matte >= 3 {
                    MaskType::Luminance
                } else {
                    MaskType::Alpha
                };
                let mut mask = Mask::from_pixmap(matte_pixmap.as_ref(), kind);
                if layer.matte == 2 || layer.matte == 4 {
                    mask.invert();
                }
                own.apply_mask(&mask);
            }
            pixmap.draw_pixmap(
                0,
                0,
                own.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }

    /// The layer's own time for its properties.
    fn local(layer: &Layer, frame: f64) -> f64 {
        (frame - layer.st) / layer.sr
    }

    fn layer_matrix(&self, layers: &[Layer], layer: &Layer, frame: f64, depth: usize) -> Transform {
        let own = layer.ks.matrix(Self::local(layer, frame));
        match layer.parent {
            Some(parent) if depth < 16 => match layers.iter().find(|l| l.ind == Some(parent)) {
                Some(p) => own.post_concat(self.layer_matrix(layers, p, frame, depth + 1)),
                None => own,
            },
            _ => own,
        }
    }

    fn layer_mask(
        &self,
        layers: &[Layer],
        layer: &Layer,
        frame: f64,
        base: Transform,
        w: u32,
        h: u32,
    ) -> Option<Mask> {
        let m = self.layer_matrix(layers, layer, frame, 0).post_concat(base);
        let local = Self::local(layer, frame);
        let mut result: Option<Vec<u8>> = None;
        for def in &layer.masks {
            if def.mode == 'n' {
                continue;
            }
            let mut one = Mask::new(w, h)?;
            if let Some(path) = bezier_path(&def.path.at(local)) {
                one.fill_path(&path, FillRule::Winding, true, m);
            }
            let opacity = (def.opacity.f(local) / 100.).clamp(0., 1.);
            let mut data: Vec<u8> = one
                .data()
                .iter()
                .map(|v| (f64::from(*v) * opacity) as u8)
                .collect();
            if def.inverted {
                for v in &mut data {
                    *v = 255 - *v;
                }
            }
            result = Some(match result {
                None => {
                    if def.mode == 's' {
                        data.iter().map(|v| 255 - v).collect()
                    } else {
                        data
                    }
                }
                Some(acc) => acc
                    .iter()
                    .zip(&data)
                    .map(|(a, b)| {
                        let (a, b) = (u32::from(*a), u32::from(*b));
                        (match def.mode {
                            's' => a * (255 - b) / 255,
                            'i' => a * b / 255,
                            'l' | 'f' | 'x' => a.max(b),
                            'd' => a.min(b),
                            _ => (a + b - a * b / 255).min(255),
                        }) as u8
                    })
                    .collect(),
            });
        }
        let data = result?;
        let mut mask = Mask::new(w, h)?;
        mask.data_mut().copy_from_slice(&data);
        Some(mask)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_layer(
        &self,
        layers: &[Layer],
        layer: &Layer,
        frame: f64,
        base: Transform,
        alpha: f32,
        pixmap: &mut Pixmap,
        depth: usize,
    ) {
        let local = Self::local(layer, frame);
        let m = self.layer_matrix(layers, layer, frame, 0).post_concat(base);
        let alpha = alpha * layer.ks.opacity(local);
        if alpha <= 0. {
            return;
        }
        match layer.ty {
            4 => {
                let mut styles = Vec::new();
                walk(&layer.shapes, local, m, alpha, &[], &[], &mut styles);
                for s in styles {
                    s.paint(pixmap);
                }
            }
            0 => {
                let Some(children) = layer.ref_id.as_ref().and_then(|id| self.assets.get(id))
                else {
                    return;
                };
                let child_frame = match &layer.tm {
                    Some(tm) => tm.f(local) * self.frame_rate,
                    None => local,
                };
                self.draw_layers(children, child_frame, m, alpha, pixmap, depth + 1);
            }
            1 => {
                if let Some((w, h, c)) = layer.solid
                    && let Some(rect) = tiny_skia::Rect::from_xywh(0., 0., w as f32, h as f32)
                {
                    let mut paint = Paint {
                        anti_alias: true,
                        ..Paint::default()
                    };
                    paint.set_color_rgba8(
                        (c[0] * 255.) as u8,
                        (c[1] * 255.) as u8,
                        (c[2] * 255.) as u8,
                        (alpha * 255.) as u8,
                    );
                    pixmap.fill_rect(rect, &paint, m, None);
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ geometry ----

type Pt = [f64; 2];

/// One contour as cubic segments.
#[derive(Clone, Debug)]
struct Contour {
    start: Pt,
    segs: Vec<[Pt; 3]>,
    closed: bool,
}

fn bezier_contour(b: &Bezier, reversed: bool) -> Option<Contour> {
    let n = b.v.len();
    if n == 0 {
        return None;
    }
    let get = |list: &[[f64; 2]], k: usize| list.get(k).copied().unwrap_or([0., 0.]);
    let mut segs = Vec::with_capacity(n);
    let count = if b.closed { n } else { n - 1 };
    for k in 0..count {
        let a = b.v[k];
        let next = (k + 1) % n;
        let z = b.v[next];
        let o = get(&b.o, k);
        let i = get(&b.i, next);
        segs.push([[a[0] + o[0], a[1] + o[1]], [z[0] + i[0], z[1] + i[1]], z]);
    }
    let c = Contour {
        start: b.v[0],
        segs,
        closed: b.closed,
    };
    Some(if reversed { reverse(&c) } else { c })
}

fn reverse(c: &Contour) -> Contour {
    let mut points = vec![c.start];
    for s in &c.segs {
        points.push(s[2]);
    }
    let mut segs = Vec::with_capacity(c.segs.len());
    for (k, s) in c.segs.iter().enumerate().rev() {
        segs.push([s[1], s[0], points[k]]);
    }
    Contour {
        start: *points.last().unwrap(),
        segs,
        closed: c.closed,
    }
}

fn bezier_path(b: &Bezier) -> Option<tiny_skia::Path> {
    to_path(&[bezier_contour(b, false)?])
}

fn to_path(contours: &[Contour]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for c in contours {
        pb.move_to(c.start[0] as f32, c.start[1] as f32);
        for s in &c.segs {
            pb.cubic_to(
                s[0][0] as f32,
                s[0][1] as f32,
                s[1][0] as f32,
                s[1][1] as f32,
                s[2][0] as f32,
                s[2][1] as f32,
            );
        }
        if c.closed {
            pb.close();
        }
    }
    pb.finish()
}

fn map(m: &Transform, p: Pt) -> Pt {
    let (x, y) = (p[0] as f32, p[1] as f32);
    [
        f64::from(m.sx * x + m.kx * y + m.tx),
        f64::from(m.ky * x + m.sy * y + m.ty),
    ]
}

fn transform_contour(c: &Contour, m: &Transform) -> Contour {
    Contour {
        start: map(m, c.start),
        segs: c
            .segs
            .iter()
            .map(|s| [map(m, s[0]), map(m, s[1]), map(m, s[2])])
            .collect(),
        closed: c.closed,
    }
}

const KAPPA: f64 = 0.552_284_749_8;

fn ellipse(p: Pt, s: Pt, reversed: bool) -> Contour {
    let (rx, ry) = (s[0] / 2., s[1] / 2.);
    let (cx, cy) = (p[0], p[1]);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let c = Contour {
        start: [cx, cy - ry],
        segs: vec![
            [[cx + kx, cy - ry], [cx + rx, cy - ky], [cx + rx, cy]],
            [[cx + rx, cy + ky], [cx + kx, cy + ry], [cx, cy + ry]],
            [[cx - kx, cy + ry], [cx - rx, cy + ky], [cx - rx, cy]],
            [[cx - rx, cy - ky], [cx - kx, cy - ry], [cx, cy - ry]],
        ],
        closed: true,
    };
    if reversed { reverse(&c) } else { c }
}

fn line(a: Pt, b: Pt) -> [Pt; 3] {
    [
        [a[0] + (b[0] - a[0]) / 3., a[1] + (b[1] - a[1]) / 3.],
        [
            a[0] + (b[0] - a[0]) * 2. / 3.,
            a[1] + (b[1] - a[1]) * 2. / 3.,
        ],
        b,
    ]
}

fn rect(p: Pt, s: Pt, r: f64, reversed: bool) -> Contour {
    let (w, h) = (s[0] / 2., s[1] / 2.);
    let r = r.min(w).min(h).max(0.);
    let (l, t, rt, b) = (p[0] - w, p[1] - h, p[0] + w, p[1] + h);
    let k = r * (1. - KAPPA);
    let c = if r <= 0. {
        Contour {
            start: [rt, t],
            segs: vec![
                line([rt, t], [rt, b]),
                line([rt, b], [l, b]),
                line([l, b], [l, t]),
                line([l, t], [rt, t]),
            ],
            closed: true,
        }
    } else {
        Contour {
            start: [rt, t + r],
            segs: vec![
                line([rt, t + r], [rt, b - r]),
                [[rt, b - k], [rt - k, b], [rt - r, b]],
                line([rt - r, b], [l + r, b]),
                [[l + k, b], [l, b - k], [l, b - r]],
                line([l, b - r], [l, t + r]),
                [[l, t + k], [l + k, t], [l + r, t]],
                line([l + r, t], [rt - r, t]),
                [[rt - k, t], [rt, t + k], [rt, t + r]],
            ],
            closed: true,
        }
    };
    if reversed { reverse(&c) } else { c }
}

fn star(
    star: bool,
    p: Pt,
    outer: f64,
    inner: f64,
    points: f64,
    rotation: f64,
    reversed: bool,
) -> Contour {
    let n = points.round().max(2.) as usize;
    let total = if star { n * 2 } else { n };
    let step = std::f64::consts::TAU / total as f64;
    let mut angle = (rotation - 90.).to_radians();
    let mut pts = Vec::with_capacity(total);
    for k in 0..total {
        let r = if star && k % 2 == 1 { inner } else { outer };
        pts.push([p[0] + r * angle.cos(), p[1] + r * angle.sin()]);
        angle += step;
    }
    let mut segs = Vec::with_capacity(total);
    for k in 0..total {
        segs.push(line(pts[k], pts[(k + 1) % total]));
    }
    let c = Contour {
        start: pts[0],
        segs,
        closed: true,
    };
    if reversed { reverse(&c) } else { c }
}

fn split(s0: Pt, s: &[Pt; 3], t: f64) -> ([Pt; 3], [Pt; 3]) {
    let lerp = |a: Pt, b: Pt| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let p01 = lerp(s0, s[0]);
    let p12 = lerp(s[0], s[1]);
    let p23 = lerp(s[1], s[2]);
    let p012 = lerp(p01, p12);
    let p123 = lerp(p12, p23);
    let mid = lerp(p012, p123);
    ([p01, p012, mid], [p123, p23, s[2]])
}

fn seg_length(s0: Pt, s: &[Pt; 3]) -> f64 {
    let mut len = 0.;
    let mut prev = s0;
    for k in 1..=16 {
        let u = f64::from(k) / 16.;
        let v = 1. - u;
        let w = [v * v * v, 3. * v * v * u, 3. * v * u * u, u * u * u];
        let q = [
            w[0] * s0[0] + w[1] * s[0][0] + w[2] * s[1][0] + w[3] * s[2][0],
            w[0] * s0[1] + w[1] * s[0][1] + w[2] * s[1][1] + w[3] * s[2][1],
        ];
        len += ((q[0] - prev[0]).powi(2) + (q[1] - prev[1]).powi(2)).sqrt();
        prev = q;
    }
    len
}

/// The part of the contours between `from` and `to` (lengths along all of them).
fn sub_path(contours: &[Contour], from: f64, to: f64) -> Vec<Contour> {
    let mut out = Vec::new();
    let mut at = 0.;
    for c in contours {
        let mut current: Option<Contour> = None;
        let mut s0 = c.start;
        for s in &c.segs {
            let len = seg_length(s0, s);
            let (a, b) = (at, at + len);
            if b > from && a < to && len > 0. {
                let t0 = ((from - a) / len).clamp(0., 1.);
                let t1 = ((to - a) / len).clamp(0., 1.);
                let mut seg = *s;
                let mut start = s0;
                if t1 < 1. {
                    seg = split(start, &seg, t1).0;
                }
                if t0 > 0. {
                    let rel = t0 / t1.max(1e-9);
                    let (head, tail) = split(start, &seg, rel.min(1.));
                    start = head[2];
                    seg = tail;
                }
                match &mut current {
                    Some(cur) => cur.segs.push(seg),
                    None => {
                        current = Some(Contour {
                            start,
                            segs: vec![seg],
                            closed: false,
                        })
                    }
                }
            } else if let Some(cur) = current.take() {
                out.push(cur);
            }
            at = b;
            s0 = s[2];
        }
        if let Some(cur) = current.take() {
            out.push(cur);
        }
    }
    out
}

fn trim(contours: Vec<Contour>, s: f64, e: f64, o: f64) -> Vec<Contour> {
    let (mut s, mut e) = (s / 100., e / 100.);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    if e - s >= 1. {
        return contours;
    }
    if e - s <= 0. {
        return Vec::new();
    }
    let shift = (o / 360.).rem_euclid(1.);
    s += shift;
    e += shift;
    let total: f64 = contours
        .iter()
        .map(|c| {
            let mut s0 = c.start;
            c.segs
                .iter()
                .map(|seg| {
                    let l = seg_length(s0, seg);
                    s0 = seg[2];
                    l
                })
                .sum::<f64>()
        })
        .sum();
    if s >= 1. {
        s -= 1.;
        e -= 1.;
    }
    if e <= 1. {
        sub_path(&contours, s * total, e * total)
    } else {
        let mut out = sub_path(&contours, s * total, total);
        out.extend(sub_path(&contours, 0., (e - 1.) * total));
        out
    }
}

// ----------------------------------------------------------------- painting ----

enum StyleKind {
    Fill(Shader<'static>, FillRule),
    Stroke(Shader<'static>, Stroke),
}

/// A fill or stroke with the paths it paints, in its own coordinates.
struct StylePaint {
    kind: Option<StyleKind>,
    matrix: Transform,
    contours: Vec<Contour>,
}

impl StylePaint {
    fn paint(self, pixmap: &mut Pixmap) {
        let Some(kind) = self.kind else {
            return;
        };
        let Some(path) = to_path(&self.contours) else {
            return;
        };
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        match kind {
            StyleKind::Fill(shader, rule) => {
                paint.shader = shader;
                pixmap.fill_path(&path, &paint, rule, self.matrix, None);
            }
            StyleKind::Stroke(shader, stroke) => {
                paint.shader = shader;
                pixmap.stroke_path(&path, &paint, &stroke, self.matrix, None);
            }
        }
    }
}

fn solid(c: &[f64], alpha: f32) -> Shader<'static> {
    let get = |k: usize| c.get(k).copied().unwrap_or(0.);
    let big = c.iter().take(3).any(|v| *v > 1.);
    let scale = if big { 255. } else { 1. };
    let color = tiny_skia::Color::from_rgba(
        (get(0) / scale).clamp(0., 1.) as f32,
        (get(1) / scale).clamp(0., 1.) as f32,
        (get(2) / scale).clamp(0., 1.) as f32,
        alpha.clamp(0., 1.),
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT);
    Shader::SolidColor(color)
}

#[derive(Clone, Copy)]
struct TrimMod {
    s: f64,
    e: f64,
    o: f64,
}

/// Walks a group's items bottom up (`rlottie`'s order): styles collect the paths
/// above them, trims change those paths.
fn walk(
    items: &[Item],
    f: f64,
    m: Transform,
    alpha: f32,
    parent_styles: &[usize],
    parent_trims: &[TrimMod],
    styles: &mut Vec<StylePaint>,
) {
    let mut active: Vec<usize> = parent_styles.to_vec();
    let mut trims: Vec<TrimMod> = parent_trims.to_vec();
    for item in items.iter().rev() {
        match item {
            Item::Fill { c, o, rule } => {
                let a = alpha * (o.f(f) / 100.).clamp(0., 1.) as f32;
                active.push(styles.len());
                styles.push(StylePaint {
                    kind: (a > 0.).then(|| StyleKind::Fill(solid(&c.at(f), a), *rule)),
                    matrix: m,
                    contours: Vec::new(),
                });
            }
            Item::Stroke { c, o, style } => {
                let a = alpha * (o.f(f) / 100.).clamp(0., 1.) as f32;
                active.push(styles.len());
                styles.push(StylePaint {
                    kind: style
                        .stroke(f)
                        .filter(|_| a > 0.)
                        .map(|st| StyleKind::Stroke(solid(&c.at(f), a), st)),
                    matrix: m,
                    contours: Vec::new(),
                });
            }
            Item::GradientFill { g, o, rule } => {
                let a = alpha * (o.f(f) / 100.).clamp(0., 1.) as f32;
                active.push(styles.len());
                styles.push(StylePaint {
                    kind: g
                        .shader(f, a, Transform::identity())
                        .map(|sh| StyleKind::Fill(sh, *rule)),
                    matrix: m,
                    contours: Vec::new(),
                });
            }
            Item::GradientStroke { g, o, style } => {
                let a = alpha * (o.f(f) / 100.).clamp(0., 1.) as f32;
                active.push(styles.len());
                styles.push(StylePaint {
                    kind: match (g.shader(f, a, Transform::identity()), style.stroke(f)) {
                        (Some(sh), Some(st)) => Some(StyleKind::Stroke(sh, st)),
                        _ => None,
                    },
                    matrix: m,
                    contours: Vec::new(),
                });
            }
            Item::Trim { s, e, o } => trims.push(TrimMod {
                s: s.f(f),
                e: e.f(f),
                o: o.f(f),
            }),
            Item::Group { items, tr } => {
                let (gm, ga) = match tr {
                    Some(tr) => (tr.matrix(f).post_concat(m), alpha * tr.opacity(f)),
                    None => (m, alpha),
                };
                if ga > 0. {
                    walk(items, f, gm, ga, &active, &trims, styles);
                }
            }
            shape => {
                let contours: Vec<Contour> = match shape {
                    Item::Path { shape, reversed } => bezier_contour(&shape.at(f), *reversed)
                        .into_iter()
                        .collect(),
                    Item::Rect { p, s, r, reversed } => {
                        vec![rect(p.p(f), s.p(f), r.f(f), *reversed)]
                    }
                    Item::Ellipse { p, s, reversed } => vec![ellipse(p.p(f), s.p(f), *reversed)],
                    Item::Star {
                        star: is_star,
                        p,
                        or,
                        ir,
                        pt,
                        r,
                        reversed,
                    } => vec![star(
                        *is_star,
                        p.p(f),
                        or.f(f),
                        ir.f(f),
                        pt.f(f),
                        r.f(f),
                        *reversed,
                    )],
                    _ => Vec::new(),
                };
                let mut contours = contours;
                for t in trims.iter().rev() {
                    contours = trim(contours, t.s, t.e, t.o);
                }
                if contours.is_empty() {
                    continue;
                }
                for &k in &active {
                    let style = &mut styles[k];
                    // Into the style's coordinates (strokes keep their width there).
                    let to_style = match style.matrix.invert() {
                        Some(inv) => m.post_concat(inv),
                        None => continue,
                    };
                    style
                        .contours
                        .extend(contours.iter().map(|c| transform_contour(c, &to_style)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_pixels(p: &Pixmap) -> usize {
        p.pixels().iter().filter(|c| c.alpha() > 0).count()
    }

    #[test]
    fn renders_shapes() {
        let json = br#"{"w":100,"h":100,"fr":60,"ip":0,"op":60,"layers":[
          {"ty":4,"ind":1,"ip":0,"op":60,"st":0,"ks":{"p":{"a":1,"k":[
             {"t":0,"s":[20,50],"o":{"x":0.3,"y":0},"i":{"x":0.7,"y":1}},
             {"t":59,"s":[80,50]}]}},
           "shapes":[{"ty":"gr","it":[
             {"ty":"el","p":{"k":[0,0]},"s":{"k":[30,30]}},
             {"ty":"fl","c":{"k":[1,0,0,1]},"o":{"k":100}},
             {"ty":"tr","p":{"k":[0,0]},"a":{"k":[0,0]},"s":{"k":[100,100]},"r":{"k":0},"o":{"k":100}}
           ]}]}]}"#;
        let a = Animation::from_json(json).unwrap();
        assert_eq!(a.frame_count(), 60);
        let first = a.render(0, 100, 100).unwrap();
        let last = a.render(59, 100, 100).unwrap();
        assert!(opaque_pixels(&first) > 500);
        // The circle moved right.
        let x_of = |p: &Pixmap| {
            let (mut sum, mut n) = (0usize, 0usize);
            for (i, c) in p.pixels().iter().enumerate() {
                if c.alpha() > 128 {
                    sum += i % 100;
                    n += 1;
                }
            }
            sum / n.max(1)
        };
        assert!(x_of(&first) < 30 && x_of(&last) > 70);
    }

    #[test]
    fn trims_and_strokes() {
        let json = br#"{"w":100,"h":100,"fr":30,"ip":0,"op":10,"layers":[
          {"ty":4,"ip":0,"op":10,"st":0,"ks":{},
           "shapes":[
             {"ty":"rc","p":{"k":[50,50]},"s":{"k":[60,60]},"r":{"k":0}},
             {"ty":"tm","s":{"k":0},"e":{"k":50},"o":{"k":0}},
             {"ty":"st","c":{"k":[0,0,1,1]},"o":{"k":100},"w":{"k":4},"lc":2,"lj":2}
           ]}]}"#;
        let a = Animation::from_json(json).unwrap();
        let p = a.render(0, 100, 100).unwrap();
        assert!(opaque_pixels(&p) > 100);
    }

    #[test]
    fn toast_animations_render() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/tdesktop-animations");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "tgs") {
                let bytes = std::fs::read(&path).unwrap();
                let a = Animation::from_tgs(&bytes).unwrap();
                let mid = a.render(a.frame_count() / 2, 64, 64).unwrap();
                assert!(opaque_pixels(&mid) > 0, "{}", path.display());
            }
        }
    }
}
