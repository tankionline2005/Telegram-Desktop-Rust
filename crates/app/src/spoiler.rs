//! Spoiler particles, ported from `lib_ui/ui/effects/spoiler_mess.cpp`
//! (`DefaultDescriptorText`): 9000 particles drawn into a square tile of 60 frames,
//! 33 ms each; hidden text is covered by that tile repeated, in the palette's spoiler
//! color. The tile is generated once per scale (tens of milliseconds) and tinted
//! once per color.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use gpui::{Bounds, ContentMask, Corners, Pixels, Point, RenderImage, Window, point, px, size};
use tdr_theme::Color;

const FRAMES: usize = 60;
const FRAME_MS: u128 = 33;
const PARTICLES: usize = 9000;
const SPRITES: usize = 5;
const FADE_IN: f64 = 200.;
const SHOWN: f64 = 200.;
const FADE_OUT: f64 = 200.;

/// Coverage of every frame of the tile, `size` x `size` device pixels.
struct Mask {
    size: usize,
    frames: Vec<Vec<f32>>,
}

static MASKS: LazyLock<Mutex<HashMap<u32, Arc<Mask>>>> = LazyLock::new(Default::default);
type Tinted = HashMap<(u32, u32), (Arc<RenderImage>, usize)>;
static TINTED: LazyLock<Mutex<Tinted>> = LazyLock::new(Default::default);
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The frame to show now (all spoilers animate together).
pub fn current_frame() -> usize {
    (EPOCH.elapsed().as_millis() / FRAME_MS) as usize % FRAMES
}

/// `base::BufferedRandom` stand-in: any uniform generator gives the same look.
struct Random(u64);

impl Random {
    fn index(&mut self, n: usize) -> usize {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize % n.max(1)
    }
}

struct Particle {
    start: f64,
    sprite: usize,
    x: i32,
    y: i32,
    dx: f64,
    dy: f64,
}

/// A rounded rectangle sprite at (1, 1), its coverage by 4x4 supersampling
/// (`GenerateSprite`).
fn sprite(index: usize, size_min: f64, size_max: f64, sprite_size: usize) -> Vec<f32> {
    let middle = SPRITES / 2;
    let delta = size_max - size_min;
    let width = if index < middle {
        size_min + delta * (middle - index) as f64 / middle as f64
    } else {
        size_min
    };
    let height = if index > middle {
        size_min + delta * (index - middle) as f64 / (SPRITES - 1 - middle) as f64
    } else {
        size_min
    };
    let radius = size_min / 2.;
    let inside = |x: f64, y: f64| -> bool {
        let (x, y) = (x - 1., y - 1.);
        if x < 0. || y < 0. || x > width || y > height {
            return false;
        }
        let cx = x.clamp(radius, width - radius);
        let cy = y.clamp(radius, height - radius);
        (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
    };
    let mut out = vec![0.; sprite_size * sprite_size];
    for py in 0..sprite_size {
        for px in 0..sprite_size {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = px as f64 + (sx as f64 + 0.5) / 4.;
                    let y = py as f64 + (sy as f64 + 0.5) / 4.;
                    hits += inside(x, y) as u32;
                }
            }
            out[py * sprite_size + px] = hits as f32 / 16.;
        }
    }
    out
}

fn generate(scale: f32) -> Mask {
    let scale = scale as f64;
    // `style::ConvertScale(128)` and `ConvertScaleExact` sizes and speeds.
    let size = (((128. * scale) - 0.01).round() as usize).max(1);
    let size_min = 1.5 * scale;
    let size_max = 2. * scale;
    let speed_min = 4. * scale;
    let speed_max = 8. * scale;
    let lifetime = FADE_IN + SHOWN + FADE_OUT;
    let full = (FRAMES as u128 * FRAME_MS) as f64;
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let particles: Vec<Particle> = (0..PARTICLES)
        .map(|i| {
            // `RandomSpeed`.
            let count = PARTICLES;
            let value = random.index(2 * count + 2);
            let negative = value < count + 1;
            let module = if negative { value } else { value - count - 1 };
            let speed = speed_min + ((speed_max - speed_min) * module as f64) / count as f64;
            let max = (speed_max * lifetime).ceil() as usize;
            let k = speed / lifetime;
            let x = (random.index(2 * max + 1) as f64 - max as f64) / max as f64;
            let y = (1. - x * x).max(0.).sqrt() * if negative { -1. } else { 1. };
            Particle {
                start: (i as u128 * FRAMES as u128 * FRAME_MS / PARTICLES as u128) as f64,
                sprite: random.index(SPRITES),
                x: random.index(size) as i32,
                y: random.index(size) as i32,
                dx: k * x,
                dy: k * y,
            }
        })
        .collect();
    let sprite_size = 2 + size_max.ceil() as usize;
    let sprites: Vec<Vec<f32>> = (0..SPRITES)
        .map(|i| sprite(i, size_min, size_max, sprite_size))
        .collect();
    let n = size as i32;
    let frames = (0..FRAMES)
        .map(|frame| {
            let mut buf = vec![0f32; size * size];
            let mut draw = |sx: i32, sy: i32, sprite: &[f32], opacity: f32| {
                for row in 0..sprite_size as i32 {
                    let y = sy + row;
                    if y < 0 || y >= n {
                        continue;
                    }
                    for col in 0..sprite_size as i32 {
                        let x = sx + col;
                        if x < 0 || x >= n {
                            continue;
                        }
                        let a = sprite[(row as usize) * sprite_size + col as usize] * opacity;
                        if a > 0. {
                            let d = &mut buf[(y * n + x) as usize];
                            *d += a * (1. - *d);
                        }
                    }
                }
            };
            let now = frame as f64 * FRAME_MS as f64;
            for p in &particles {
                for t in [now - p.start, now + full - p.start] {
                    if t <= 0. || t >= lifetime {
                        continue;
                    }
                    let wrap = |v: i32| v.rem_euclid(n);
                    let x = wrap(p.x + (t * p.dx).round() as i32);
                    let y = wrap(p.y + (t * p.dy).round() as i32);
                    let opacity = if t < FADE_IN {
                        t / FADE_IN
                    } else if t > lifetime - FADE_OUT {
                        (lifetime - t) / FADE_OUT
                    } else {
                        1.
                    } as f32;
                    let s = &sprites[p.sprite];
                    let ss = sprite_size as i32;
                    draw(x, y, s, opacity);
                    if x + ss > n {
                        draw(x - n, y, s, opacity);
                        if y + ss > n {
                            draw(x, y - n, s, opacity);
                            draw(x - n, y - n, s, opacity);
                        }
                    } else if y + ss > n {
                        draw(x, y - n, s, opacity);
                    }
                }
            }
            buf
        })
        .collect();
    Mask { size, frames }
}

/// The tile in `color`, all frames, and its size in device pixels.
fn tinted(color: Color, scale: f32) -> (Arc<RenderImage>, usize) {
    let key = (color.rgba(), scale.to_bits());
    if let Some(t) = TINTED.lock().unwrap().get(&key) {
        return t.clone();
    }
    let mask = {
        let cached = MASKS.lock().unwrap().get(&scale.to_bits()).cloned();
        match cached {
            Some(m) => m,
            None => {
                let started = Instant::now();
                let m = Arc::new(generate(scale));
                tracing::info!(elapsed = ?started.elapsed(), "spoiler particles generated");
                MASKS.lock().unwrap().insert(scale.to_bits(), m.clone());
                m
            }
        }
    };
    let rgba = color.rgba();
    let (r, g, b, a) = (
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        (rgba & 0xff) as f32 / 255.,
    );
    let frames: Vec<image::Frame> = mask
        .frames
        .iter()
        .map(|coverage| {
            let mut img = image::RgbaImage::new(mask.size as u32, mask.size as u32);
            for (px, c) in img.pixels_mut().zip(coverage) {
                // GPUI expects BGRA.
                px.0 = [b, g, r, (c * a * 255.).round() as u8];
            }
            image::Frame::new(img)
        })
        .collect();
    let image = Arc::new(RenderImage::new(smallvec::SmallVec::from_vec(frames)));
    let out = (image, mask.size);
    TINTED.lock().unwrap().insert(key, out.clone());
    out
}

/// Covers `rect` with the particles (`FillSpoilerRect`), the tiles aligned to
/// `origin` (the text's top left).
pub fn fill(
    rect: Bounds<Pixels>,
    origin: Point<Pixels>,
    color: Color,
    frame: usize,
    window: &mut Window,
) {
    if rect.size.width <= px(0.) || rect.size.height <= px(0.) {
        return;
    }
    let scale = window.scale_factor();
    let (image, device) = tinted(color, scale);
    let tile = px(device as f32 / scale);
    let first = |from: Pixels, o: Pixels| o + tile * ((from - o) / tile).floor();
    let x0 = first(rect.origin.x, origin.x);
    let y0 = first(rect.origin.y, origin.y);
    window.with_content_mask(Some(ContentMask { bounds: rect }), |window| {
        let mut y = y0;
        while y < rect.bottom() {
            let mut x = x0;
            while x < rect.right() {
                let _ = window.paint_image(
                    Bounds::new(point(x, y), size(tile, tile)),
                    Corners::default(),
                    image.clone(),
                    frame,
                    false,
                );
                x += tile;
            }
            y += tile;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_has_particles_in_every_frame() {
        let mask = generate(1.);
        assert_eq!(mask.size, 128);
        assert_eq!(mask.frames.len(), FRAMES);
        for frame in &mask.frames {
            let covered = frame.iter().filter(|c| **c > 0.).count();
            // About a third of the particles are alive at any time.
            assert!(covered > 1000, "{covered}");
        }
    }
}
