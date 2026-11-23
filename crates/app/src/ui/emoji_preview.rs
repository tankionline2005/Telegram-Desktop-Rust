//! The large preview of a custom emoji held down in the emoji panel or a set box
//! (tdesktop's `MediaPreviewWidget` via `showMediaPreview`): after
//! `QApplication::startDragTime` the emoji shows at sticker size over a dimmed
//! window with its emoji above; moving over other emoji while held switches it,
//! releasing hides it (and chooses nothing).

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled,
    Window, canvas, div, point,
};

use super::{MainView, c};
use crate::scale::{self, lg, px as gpx};

/// `QApplication::startDragTime()` on Windows.
const PREVIEW_DELAY: Duration = Duration::from_millis(500);
/// `stickerPreviewDuration`.
const PREVIEW_FADE_MS: f32 = 150.;
/// `maxStickerSize`.
const PREVIEW_SIZE: f32 = 256.;

#[derive(Default)]
pub(super) struct EmojiPreview {
    /// The custom emoji held and since when.
    press: Option<(i64, Instant)>,
    /// The one shown, since when, and when it began to hide.
    shown: Option<(i64, Instant)>,
    hiding: Option<(i64, Instant)>,
}

impl MainView {
    /// A press on a custom emoji: its preview shows if it is held.
    pub(super) fn preview_press(&mut self, id: i64, cx: &mut Context<Self>) {
        self.emoji_preview.press = Some((id, Instant::now()));
        Self::wake_after(PREVIEW_DELAY, cx);
    }

    /// The mouse went over a custom emoji while held: the preview follows.
    pub(super) fn preview_hover(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some((shown, _)) = &mut self.emoji_preview.shown
            && *shown != id
        {
            *shown = id;
            cx.notify();
        } else if let Some((pressed, _)) = &mut self.emoji_preview.press {
            *pressed = id;
        }
    }

    /// The button went up: true when a preview was shown (no choice then).
    pub(super) fn preview_release(&mut self, cx: &mut Context<Self>) -> bool {
        self.emoji_preview.press = None;
        match self.emoji_preview.shown.take() {
            Some((id, _)) => {
                self.emoji_preview.hiding = Some((id, Instant::now()));
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Shows the held emoji's preview when it is due.
    pub(super) fn update_emoji_preview(&mut self, window: &mut Window) {
        if self.emoji_preview.shown.is_none()
            && let Some((id, at)) = self.emoji_preview.press
            && at.elapsed() >= PREVIEW_DELAY
        {
            self.emoji_preview.shown = Some((id, Instant::now()));
            self.emoji_preview.hiding = None;
        }
        let fading = |t: Option<(i64, Instant)>| {
            t.is_some_and(|(_, at)| at.elapsed().as_secs_f32() * 1000. < PREVIEW_FADE_MS)
        };
        if fading(self.emoji_preview.shown) || fading(self.emoji_preview.hiding) {
            window.request_animation_frame();
        } else if self.emoji_preview.hiding.is_some() {
            self.emoji_preview.hiding = None;
        }
    }

    /// The dimmed window with the emoji in the middle (`MediaPreviewWidget::paintEvent`).
    pub(super) fn render_emoji_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (id, opacity) = match (self.emoji_preview.shown, self.emoji_preview.hiding) {
            (Some((id, at)), _) => (
                id,
                (at.elapsed().as_secs_f32() * 1000. / PREVIEW_FADE_MS).min(1.),
            ),
            (None, Some((id, at))) => (
                id,
                1. - (at.elapsed().as_secs_f32() * 1000. / PREVIEW_FADE_MS).min(1.),
            ),
            (None, None) => return None,
        };
        if opacity <= 0. {
            return None;
        }
        let p = self.palette();
        let size = window.viewport_size();
        let top = lg(super::TITLE_HEIGHT);
        let (w, h) = (f32::from(size.width), f32::from(size.height) - top);
        let side = (lg(PREVIEW_SIZE) * scale::get()).round() as u32;
        let side_l = side as f32 / scale::get();
        let alt =
            crate::custom_emoji::info(id).and_then(|i| tdr_emoji::find(&i.alt).map(|(e, _)| e));
        let fg: gpui::Hsla = c(p.window_fg).into();
        let emoji_size = crate::emoji::size_large() as f32 / scale::get();
        Some(
            div()
                .id("emoji-preview")
                .absolute()
                .left_0()
                .top(gpx(top))
                .w(gpx(w))
                .h(gpx(h))
                .opacity(opacity)
                .bg(c(p.sticker_preview_bg))
                // Not occluding: the emoji under it still see the mouse, so the
                // preview follows it; the release stops here (no choice).
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.preview_release(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let x = (w - side_l) / 2.;
                            let y = (h - side_l) / 2.;
                            let at = point(
                                bounds.origin.x + gpx(x.floor()),
                                bounds.origin.y + gpx(y.floor()),
                            );
                            crate::custom_emoji::paint(id, at, side, fg, window);
                            if let Some(emoji) = alt {
                                // The emoji above (`_emojiList`, one here).
                                let at = point(
                                    bounds.origin.x + gpx(((w - emoji_size) / 2.).floor()),
                                    bounds.origin.y + gpx((y - emoji_size * 2.).floor()),
                                );
                                crate::emoji::paint(emoji, at, crate::emoji::size_large(), window);
                            }
                        },
                    )
                    .size_full(),
                )
                .into_any_element(),
        )
    }
}
