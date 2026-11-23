//! `Ui::Animations::Simple` and lib_ui's easing functions (`anim::easeOutCirc`, ...).

use std::time::Instant;

pub type Ease = fn(f32) -> f32;

pub fn linear(t: f32) -> f32 {
    t
}

pub fn ease_out_circ(t: f32) -> f32 {
    let t = t - 1.;
    (1. - t * t).max(0.).sqrt()
}

/// `anim::sineInOut`.
pub fn sine_in_out(t: f32) -> f32 {
    -((std::f32::consts::PI * t).cos() - 1.) / 2.
}

pub fn ease_in_circ(t: f32) -> f32 {
    1. - (1. - t * t).max(0.).sqrt()
}

/// `anim::interpolate` for floats.
pub fn mix(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// A value going from `from` to `to` over `duration` milliseconds.
#[derive(Clone, Copy)]
pub struct Simple {
    from: f32,
    to: f32,
    start: Instant,
    duration: f32,
    ease: Ease,
}

impl Simple {
    pub fn new(from: f32, to: f32, duration: f32, ease: Ease) -> Self {
        Self {
            from,
            to,
            start: Instant::now(),
            duration,
            ease,
        }
    }

    /// Linear progress, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.duration <= 0. {
            return 1.;
        }
        (self.start.elapsed().as_secs_f32() * 1000. / self.duration).clamp(0., 1.)
    }

    pub fn value(&self) -> f32 {
        let t = self.progress();
        if t >= 1. {
            self.to
        } else {
            mix(self.from, self.to, (self.ease)(t))
        }
    }

    pub fn animating(&self) -> bool {
        self.progress() < 1.
    }
}

/// `Simple::value(final)`: the animation's value while it runs, else `rest`.
pub fn value(anim: &Option<Simple>, rest: f32) -> f32 {
    match anim {
        Some(a) if a.animating() => a.value(),
        _ => rest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_ends() {
        for f in [linear as Ease, ease_out_circ, ease_in_circ] {
            assert!(f(0.).abs() < 1e-6);
            assert!((f(1.) - 1.).abs() < 1e-6);
        }
        // easeOutCirc runs ahead, easeInCirc behind.
        assert!(ease_out_circ(0.5) > 0.5);
        assert!(ease_in_circ(0.5) < 0.5);
    }
}
