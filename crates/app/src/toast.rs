//! Toasts (`Ui::Toast` with `st::defaultToast`): a dark rounded plate with a line of
//! text in the middle of the window, optionally with an animated icon on its left
//! ("Text copied to clipboard." with `toast/copy`). It fades in, stays 1.5 s and
//! fades out.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use gpui::{
    AnyElement, Corners, FontWeight, IntoElement, ParentElement, RenderImage, SharedString, Styled,
    TextRun, Window, canvas, div, font, size,
};
use tdr_theme::{Color, Palette};

use crate::scale::{self, lg, px};

// widgets.style: defaultToast.
const PADDING: [f32; 4] = [19., 13., 19., 12.];
const MAX_WIDTH: f32 = 480.;
const MARGIN: f32 = 13.;
const RADIUS: f32 = 8.;
const FADE_IN_MS: f32 = 200.;
const FADE_OUT_MS: f32 = 1000.;
/// `Ui::Toast::kDefaultDuration`.
const DURATION_MS: f32 = 1500.;
// chat_helpers.style: toastLottieIconSize; widgets.style: defaultToastIconPadding.
const ICON_SIZE: f32 = 20.;
const ICON_PADDING: [f32; 4] = [12., 2., 12., 2.];
const FONT_SIZE: f32 = 13.;
const LOTTIE_FPS: f32 = 60.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastIcon {
    /// `toast/copy`.
    Copy,
}

/// `boxWideWidth`: a wide toast's width (`windowArchiveToast`).
const WIDE_WIDTH: f32 = 364.;

#[derive(Clone, Debug)]
pub struct Toast {
    text: SharedString,
    icon: Option<ToastIcon>,
    shown: Instant,
    duration_ms: f32,
    /// `boxWideWidth` wide, its lines wrapped.
    wide: bool,
}

impl Toast {
    pub fn new(text: impl Into<SharedString>, icon: Option<ToastIcon>) -> Self {
        Self {
            text: text.into(),
            icon,
            shown: Instant::now(),
            duration_ms: DURATION_MS,
            wide: false,
        }
    }

    /// A wide toast of several lines, shown for `duration_ms`.
    pub fn wide(text: impl Into<SharedString>, duration_ms: f32) -> Self {
        Self {
            duration_ms,
            wide: true,
            ..Self::new(text, None)
        }
    }

    /// How much it is shown (0..1), `None` once it is gone.
    pub fn level(&self) -> Option<f32> {
        let ms = self.shown.elapsed().as_secs_f32() * 1000.;
        if ms < FADE_IN_MS {
            Some(ms / FADE_IN_MS)
        } else if ms < self.duration_ms {
            Some(1.)
        } else if ms < self.duration_ms + FADE_OUT_MS {
            Some(1. - (ms - self.duration_ms) / FADE_OUT_MS)
        } else {
            None
        }
    }
}

type Frames = HashMap<(ToastIcon, u32, u32), (Arc<RenderImage>, usize)>;
static FRAMES: LazyLock<Mutex<Frames>> = LazyLock::new(Default::default);

/// Every frame of the icon's animation, tinted.
fn icon_frames(
    icon: ToastIcon,
    color: Color,
    scale_factor: f32,
) -> Option<(Arc<RenderImage>, usize)> {
    let key = (icon, color.rgba(), scale_factor.to_bits());
    if let Some(f) = FRAMES.lock().unwrap().get(&key) {
        return Some(f.clone());
    }
    let bytes: &[u8] = match icon {
        ToastIcon::Copy => include_bytes!("../../../assets/tdesktop-animations/toast__copy.tgs"),
    };
    let animation = crate::lottie::Animation::from_tgs(bytes)?;
    let side = scale::device(ICON_SIZE).max(1.) as u32;
    let rgba = color.rgba();
    let (r, g, b, a) = (
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        (rgba & 0xff) as f32 / 255.,
    );
    let frames: Vec<image::Frame> = (0..animation.frames)
        .map(|f| {
            let coverage = animation.render(f as f64, side);
            let mut img = image::RgbaImage::new(side, side);
            for (px, c) in img.pixels_mut().zip(coverage) {
                px.0 = [b, g, r, (c as f32 * a).round() as u8];
            }
            image::Frame::new(img)
        })
        .collect();
    let count = frames.len();
    let out = (
        Arc::new(RenderImage::new(smallvec::SmallVec::from_vec(frames))),
        count,
    );
    FRAMES.lock().unwrap().insert(key, out.clone());
    Some(out)
}

fn text_width(window: &Window, text: &str) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font("Open Sans"),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .layout_line(text, scale::font(FONT_SIZE).size, &[run], None)
            .width,
    )
}

/// The toast over an area `area_width` x `area_height` at (`left`, `top`): its
/// middle (`Widget::updateGeometry` with no slide side).
pub fn render(
    toast: &Toast,
    p: &Palette,
    left: f32,
    top: f32,
    area_width: f32,
    area_height: f32,
    window: &mut Window,
) -> Option<AnyElement> {
    let level = toast.level()?;
    let f = scale::font(FONT_SIZE);
    let [pl, pt, pr, pb] = PADDING.map(lg);
    if toast.wide {
        // Its lines wrap in the fixed width; the plate sits in the middle.
        let width = lg(WIDE_WIDTH).min(area_width - 2. * lg(MARGIN)).max(0.);
        window.request_animation_frame();
        return Some(
            div()
                .absolute()
                .left(gpui::px(left))
                .top(gpui::px(top))
                .w(gpui::px(area_width))
                .h(gpui::px(area_height))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(gpui::px(width))
                        .pl(gpui::px(pl))
                        .pr(gpui::px(pr))
                        .pt(gpui::px(pt))
                        .pb(gpui::px(pb))
                        .opacity(level)
                        .rounded(px(RADIUS))
                        .bg(gpui::rgba(p.toast_bg.rgba()))
                        .text_color(gpui::rgba(p.toast_fg.rgba()))
                        .text_size(f.size)
                        .line_height(f.line_height())
                        .font_weight(FontWeight::NORMAL)
                        .flex()
                        .flex_col()
                        .children(
                            toast
                                .text
                                .split('\n')
                                .map(|line| div().w_full().child(line.to_string())),
                        ),
                )
                .into_any_element(),
        );
    }
    let icon = toast
        .icon
        .and_then(|i| icon_frames(i, p.toast_fg, window.scale_factor()));
    let icon_full = if icon.is_some() {
        lg(ICON_PADDING[0]) + lg(ICON_SIZE) + lg(ICON_PADDING[2])
    } else {
        0.
    };
    let left_pad = if icon.is_some() { icon_full } else { pl };
    let natural = left_pad + text_width(window, &toast.text).ceil() + pr;
    let width = natural
        .min(lg(MAX_WIDTH))
        .min(area_width - 2. * lg(MARGIN))
        .max(0.);
    let mut height = pt + f.height + pb;
    if icon.is_some() {
        height = height.max(lg(ICON_PADDING[1]) + lg(ICON_SIZE) + lg(ICON_PADDING[3]));
    }
    let x = left + ((area_width - width) / 2.).floor();
    let y = top + ((area_height - height) / 2.).floor();
    let elapsed = toast.shown.elapsed().as_secs_f32();
    let icon_el = icon.map(|(image, count)| {
        let frame = ((elapsed * LOTTIE_FPS) as usize).min(count.saturating_sub(1));
        let side = lg(ICON_SIZE);
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let _ = window.paint_image(
                    gpui::Bounds::new(bounds.origin, size(gpui::px(side), gpui::px(side))),
                    Corners::default(),
                    image.clone(),
                    frame,
                    false,
                );
                if frame + 1 < count {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .left(px(ICON_PADDING[0]))
        .top(gpui::px(((height - side) / 2.).floor()))
        .w(gpui::px(side))
        .h(gpui::px(side))
    });
    window.request_animation_frame();
    Some(
        div()
            .absolute()
            .left(gpui::px(x))
            .top(gpui::px(y))
            .w(gpui::px(width))
            .h(gpui::px(height))
            .opacity(level)
            .rounded(px(RADIUS))
            .bg(gpui::rgba(p.toast_bg.rgba()))
            .text_color(gpui::rgba(p.toast_fg.rgba()))
            .text_size(f.size)
            .line_height(f.line_height())
            .font_weight(FontWeight::NORMAL)
            .children(icon_el)
            .child(
                div()
                    .absolute()
                    .left(gpui::px(left_pad))
                    .top(gpui::px(((height - f.height) / 2.).floor()))
                    .w(gpui::px((width - left_pad - pr).max(0.)))
                    .h(f.line_height())
                    .overflow_hidden()
                    .child(crate::one_line::one_line(toast.text.to_string())),
            )
            .into_any_element(),
    )
}
