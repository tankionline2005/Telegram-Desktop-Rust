//! The main window: custom title bar, chat list on the left, the open chat on the right.
//!
//! Sizes follow tdesktop's styles (`dialogs.style`, `ui/chat/chat.style`,
//! `chat_helpers.style`, `window/window.style`, `lib_ui/ui/widgets/widgets.style`),
//! colors come from its palette (`tdr-theme`), icons from its resources (`icons`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListOffset,
    ListState, MouseButton, Pixels, RenderImage, Rgba, SharedString, TextRun,
    UniformListScrollHandle, WeakEntity, Window, WindowControlArea, canvas, div, font, img,
    linear_color_stop, linear_gradient, list, point, prelude::*, rgba, uniform_list,
};
use tdr_data::describe::PeerKind as ChatKind;
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Command, DialogItem, FolderItem, MessageItem, Update};
use crate::icons::{Icon, icon, icon_width};
use crate::one_line::one_line;
use crate::placement::{COLUMN_MIN_LEFT, COLUMN_MIN_MAIN, Columns};
use crate::present;
use crate::scale::{self, dpx, lg, px};
use crate::text_input::{FieldMenu, Submit, TagSpan, TextInput};

pub(crate) use chrome::paint_radial;

mod archive;
mod chrome;
mod country_box;
mod emoji_panel;
mod emoji_set_box;
mod emoji_suggest_box;
mod layers;
mod main_menu;
mod menus;
mod message_text;
mod settings;
mod threads;

gpui::actions!(main_view, [Escape]);

/// Window-wide keys (the text field's own keys are in `text_input::bind_keys`).
/// Ctrl+C anywhere copies selected message text; in the text field it copies the
/// field's selection, or bubbles up here when the field has none.
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("escape", Escape, None),
        gpui::KeyBinding::new("secondary-c", crate::text_input::Copy, None),
    ]);
}

// widgets.style: windowTitleHeight, windowTitleButtonWidth
pub(crate) const TITLE_HEIGHT: f32 = 24.;
const TITLE_BUTTON_WIDTH: f32 = 36.;
// dialogs.style: defaultDialogRow
const DIALOG_ROW_HEIGHT: f32 = 62.;
const DIALOG_PHOTO: f32 = 46.;
const DIALOG_PADDING_X: f32 = 10.;
const DIALOG_PADDING_Y: f32 = 8.;
const TOP_BAR_HEIGHT: f32 = 54.;
// dialogs.style: dialogsChatTypeSkip, dialogsUnread*
const CHAT_TYPE_SKIP: f32 = 3.;
const UNREAD_HEIGHT: f32 = 19.;
const UNREAD_PADDING: f32 = 5.;
// window.style: adaptiveChatWideWidth
const CHAT_WIDE_WIDTH: f32 = 880.;
// chat.style: historyResizeWidth (the column edge you drag)
const RESIZE_WIDTH: f32 = 6.;
// chat.style
const MSG_MAX_WIDTH: f32 = 430.;
const MSG_PADDING_X: f32 = 11.;
const MSG_PADDING_Y: f32 = 8.;
const MSG_RADIUS_LARGE: f32 = 16.; // bubbleRadiusLarge
const MSG_RADIUS_SMALL: f32 = 6.; // bubbleRadiusSmall = roundRadiusLarge
const MSG_MARGIN_TOP: f32 = 6.;
const MSG_MARGIN_TOP_ATTACHED: f32 = 0.;
const MSG_SIDE: f32 = 16.;
const MSG_MARGIN_FAR: f32 = 56.;
const MSG_PHOTO_SIZE: f32 = 33.;
const MSG_PHOTO_SKIP: f32 = 40.;
/// A wide chat lays everything out in a column this wide at its left edge
/// (`HistoryView::WideChatWidth`: msgMaxWidth + 2 msgPhotoSkip + 2 msgMargin.left).
const WIDE_CHAT_COLUMN: f32 = MSG_MAX_WIDTH + 2. * MSG_PHOTO_SKIP + 2. * MSG_SIDE;
// msgServicePadding, msgServiceMargin
const SERVICE_PADDING_X: f32 = 12.;
const SERVICE_PADDING_TOP: f32 = 3.;
const SERVICE_PADDING_BOTTOM: f32 = 4.;
const SERVICE_MARGIN_TOP: f32 = 10.;
const SERVICE_MARGIN_BOTTOM: f32 = 2.;
const SERVICE_MAX_WIDTH: f32 = 320.;
// historyUnreadBarHeight, historyUnreadBarMargin
const UNREAD_BAR_HEIGHT: f32 = 32.;
const UNREAD_BAR_MARGIN: f32 = 8.;
// chat.style: msgDateSpace, msgDateDelta, historySendStateSpace,
// historySendStatePosition, msgShadow
const MSG_DATE_SPACE: f32 = 12.;
const MSG_DATE_DELTA_X: f32 = 2.;
const MSG_DATE_DELTA_Y: f32 = 5.;
const SEND_STATE_SPACE: f32 = 24.;
const SEND_STATE_X: f32 = -17.;
const SEND_STATE_Y: f32 = -19.;
const MSG_SHADOW: f32 = 2.;
const MSG_MARGIN_BOTTOM: f32 = 2.;
/// Messages of one author closer than this are drawn as a group
/// (tdesktop: `kAttachMessageToPreviousSecondsDelta`).
const ATTACH_SECONDS: i32 = 900;
// chat_helpers.style: historyToDown*
const TO_DOWN_WIDTH: f32 = 52.;
const TO_DOWN_HEIGHT: f32 = 62.;
const TO_DOWN_RIGHT: f32 = 12.;
const TO_DOWN_BOTTOM: f32 = 10.;
const TO_DOWN_ICON_TOP: f32 = 10.;
const TO_DOWN_SHOWN_AFTER: f32 = 480.;
/// The chat list's "scroll to top" button sits this far above the list's bottom
/// (`connectingMargin.top`).
const TO_UP_BOTTOM: f32 = 2.;
// ScrollArea styles: deltax/deltat/deltab and minHeight are shared; width and round
// differ (`historyScroll` in the chat, `defaultScrollArea` in the chat list).
const SCROLL_DELTA: f32 = 3.;
const SCROLL_MIN_BAR: f32 = 20.;
const SCROLL_HIDE_AFTER: Duration = Duration::from_millis(1000);
// basic.style: fsize
const FONT_SIZE: f32 = 13.;
/// `msgDateFont` and `dialogsDateFont`: font(13px).
const DATE_FONT_SIZE: f32 = 13.;
/// `dialogsUnreadFont`: font(12px bold).
const UNREAD_FONT_SIZE: f32 = 12.;
// dialogs.style: defaultDialogRow nameLeft / nameTop / textTop, dialogsDateSkip,
// dialogsSendStateSkip, dialogsUnreadPadding
const DIALOG_NAME_LEFT: f32 = 68.;
const DIALOG_NAME_TOP: f32 = 10.;
const DIALOG_TEXT_TOP: f32 = 34.;
const DIALOG_DATE_SKIP: f32 = 5.;
const DIALOG_SEND_STATE_SKIP: f32 = 20.;
const FONT_FAMILY: &str = "Open Sans";
/// Ask for older messages when the top of the list is this close (in items).
const LOAD_OLDER_THRESHOLD: usize = 3;

fn c(color: Color) -> Rgba {
    rgba(color.rgba())
}

/// Width of the longest line of `text` when laid out without wrapping.
pub(crate) fn text_width(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    let mut f = font(FONT_FAMILY);
    f.weight = weight;
    text.split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let run = TextRun {
                len: line.len(),
                font: f.clone(),
                color: gpui::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(
                window
                    .text_system()
                    .layout_line(line, px(size), &[run], None)
                    .width,
            )
        })
        .fold(0., f32::max)
}

/// tdesktop's two-color userpic gradients, by color index
/// (`ColorIndexToPaletteIndex`: 0, 7, 4, 1, 6, 3, 5).
fn userpic_colors(p: &Palette, index: u8) -> (Color, Color) {
    [
        (p.history_peer1_userpic_bg, p.history_peer1_userpic_bg2),
        (p.history_peer8_userpic_bg, p.history_peer8_userpic_bg2),
        (p.history_peer5_userpic_bg, p.history_peer5_userpic_bg2),
        (p.history_peer2_userpic_bg, p.history_peer2_userpic_bg2),
        (p.history_peer7_userpic_bg, p.history_peer7_userpic_bg2),
        (p.history_peer4_userpic_bg, p.history_peer4_userpic_bg2),
        (p.history_peer6_userpic_bg, p.history_peer6_userpic_bg2),
    ][index as usize % 7]
}

fn name_color(p: &Palette, index: u8) -> Color {
    [
        p.history_peer1_name_fg,
        p.history_peer8_name_fg,
        p.history_peer5_name_fg,
        p.history_peer2_name_fg,
        p.history_peer7_name_fg,
        p.history_peer4_name_fg,
        p.history_peer6_name_fg,
    ][index as usize % 7]
}

/// An empty userpic: gradient circle with initials (`Ui::EmptyUserpic::paint`), or the
/// Saved Messages bookmark (`PaintSavedMessages`).
fn empty_userpic(p: &Palette, size: f32, initials: &str, color: u8, saved: bool) -> gpui::Div {
    let (top, bottom) = if saved {
        (
            p.history_peer_saved_messages_bg,
            p.history_peer_saved_messages_bg2,
        )
    } else {
        userpic_colors(p, color)
    };
    let fg = p.history_peer_userpic_fg;
    let circle = div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(linear_gradient(
            180.,
            linear_color_stop(c(top), 0.),
            linear_color_stop(c(bottom), 1.),
        ))
        .flex()
        .items_center()
        .justify_center();
    if saved {
        circle.child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_bookmark(bounds, size, fg, window),
            )
            .size_full(),
        )
    } else {
        circle
            .text_color(c(fg))
            .text_size(px((size * 13. / 33.).floor()))
            .font_weight(FontWeight::SEMIBOLD)
            .child(initials.to_string())
    }
}

/// The bookmark of tdesktop's `PaintSavedMessagesInner`, computed like there in device
/// pixels of the userpic's size: a round-joined top half and a miter-joined bottom half
/// with the notch, flat caps.
fn paint_bookmark(bounds: gpui::Bounds<Pixels>, size: f32, fg: Color, window: &mut Window) {
    use crate::vector::{Join, Stroke, paint_strokes};
    let scale = window.scale_factor();
    let s = scale::device(size);
    let thickness = (s * 0.055).round();
    let increment = (thickness as i32 % 2 + s as i32 % 2) as f32;
    let width = (s * 0.15).round() * 2. + increment;
    let height = (s * 0.19).round() * 2. + increment;
    let add = (s * 0.064).round();
    let left = ((s - width) / 2.).floor();
    let top = ((s - height) / 2.).floor();
    let (right, bottom) = (left + width, top + height);
    let middle = ((left + right) / 2.).floor();
    let half = ((top + bottom) / 2.).floor();
    // Qt draws this pen so that each line covers whole pixels (a 3 px side is 3 solid
    // pixels in tdesktop): the outline sits half a pixel off the integer grid.
    let (bx, by) = (f32::from(bounds.left()), f32::from(bounds.top()));
    let at = |x: f32, y: f32| (bx + (x + 0.5) / scale, by + (y + 0.5) / scale);
    let upper = [
        at(left, half),
        at(left, top),
        at(right, top),
        at(right, half),
    ];
    let lower = [
        at(left, half),
        at(left, bottom),
        at(middle, bottom - add),
        at(right, bottom),
        at(right, half),
    ];
    let stroke = Stroke::new(thickness / scale);
    paint_strokes(&[&upper], stroke.join(Join::Round), c(fg), window);
    paint_strokes(&[&lower], stroke, c(fg), window);
}

/// `bubble_tail` in style pixels.
const TAIL_SIZE: (f32, f32) = (6., 10.);

/// The bubble tail filled in `bounds` (mirrored for the right side): an image of
/// the tail at its device size (`icons::bubble_tail_image`).
fn paint_tail(bounds: gpui::Bounds<Pixels>, right_side: bool, fill: Rgba, window: &mut Window) {
    if crate::profile::off("tails") {
        return;
    }
    let scale = window.scale_factor();
    let w = (f32::from(bounds.size.width) * scale).round().max(1.) as u32;
    let h = (f32::from(bounds.size.height) * scale).round().max(1.) as u32;
    let to = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    let rgba = (to(fill.r) << 24) | (to(fill.g) << 16) | (to(fill.b) << 8) | to(fill.a);
    let image = crate::icons::bubble_tail_image(right_side, rgba, w, h);
    let pad = crate::icons::TAIL_PAD as f32 / scale;
    let at = gpui::Bounds::new(
        point(bounds.left() - gpui::px(pad), bounds.top() - gpui::px(pad)),
        gpui::size(
            gpui::px((w + 2 * crate::icons::TAIL_PAD) as f32 / scale),
            gpui::px((h + 2 * crate::icons::TAIL_PAD) as f32 / scale),
        ),
    );
    let _ = window.paint_image(at, gpui::Corners::default(), image, 0, false);
}

/// A grey rounded "service" pill (dates, service messages, `msgServiceBg`).
fn service_pill(p: &Palette, text: String, width: f32) -> gpui::Div {
    div()
        .flex_none()
        .w(px(width + 2. * SERVICE_PADDING_X))
        .px(px(SERVICE_PADDING_X))
        .pt(px(SERVICE_PADDING_TOP))
        .pb(px(SERVICE_PADDING_BOTTOM))
        .rounded(px(11.))
        .bg(c(p.msg_service_bg))
        .text_color(c(p.msg_service_fg))
        .font_weight(FontWeight::SEMIBOLD)
        .text_center()
        .child(text)
}

// chat.style: historyFastShareSize (topPeersSelectorUserpicSize), historyFastShareLeft,
// historyFastShareBottom
/// `historyViewsWidth`, `historyViewsSpace`, `historyViewsTop`: the eye of a post's
/// views (its top from the date line's bottom).
const VIEWS_WIDTH: f32 = 20.;
const VIEWS_SPACE: f32 = 8.;
const VIEWS_TOP: f32 = -15.;
/// `historyCommentsButtonHeight`, `historyCommentsSkipLeft/Text/Right`.
const COMMENTS_HEIGHT: f32 = 40.;
const COMMENTS_SKIP_LEFT: f32 = 9.;
const COMMENTS_SKIP_TEXT: f32 = 10.;
const COMMENTS_SKIP_RIGHT: f32 = 8.;
const FAST_SHARE_SIZE: f32 = 31.;
const FAST_SHARE_LEFT: f32 = 13.;
const FAST_SHARE_BOTTOM: f32 = 5.;
/// The sender's userpic in "Forwarded from" (a userpic emoji with
/// `fwdTextUserpicPadding`: 1 px above, 3 px after it).
const FWD_USERPIC: f32 = 17.;
const FWD_USERPIC_SKIP: f32 = 3.;
// basic.style: activeFadeInDuration, activeFadeOutDuration (a jumped-to message)
const HIGHLIGHT_IN: Duration = Duration::from_millis(500);
const HIGHLIGHT_OUT: Duration = Duration::from_millis(3000);
/// basic.style: slideShift, how far the chat underneath moves while another slides in.
const SLIDE_SHIFT: f32 = 100.;
/// `historyTopBarBack`: 52 px wide, the arrow at `infoTopBarBack`'s 11 px.
const BACK_WIDTH: f32 = 52.;
const BACK_ICON_LEFT: f32 = 11.;

fn ease_out_circ(t: f32) -> f32 {
    (1. - (t - 1.) * (t - 1.)).max(0.).sqrt()
}

/// A chat set aside, with its list and scroll position: in the back stack
/// (`SessionController`'s section stack), or drawn while another slides over it.
#[derive(Clone)]
struct SavedPane {
    peer: PeerKey,
    messages: Vec<MessageItem>,
    state: ListState,
    history_complete: bool,
    stick_to_bottom: bool,
    unread_bar: Option<i32>,
    read_till: i32,
    newer_available: bool,
    thread: Option<threads::ThreadView>,
}

/// A chat sliding in from the right (forward) or out to the right (back), as
/// `Window::SlideAnimation` does.
struct Slide {
    /// The other chat: under the arriving one, or the one leaving.
    pane: SavedPane,
    forward: bool,
    start: Instant,
}

/// The message the field replies to: shown over the field ("Reply to Name").
#[derive(Clone)]
struct ReplyDraft {
    msg_id: i32,
    name: String,
    text: String,
}

/// `historyReplyPadding`, `historyReplyTop` / `historyReplyBottom`: a reply's box in
/// a message.
const REPLY_PADDING: [f32; 4] = [11., 2., 6., 2.];
const REPLY_TOP: f32 = 2.;
const REPLY_BOTTOM: f32 = 2.;
/// `historyReplyHeight`, `historyReplySkip`, `historyReplyIconPosition`,
/// `msgReplyPadding.top`: the bar over the field.
const REPLY_BAR_HEIGHT: f32 = 49.;
const REPLY_BAR_SKIP: f32 = 53.;
const REPLY_BAR_ICON: (f32, f32) = (7., 7.);
const REPLY_BAR_TEXT_TOP: f32 = 6.;

/// `MainWindow::setupIntro` / `setupMain` with an animation: the login screens
/// arrive over the account ("Add Account"), or the account over the leaving ones
/// (back, logged in).
pub(crate) struct IntroSlide {
    pub start: Instant,
    pub leaving: Option<Entity<crate::intro::IntroView>>,
}

/// One column: the chat coming over the chat list, or going back
/// (`Window::SlideAnimation`, as `MainWidget` shows a chat in one column).
/// The chat list changing folders from the tabs (`startChatsFilterSlide`,
/// `Ui::SlideAnimation`): the old list's rows as they were shown.
struct FolderSlide {
    start: Instant,
    old: Vec<DialogItem>,
    old_skip: usize,
    old_offset: f32,
    /// The new folder stands left of the old one: the lists move right.
    left: bool,
}

/// `dialogsFilterSlideDuration`.
const FOLDER_SLIDE_DURATION: f32 = 200.;

#[derive(Clone, Copy)]
struct ColumnSlide {
    start: Instant,
    to_chat: bool,
}

/// `windowMinWidth`: the folder column shows from this plus its own width
/// (`enoughSpaceForFilters`), else the folders are tabs over the chat list.
const WINDOW_MIN_WIDTH: f32 = 380.;
/// `titleUnreadCounterTop`: the unread count over the back arrow in one column.
const BACK_UNREAD_TOP: f32 = 6.;

/// What a message list is drawn from: the open chat or a set-aside one.
#[derive(Clone, Copy)]
struct PaneRef<'a> {
    peer: Option<PeerKey>,
    messages: &'a [MessageItem],
    unread_bar: Option<i32>,
    /// The jumped-to message and its highlight's opacity.
    highlight: Option<(i32, f32)>,
    /// Clicks act (the open chat); a sliding copy only shows.
    live: bool,
    /// The thread shown (its replies to the root show no reply box).
    thread_root: Option<i32>,
}

/// `slideDuration`: the "scroll to top" / "scroll down" animations.
const SLIDE_DURATION: Duration = Duration::from_millis(240);
/// `stickersRowDuration`: pinned chats sliding into place while one is dragged.
const PINNED_SHIFT_DURATION: Duration = Duration::from_millis(200);
/// `kStartReorderThreshold`: how far a pressed pinned chat moves before it is dragged.
const REORDER_THRESHOLD: f32 = 30.;

fn sine_in_out(t: f32) -> f32 {
    -((std::f32::consts::PI * t).cos() - 1.) / 2.
}

fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// The pinned chats of `list` (its first rows) put in `order`.
fn apply_pinned_order(list: &mut [DialogItem], order: &[PeerKey]) {
    let pinned = list.iter().take_while(|d| d.pinned).count();
    list[..pinned].sort_by_key(|d| {
        order
            .iter()
            .position(|p| *p == d.peer)
            .unwrap_or(usize::MAX)
    });
}

/// A chat row pressed (left or right button): it may become a pinned-chat drag.
struct RowPress {
    peer: PeerKey,
    start_y: Pixels,
}

/// A pinned chat being dragged (`InnerWidget::_dragging`).
struct PinDrag {
    peer: PeerKey,
    /// The mouse position the row's slot corresponds to (moves with each swap).
    start_y: f32,
    /// The row's offset from its slot, following the mouse.
    yadd: f32,
    /// At the start the row slides from its slot to the mouse (`yadd` animated
    /// from `anim_from` since `anim_start`); swaps meanwhile keep it sliding.
    anim_from: f32,
    anim_start: Option<Instant>,
    start_order: Vec<PeerKey>,
}

impl PinDrag {
    /// The offset drawn now.
    fn shown_yadd(&self) -> f32 {
        match self.anim_start {
            Some(start) if start.elapsed() < PINNED_SHIFT_DURATION => {
                let t = start.elapsed().as_secs_f32() / PINNED_SHIFT_DURATION.as_secs_f32();
                self.anim_from + (self.yadd - self.anim_from) * sine_in_out(t)
            }
            _ => self.yadd,
        }
    }

    fn animating(&self) -> bool {
        self.anim_start
            .is_some_and(|s| s.elapsed() < PINNED_SHIFT_DURATION)
    }
}

/// An animated scroll (`_scrollToAnimation`).
struct ScrollAnim {
    area: Area,
    from: f32,
    to: f32,
    start: Instant,
    ease_out: bool,
}

/// The column edge being dragged: where it started.
struct ColumnDrag {
    start_mouse_x: Pixels,
    start_width: f32,
}

/// The width dates, service messages and the "Unread messages" text are centered in:
/// the whole chat, or in a wide chat (≥ 880 px) the message column at its left edge,
/// where every message is (`ServiceMessagePainter::PaintPreparedDate`,
/// `Service::countGeometry`).
fn service_column(chat_width: f32) -> f32 {
    if chat_width >= CHAT_WIDE_WIDTH {
        chat_width.min(WIDE_CHAT_COLUMN)
    } else {
        chat_width
    }
}

/// Scrollbar drag in progress: where it started.
/// Unlike tdesktop, where the thumb moves by the mouse's movement (so once more
/// history loads and the thumb shrinks away, it can't be dragged further), the thumb
/// stays under the mouse: the grabbed point follows it, also when the list grows.
struct ScrollDrag {
    /// Where in the thumb it was grabbed.
    grab: f32,
    /// The mouse's last position (window coordinates).
    mouse_y: Pixels,
    /// The scrollable height the position was last applied for.
    applied_max: f32,
}

/// One overlay scrollbar (tdesktop's `ScrollArea`): shown while scrolling and on hover.
#[derive(Default)]
/// A value moving linearly to its target (`Ui::Animations::Simple` with the default
/// linear transition).
#[derive(Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    start: Option<Instant>,
    duration: Duration,
}

impl Fade {
    fn new(value: f32, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            start: None,
            duration,
        }
    }

    fn value(&self) -> f32 {
        match self.start {
            Some(start) if start.elapsed() < self.duration => {
                let t = start.elapsed().as_secs_f32() / self.duration.as_secs_f32();
                self.from + (self.to - self.from) * t
            }
            _ => self.to,
        }
    }

    fn animating(&self) -> bool {
        self.start.is_some_and(|s| s.elapsed() < self.duration)
    }

    fn set(&mut self, to: f32) {
        if self.to != to {
            self.from = self.value();
            self.to = to;
            self.start = Some(Instant::now());
        }
    }
}

/// `ScrollArea::duration`: the scrollbar fades and its hover highlights.
const SCROLL_FADE: Duration = Duration::from_millis(150);
/// `historyScrollDateHideTimeout`, `historyDateFadeDuration`.
const SCROLL_DATE_HIDE_AFTER: Duration = Duration::from_millis(1000);
const SCROLL_DATE_FADE: Duration = Duration::from_millis(200);
/// `historyToDownDuration`: the round buttons slide in and out.
const TO_DOWN_DURATION: Duration = Duration::from_millis(150);

/// One overlay scrollbar, as lib_ui's `ScrollBar`: hidden until the mouse enters its
/// area or the area scrolls, then hidden again a second later (at once when the mouse
/// leaves the area), never while the mouse is over the bar; every change fades.
struct Scrollbar {
    /// Shown (1) or hidden (0); `_a_opacity`.
    opacity: Fade,
    /// When the hide timer fires (`_hideTimer`), and its interval (`_hideIn`).
    hide_at: Option<Instant>,
    hide_in: Duration,
    /// The mouse is over the bar's strip (`_over`), over the thumb (`_overbar`).
    over: bool,
    over_bar: bool,
    over_fade: Fade,
    bar_fade: Fade,
    /// The list last scrolled (the floating date goes with it).
    last_scroll: Option<Instant>,
    drag: Option<ScrollDrag>,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            opacity: Fade::new(0., SCROLL_FADE),
            hide_at: None,
            hide_in: SCROLL_HIDE_AFTER,
            over: false,
            over_bar: false,
            over_fade: Fade::new(0., SCROLL_FADE),
            bar_fade: Fade::new(0., SCROLL_FADE),
            last_scroll: None,
            drag: None,
        }
    }
}

impl Scrollbar {
    fn hiding(&self) -> bool {
        self.opacity.to == 0.
    }

    /// `ScrollBar::hideTimeout`: show now (unless `dt` is zero) and hide after `dt`.
    fn hide_timeout(&mut self, dt: Duration) {
        if self.hiding() && !dt.is_zero() {
            self.opacity.set(1.);
        }
        self.hide_in = dt;
        if self.drag.is_none() {
            self.hide_at = Some(Instant::now() + dt);
        }
    }

    /// Fires the hide timer; true while something still animates or waits.
    fn tick(&mut self) -> bool {
        if let Some(at) = self.hide_at
            && Instant::now() >= at
        {
            self.hide_at = None;
            if !self.over && self.drag.is_none() {
                self.opacity.set(0.);
            }
        }
        let moving = self.drag.is_some();
        self.over_fade
            .set(if self.over || moving { 1. } else { 0. });
        self.bar_fade
            .set(if self.over_bar || moving { 1. } else { 0. });
        self.opacity.animating() || self.over_fade.animating() || self.bar_fade.animating()
    }

    /// `setOver`: the mouse entered or left the bar's strip.
    fn set_over(&mut self, over: bool) {
        if self.over == over {
            return;
        }
        self.over = over;
        if over {
            self.hide_at = None;
            if self.hiding() {
                self.opacity.set(1.);
            }
        } else {
            self.over_bar = false;
            if !self.hiding() && self.drag.is_none() {
                self.hide_at = Some(Instant::now() + self.hide_in);
            }
        }
    }
}

/// `anim::color`: a mix of two colors, then faded by `opacity`.
fn mix(a: Rgba, b: Rgba, t: f32, opacity: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: (a.a + (b.a - a.a) * t) * opacity,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    History,
    Dialogs,
    /// Settings (the layer or the section): every scrolled box of this kind.
    Settings,
    /// The emoji panel's list.
    Emoji,
}

impl Area {
    /// The scrollbar's (width, corner radius).
    fn bar_style(self) -> (f32, f32) {
        match self {
            Area::History => (12., 3.),
            // The emoji panel's too: tdesktop's `emojiScroll` (7 px, a 3 px bar)
            // is too thin to grab (docs/ARCHITECTURE.md).
            Area::Dialogs | Area::Settings | Area::Emoji => (10., 2.),
        }
    }
}

pub struct MainView {
    dark: bool,
    dialogs: Vec<DialogItem>,
    dialogs_scroll: UniformListScrollHandle,
    selected: Option<PeerKey>,
    messages: Vec<MessageItem>,
    messages_state: ListState,
    history_complete: bool,
    loading_older: bool,
    /// Keep the newest message in view (true until the user scrolls up, and again once
    /// they scroll back to the bottom).
    stick_to_bottom: bool,
    history_bar: Scrollbar,
    dialogs_bar: Scrollbar,
    settings_bar: Scrollbar,
    emoji_bar: Scrollbar,
    /// The emoji panel or the docked selector.
    emoji_panel: emoji_panel::EmojiPanel,
    /// The open emoji set box.
    emoji_set_box: Option<emoji_set_box::SetBox>,
    /// Emoji suggestions over the message field (shared with it).
    emoji_suggest: std::rc::Rc<std::cell::RefCell<crate::emoji_suggest::Suggest>>,
    /// The set the "Remove" box asks about.
    emoji_set_removing: Option<(tdr_data::emoji_sets::SetRef, String)>,
    /// The chat list's share of the window (tdesktop's dialogs width ratios).
    columns: Columns,
    columns_path: Option<PathBuf>,
    column_drag: Option<ColumnDrag>,
    row_press: Option<RowPress>,
    pin_drag: Option<PinDrag>,
    /// Rows sliding into place after a reorder: offset at the start, start time.
    row_shifts: HashMap<PeerKey, (f32, Instant)>,
    /// The order just set by a drag, kept on incoming lists until they have it.
    pinned_override: Option<(Vec<PeerKey>, Instant)>,
    /// A drag ended with this mouse release: the click it makes must not open a chat.
    suppress_open: bool,
    scroll_anim: Option<ScrollAnim>,
    /// The list length when more chats were last asked for.
    more_dialogs_requested: usize,
    /// The archive's row (`None`: empty), whether it is open, its slide, and where
    /// the chat list was when it opened.
    archive: Option<crate::bridge::ArchiveInfo>,
    archive_open: bool,
    archive_slide: Option<archive::ArchiveSlide>,
    main_list_offset: f32,
    /// The main menu's "Night Mode" row is under the mouse.
    /// Header data of opened chats that are not in the loaded chat list.
    extra_chats: HashMap<PeerKey, DialogItem>,
    /// Show this chat's row once the chat list has it (it joins it when opened).
    reveal_pending: Option<PeerKey>,
    /// Chats left this session with their place, as tdesktop keeps every history's
    /// `scrollTopItem`: opened again, a chat shows where it was. A chat left at its
    /// bottom is not kept (`forgetScrollState`): it opens as usual.
    remembered: HashMap<PeerKey, SavedPane>,
    /// Chats to go back to (`showBackFromStack`), the last on top.
    back_stack: Vec<SavedPane>,
    slide: Option<Slide>,
    /// The open chat shows a window around an older message: newer ones load at its
    /// bottom.
    newer_available: bool,
    loading_newer: bool,
    /// Show this message when the history arrives, and the highlighted one.
    pending_focus: Option<i32>,
    highlight: Option<(i32, Instant)>,
    /// Center this list item once it is laid out.
    center_on: Option<usize>,
    /// The day wallpaper for a chat area of this many device pixels.
    day_background: Option<((u32, u32), Arc<RenderImage>)>,
    day_background_busy: bool,
    /// Opening a chat with unread messages: its `read_inbox_max_id`, until the history
    /// arrives and the view is placed at the first unread message.
    unread_target: Option<i32>,
    /// The "Unread messages" bar goes above this message (while the chat is open).
    unread_bar: Option<i32>,
    /// The newest message id already reported as read.
    read_till: i32,
    window_active: bool,
    /// Member counts from full chat info ("906 members").
    members: HashMap<PeerKey, String>,
    /// Decoded userpics by peer, with their photo id.
    userpics: HashMap<PeerKey, (i64, Arc<RenderImage>)>,
    /// Photos already asked for.
    userpic_requests: RefCell<HashSet<i64>>,
    /// Draft edits made here or pushed from the server since the chat list was last
    /// loaded (`None`: cleared). They win over the list's own `DialogItem::draft`,
    /// which may lag behind the data thread.
    drafts: HashMap<PeerKey, Option<String>>,
    /// The formatting of a chat's unsent text, while that text stays the same (drafts
    /// keep plain text for now).
    draft_tags: HashMap<PeerKey, (String, Vec<TagSpan>)>,
    /// The text the input field had at the last check (to notice typing).
    input_text: String,
    /// Bumped on every edit: pending draft-save timers check it.
    draft_generation: u64,
    /// The first unsaved edit (tdesktop's `_saveDraftStart`).
    draft_dirty_since: Option<Instant>,
    /// The main menu, Settings and boxes over the window (`LayerStackWidget`).
    layers: layers::Layers,
    /// The login's country list, while open.
    country_box: Option<country_box::CountryBox>,
    /// The window's width at the last frame (the layout follows it).
    window_w: f32,
    /// The last chat list of each folder: a folder opens at once with it.
    folder_lists: HashMap<i32, Vec<DialogItem>>,
    column_slide: Option<ColumnSlide>,
    /// The folder tabs (`ChatsFiltersTabs`): the bar's move to the chosen tab, the
    /// tab chosen before the switch happens, the bar's range at the last frame.
    tabs_anim: Option<(Instant, f32, f32)>,
    tabs_chosen: Option<i32>,
    folder_slide: Option<FolderSlide>,
    /// When the main menu's accounts were last shown or hidden.
    menu_accounts_toggled: Option<Instant>,
    tabs_bar: std::cell::Cell<(f32, f32)>,
    /// The caption button under the mouse.
    title_hovered: Option<&'static str>,
    /// The "scroll down" / "scroll to top" button under the mouse.
    jump_hovered: Option<&'static str>,
    /// How far the round buttons have slid in (`CornerButton::animation`).
    to_down_shown: Fade,
    to_up_shown: Fade,
    /// The floating date's opacity (`_scrollDateOpacity`).
    scroll_date: Fade,
    input: Entity<TextInput>,
    /// The chat search field over the chat list.
    search: Entity<TextInput>,
    /// The accounts: their rows in the menu, switching, adding.
    accounts: Entity<crate::accounts::Accounts>,
    /// The login screens (no account yet, or "Add Account"), with the id the new
    /// account gets.
    intro: Option<(Entity<crate::intro::IntroView>, u32)>,
    /// A thread of the open chat shown instead of its history (comments, a topic).
    thread: Option<threads::ThreadView>,
    /// An open forum: its topics beside the narrowed chat list.
    forum: Option<threads::ForumView>,
    /// A closed forum's topics sliding out, and how far they were shown.
    forum_hiding: Option<(threads::ForumView, Instant, f32)>,
    /// The message each chat's field replies to (`FullReplyTo` of the draft).
    reply_to: HashMap<PeerKey, ReplyDraft>,
    /// The login screens sliding in over the account or out under it, with the
    /// leaving login view while it goes.
    pub(crate) intro_slide: Option<IntroSlide>,
    /// The folder column (empty: the account has no folders) and the open folder.
    folders: Vec<FolderItem>,
    active_folder: i32,
    /// The home DC's connection and the "Connecting..." box.
    connection: chrome::Connection,
    /// The icon button under the mouse, and press ripples of buttons.
    button_hovered: Option<SharedString>,
    button_ripples: Vec<chrome::ButtonRipple>,
    /// What the send button shows (send: the field has text) and since when.
    send_switch: (bool, Instant),
    fatal: Option<String>,
    commands: mpsc::UnboundedSender<Command>,
    focus: FocusHandle,
    /// Font with the Windows caption-button glyphs.
    icon_font: SharedString,
    first_list_frame_logged: bool,
    /// Message texts: layouts, selection, clicks, toasts.
    text_ui: message_text::TextUi,
    /// Demo/screenshot helper: open the chat at this list index once the list arrives.
    pub open_on_start: Option<usize>,
}

impl MainView {
    /// The view of one account: `commands` go to its data thread; its updates come
    /// through `accounts` ([`Self::apply`]).
    pub fn new(
        commands: mpsc::UnboundedSender<Command>,
        accounts: Entity<crate::accounts::Accounts>,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&accounts, |_, _, cx| cx.notify()).detach();
        let palette = tdr_theme::palette(dark);
        let input = cx.new(|cx| TextInput::new("Write a message…", palette, cx));
        let emoji_suggest = std::rc::Rc::new(std::cell::RefCell::new(
            crate::emoji_suggest::Suggest::default(),
        ));
        input.update(cx, |i, _| i.enable_emoji_helpers(emoji_suggest.clone()));
        if let Some(dir) = accounts
            .read(cx)
            .emoji_settings_path()
            .and_then(|p| p.parent().map(|d| d.join("emoji-keywords")))
        {
            crate::emoji_suggest::load_keywords(dir);
        }
        let search = cx.new(|cx| {
            let mut field = TextInput::new("Search", palette, cx);
            field.set_single_line(true);
            field
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        // Custom emoji animations and loads repaint the window when their next frame
        // is due (at most `FRAME_MS` apart), instead of on every vsync.
        cx.spawn(async move |this, cx| {
            loop {
                let wait = match crate::repaint::due() {
                    Some(due) => due.saturating_duration_since(Instant::now()),
                    None => Duration::from_millis(50),
                };
                cx.background_executor()
                    .timer(wait.max(Duration::from_millis(4)))
                    .await;
                if crate::repaint::due().is_some_and(|d| d <= Instant::now())
                    && this.update(cx, |_, cx| cx.notify()).is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.observe(&input, |this: &mut Self, _, cx| this.input_changed(cx))
            .detach();
        cx.subscribe(&input, |this: &mut Self, _, FieldMenu(at), cx| {
            this.open_field_menu(*at, cx);
        })
        .detach();
        cx.subscribe(
            &input,
            |this: &mut Self,
             _,
             Submit {
                 text,
                 tags,
                 languages,
             },
             cx| {
                if let Some(peer) = this.selected {
                    // Sending clears the draft (the data thread clears the stored one).
                    this.drafts.insert(peer, None);
                    this.draft_tags.remove(&peer);
                    this.input_text.clear();
                    this.draft_dirty_since = None;
                    this.draft_generation += 1;
                    this.stick_to_bottom = true;
                    // tdesktop drops the bar once you write.
                    this.unread_bar = None;
                    // Sending shows the end of the history (`HistoryWidget::send` scrolls
                    // to the bottom), so the new message is in view when it comes.
                    this.messages_state.scroll_to(ListOffset {
                        item_ix: this.messages.len(),
                        offset_in_item: px(0.),
                    });
                    // In a thread every message replies to its root, or under it.
                    let root = this.thread.as_ref().and_then(|t| t.root);
                    let reply_to = match (this.reply_to.remove(&peer), root) {
                        (Some(r), root) => Some(tdr_data::OutReply {
                            msg_id: r.msg_id,
                            top_id: root,
                        }),
                        (None, Some(root)) => Some(tdr_data::OutReply {
                            msg_id: root,
                            top_id: Some(root),
                        }),
                        (None, None) => None,
                    };
                    let _ = this.commands.send(Command::Send {
                        peer,
                        text: text.clone(),
                        entities: crate::text_input::out_entities_with(text, tags, languages),
                        reply_to,
                    });
                }
                cx.notify();
            },
        )
        .detach();

        // "last seen 5 minutes ago" moves on with the clock.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
                // Also takes online dots off when their time is up.
                let alive = this.update(cx, |_, cx| cx.notify());
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let messages_state = Self::new_messages_state(cx);

        let fonts = cx.text_system().all_font_names();
        let icon_font = ["Segoe Fluent Icons", "Segoe MDL2 Assets"]
            .into_iter()
            .find(|f| fonts.iter().any(|n| n == f))
            .unwrap_or("Segoe MDL2 Assets");

        Self {
            dark,
            dialogs: Vec::new(),
            dialogs_scroll: UniformListScrollHandle::new(),
            selected: None,
            messages: Vec::new(),
            messages_state,
            history_complete: false,
            loading_older: false,
            stick_to_bottom: true,
            history_bar: Scrollbar::default(),
            dialogs_bar: Scrollbar::default(),
            settings_bar: Scrollbar::default(),
            emoji_bar: Scrollbar::default(),
            emoji_panel: emoji_panel::EmojiPanel::new(accounts.read(cx).emoji_settings_path()),
            emoji_set_box: None,
            emoji_suggest,
            emoji_set_removing: None,
            columns: Columns::default(),
            columns_path: None,
            column_drag: None,
            row_press: None,
            pin_drag: None,
            row_shifts: HashMap::new(),
            pinned_override: None,
            suppress_open: false,
            scroll_anim: None,
            more_dialogs_requested: 0,
            archive: None,
            archive_open: false,
            archive_slide: None,
            main_list_offset: 0.,
            extra_chats: HashMap::new(),
            reveal_pending: None,
            remembered: HashMap::new(),
            back_stack: Vec::new(),
            slide: None,
            newer_available: false,
            loading_newer: false,
            pending_focus: None,
            highlight: None,
            center_on: None,
            day_background: None,
            day_background_busy: false,
            text_ui: Default::default(),
            unread_target: None,
            unread_bar: None,
            read_till: 0,
            window_active: true,
            members: HashMap::new(),
            userpics: HashMap::new(),
            userpic_requests: RefCell::new(HashSet::new()),
            drafts: HashMap::new(),
            draft_tags: HashMap::new(),
            input_text: String::new(),
            draft_generation: 0,
            draft_dirty_since: None,
            layers: Default::default(),
            country_box: None,
            window_w: 0.,
            folder_lists: HashMap::new(),
            column_slide: None,
            tabs_anim: None,
            tabs_chosen: None,
            folder_slide: None,
            menu_accounts_toggled: None,
            tabs_bar: std::cell::Cell::new((0., 0.)),
            title_hovered: None,
            jump_hovered: None,
            to_down_shown: Fade::new(0., TO_DOWN_DURATION),
            to_up_shown: Fade::new(0., TO_DOWN_DURATION),
            scroll_date: Fade::new(0., SCROLL_DATE_FADE),
            input,
            search,
            accounts,
            intro: None,
            intro_slide: None,
            reply_to: HashMap::new(),
            thread: None,
            forum: None,
            forum_hiding: None,
            folders: Vec::new(),
            active_folder: 0,
            connection: chrome::Connection::new(),
            button_hovered: None,
            button_ripples: Vec::new(),
            send_switch: (false, Instant::now() - Duration::from_secs(1)),
            fatal: None,
            commands,
            focus: cx.focus_handle(),
            icon_font: icon_font.into(),
            first_list_frame_logged: false,
            open_on_start: None,
        }
    }

    fn palette(&self) -> &'static Palette {
        tdr_theme::palette(self.dark)
    }

    /// A chat's message list: loads older messages near its top, newer ones (in a
    /// window around an older message) at its bottom.
    fn new_messages_state(cx: &mut Context<Self>) -> ListState {
        let state = ListState::new(0, ListAlignment::Bottom, px(600.));
        let view = cx.entity().downgrade();
        state.set_scroll_handler(move |event, _window, cx| {
            let near_top = event.visible_range.start <= LOAD_OLDER_THRESHOLD;
            let at_bottom = event.visible_range.end >= event.count;
            let visible_end = event.visible_range.end;
            let view = view.clone();
            cx.defer(move |cx| {
                let _ = view.update(cx, |v, cx| {
                    v.stick_to_bottom = at_bottom && !v.newer_available;
                    v.stop_scroll_anim(Area::History);
                    v.scrolled(Area::History, cx);
                    v.mark_read(visible_end);
                    if near_top {
                        v.load_older();
                    }
                    if at_bottom {
                        v.load_newer();
                    }
                });
            });
        });
        state
    }

    fn load_newer(&mut self) {
        if self.loading_newer || !self.newer_available {
            return;
        }
        if let Some(peer) = self.selected {
            self.loading_newer = true;
            let _ = self.commands.send(Command::LoadNewer(peer));
        }
    }

    /// A chat's list row, or its header data when it is not in the loaded list.
    fn chat_item(&self, peer: PeerKey) -> Option<&DialogItem> {
        self.dialogs
            .iter()
            .find(|d| d.peer == peer)
            .or_else(|| self.extra_chats.get(&peer))
    }

    /// `InnerWidget::scrollToEntry`: the chat list scrolls just enough to show the
    /// open chat's row.
    fn reveal_in_list(&mut self, peer: PeerKey) {
        let Some(ix) = self.dialogs.iter().position(|d| d.peer == peer) else {
            self.reveal_pending = Some(peer);
            return;
        };
        let ix = ix + self.list_skip();
        self.reveal_pending = None;
        let (_, _, offset) = self.scroll_metrics(Area::Dialogs);
        let first_visible = (offset / lg(DIALOG_ROW_HEIGHT)).floor() as usize;
        let strategy = if ix < first_visible {
            gpui::ScrollStrategy::Top
        } else {
            gpui::ScrollStrategy::Bottom
        };
        self.dialogs_scroll.scroll_to_item(ix, strategy);
    }

    /// The open chat, set aside with its list (a fresh list takes its place).
    fn take_current_pane(&mut self, cx: &mut Context<Self>) -> Option<SavedPane> {
        let peer = self.selected?;
        let state = std::mem::replace(&mut self.messages_state, Self::new_messages_state(cx));
        Some(SavedPane {
            peer,
            messages: std::mem::take(&mut self.messages),
            state,
            history_complete: self.history_complete,
            stick_to_bottom: self.stick_to_bottom,
            unread_bar: self.unread_bar.take(),
            read_till: self.read_till,
            newer_available: self.newer_available,
            thread: self.thread.take(),
        })
    }

    /// Keeps a chat's place for when it is opened again, unless it was left at its
    /// bottom.
    fn remember(&mut self, pane: SavedPane) {
        // A thread is not where its chat opens again.
        if pane.thread.is_some() {
            return;
        }
        if pane.stick_to_bottom {
            self.remembered.remove(&pane.peer);
        } else {
            self.remembered.insert(pane.peer, pane);
        }
    }

    /// The open chat is left (another one opens, or Esc): its place is kept.
    fn leave_current(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.take_current_pane(cx) {
            self.remember(pane);
        }
        self.selected = None;
    }

    /// A set-aside chat open again, where it was scrolled.
    fn restore_pane(&mut self, pane: SavedPane) {
        self.selected = Some(pane.peer);
        self.messages = pane.messages;
        self.messages_state = pane.state;
        self.history_complete = pane.history_complete;
        self.stick_to_bottom = pane.stick_to_bottom;
        self.unread_bar = pane.unread_bar;
        self.read_till = pane.read_till;
        self.newer_available = pane.newer_available;
        self.thread = pane.thread;
        self.unread_target = None;
        self.loading_older = false;
        self.loading_newer = false;
        self.pending_focus = None;
        self.highlight = None;
        self.center_on = None;
        self.scroll_anim = None;
        self.history_bar.drag = None;
    }

    /// The "go to original" button: the chat the message was saved from slides in,
    /// open at that message (`showPeerHistory(..., Way::Forward, msgId)`); the current
    /// one waits in the back stack.
    fn go_to_original(
        &mut self,
        peer: PeerKey,
        msg_id: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected == Some(peer) {
            if msg_id.is_some() {
                self.pending_focus = msg_id;
                self.stick_to_bottom = false;
                let _ = self.commands.send(Command::OpenChatAt { peer, msg_id });
                cx.notify();
            }
            return;
        }
        self.save_draft(cx);
        if let Some(pane) = self.take_current_pane(cx) {
            self.back_stack.push(pane.clone());
            self.slide = Some(Slide {
                pane,
                forward: true,
                start: Instant::now(),
            });
        }
        self.restore_draft(peer, cx);
        let read_till = self
            .dialogs
            .iter()
            .find(|d| d.peer == peer)
            .map_or(0, |d| d.read_inbox_max_id);
        self.selected = Some(peer);
        self.history_complete = false;
        self.loading_older = false;
        self.loading_newer = false;
        self.newer_available = false;
        self.stick_to_bottom = false;
        self.unread_target = None;
        self.unread_bar = None;
        self.read_till = read_till;
        self.pending_focus = msg_id;
        // At its latest messages when no message is asked for.
        self.stick_to_bottom = msg_id.is_none();
        self.highlight = None;
        self.scroll_anim = None;
        self.history_bar.drag = None;
        let _ = self.commands.send(Command::OpenChatAt { peer, msg_id });
        self.reveal_in_list(peer);
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// The header's back arrow (or Esc): the previous chat slides back in from the
    /// left, where it was, and this one leaves to the right.
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(previous) = self.back_stack.pop() else {
            return;
        };
        self.save_draft(cx);
        let closing = self.take_current_pane(cx);
        let peer = previous.peer;
        self.restore_pane(previous);
        if let Some(pane) = closing {
            self.remember(pane.clone());
            self.slide = Some(Slide {
                pane,
                forward: false,
                start: Instant::now(),
            });
        }
        self.restore_draft(peer, cx);
        // The data side gives back the same history: the list keeps its position.
        let _ = self.commands.send(Command::Back);
        self.reveal_in_list(peer);
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// The open chat at its newest messages again (from a window around an older one).
    fn jump_to_latest(&mut self) {
        if let Some(peer) = self.selected {
            self.stick_to_bottom = true;
            self.newer_available = false;
            let _ = self.commands.send(Command::OpenChat(peer));
        }
    }

    /// The highlight of a jumped-to message (`ElementHighlighter`): fades in, then
    /// slowly out.
    fn highlight_opacity(&self) -> Option<(i32, f32)> {
        let (id, start) = self.highlight?;
        let t = start.elapsed();
        let opacity = if t < HIGHLIGHT_IN {
            t.as_secs_f32() / HIGHLIGHT_IN.as_secs_f32()
        } else if t < HIGHLIGHT_IN + HIGHLIGHT_OUT {
            1. - (t - HIGHLIGHT_IN).as_secs_f32() / HIGHLIGHT_OUT.as_secs_f32()
        } else {
            return None;
        };
        Some((id, opacity))
    }

    /// Where the chat list widths are kept; loads them.
    pub fn set_columns_path(&mut self, path: PathBuf) {
        self.columns = Columns::load(&path);
        self.columns_path = Some(path);
    }

    /// The chat list's width in this window.
    /// The folder column's width (0 without folders).
    /// The folder column fits (`enoughSpaceForFilters`); else the folders are tabs.
    fn filters_vertical(&self) -> bool {
        !self.folders.is_empty() && self.window_w >= lg(WINDOW_MIN_WIDTH + chrome::FILTERS_WIDTH)
    }

    fn filters_width(&self) -> f32 {
        if self.filters_vertical() {
            lg(chrome::FILTERS_WIDTH)
        } else {
            0.
        }
    }

    /// `computeColumnLayout`: narrower than the two columns' minimums, the window
    /// shows the chat list or a chat (`Adaptive::WindowLayout::OneColumn`).
    fn one_column(&self) -> bool {
        self.window_w - self.filters_width() < lg(COLUMN_MIN_LEFT) + lg(COLUMN_MIN_MAIN)
    }

    fn dialogs_width(&self, window: &Window) -> f32 {
        let body = f32::from(window.viewport_size().width) - self.filters_width();
        if self.one_column() {
            return body;
        }
        self.columns.width(self.selected.is_none(), body)
    }

    /// The chat list shows userpics only (tdesktop's `narrow` row context; never in
    /// one column).
    fn narrow(&self, window: &Window) -> bool {
        // An open forum narrows the chat list to userpics beside its topics (once
        // they slid in: until then they cover the wide list).
        self.forum
            .as_ref()
            .is_some_and(|f| f.start.elapsed() >= SLIDE_DURATION)
            || (!self.one_column() && self.dialogs_width(window) < COLUMN_MIN_LEFT / 2.)
    }

    /// The open chat's width.
    fn chat_width(&self, window: &Window) -> f32 {
        let body = f32::from(window.viewport_size().width) - self.filters_width();
        if self.one_column() {
            return body;
        }
        body - self.dialogs_width(window) - self.emoji_section_width(window)
    }

    /// In one column, the chat goes back to the list with the slide; the chat closes
    /// when it is done.
    fn start_column_back(&mut self) {
        if self.column_slide.is_some_and(|s| !s.to_chat) {
            return;
        }
        self.column_slide = Some(ColumnSlide {
            start: Instant::now(),
            to_chat: false,
        });
    }

    /// The back arrow over the chat: the previous chat, else (one column) the list.
    fn header_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.back_stack.is_empty() {
            self.go_back(window, cx);
        } else if self.one_column() {
            self.start_column_back();
            cx.notify();
        }
    }

    /// `unreadBadgeIgnoreOne`: the unread count of the other chats on the back arrow,
    /// and whether they are all muted.
    fn others_unread_badge(&self, chat: &DialogItem, cx: &gpui::App) -> Option<(u32, bool)> {
        let state = self.accounts.read(cx).active().map(|s| s.state)?;
        let own = chat.unread.max(0) as u32;
        let count = state.unread.saturating_sub(own);
        (count > 0).then_some((count, state.unread_muted))
    }

    /// The chat list during a folder change from the tabs: `Ui::SlideAnimation`'s
    /// frame, the arriving list moving with `easeOutCirc` and appearing with
    /// `easeInCirc`, the departing one the other way round.
    fn folder_slide_view(
        &mut self,
        live: AnyElement,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(slide) = self.folder_slide.take() else {
            return live;
        };
        let t = slide.start.elapsed().as_secs_f32() * 1000. / FOLDER_SLIDE_DURATION;
        if t >= 1. {
            return live;
        }
        window.request_animation_frame();
        let p = self.palette();
        let (out, inn) = (crate::anim::ease_out_circ(t), crate::anim::ease_in_circ(t));
        let (new_x, old_x) = if slide.left {
            (-width * (1. - out), width * inn)
        } else {
            (width * (1. - out), -width * inn)
        };
        let row = lg(DIALOG_ROW_HEIGHT);
        let height = f32::from(window.viewport_size().height);
        let first = (slide.old_offset / row).floor().max(0.) as usize;
        let mut rows = Vec::new();
        let mut i = first;
        loop {
            let top = i as f32 * row - slide.old_offset;
            if top >= height {
                break;
            }
            let element = if i < slide.old_skip {
                self.render_archive_row(window, cx)
            } else if let Some(item) = slide.old.get(i - slide.old_skip) {
                self.render_dialog(item, false, window, cx)
            } else {
                break;
            };
            rows.push(
                div()
                    .absolute()
                    .left_0()
                    .top(gpui::px(top))
                    .w_full()
                    .h(px(DIALOG_ROW_HEIGHT))
                    .child(element),
            );
            i += 1;
        }
        self.folder_slide = Some(slide);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(c(p.dialogs_bg))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(old_x))
                    .w(gpui::px(width))
                    .h_full()
                    .opacity(1. - out)
                    .children(rows),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(new_x))
                    .w(gpui::px(width))
                    .h_full()
                    .opacity(inn)
                    .child(live),
            )
            .into_any_element()
    }

    /// The list under, the chat over it sliding in from the right or out
    /// (`Window::SlideAnimation` without fade: the list shifts `slideShift` and
    /// darkens, the chat casts its shadow).
    fn column_slide_view(
        &self,
        under: gpui::Div,
        over: gpui::Div,
        slide: ColumnSlide,
        width: f32,
        window: &mut Window,
    ) -> gpui::Div {
        let p = self.palette();
        let t = (slide.start.elapsed().as_secs_f32() / SLIDE_DURATION.as_secs_f32()).min(1.);
        let progress = if slide.to_chat {
            ease_out_circ(t)
        } else {
            1. - ease_out_circ(t)
        };
        let coord_over = width * (1. - progress);
        let coord_under = -lg(SLIDE_SHIFT) * progress;
        window.request_animation_frame();
        div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(coord_under))
                    .w(gpui::px(width))
                    .h_full()
                    .flex()
                    .child(under),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(gpui::px(coord_over))
                    .h_full()
                    .bg(c(p.slide_fade_out_bg))
                    .opacity(progress),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(coord_over))
                    .w(gpui::px(width))
                    .h_full()
                    .flex()
                    .child(over),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(
                        coord_over - crate::icons::icon_width(Icon::SlideShadow, window),
                    ))
                    .h_full()
                    .opacity(progress)
                    .child(crate::icons::stretched(
                        Icon::SlideShadow,
                        p.slide_fade_out_shadow_fg,
                        window,
                    )),
            )
    }

    fn dragging(&self) -> bool {
        self.column_drag.is_some()
            || self.pin_drag.is_some()
            || self.history_bar.drag.is_some()
            || self.dialogs_bar.drag.is_some()
            || self.settings_bar.drag.is_some()
    }

    /// The mouse moved while something is dragged: wherever it is, like Qt's mouse grab
    /// in tdesktop.
    fn drag_moved(&mut self, e: &gpui::MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        if e.pressed_button.is_none() {
            // Released outside the window: no mouse up came.
            self.drag_ended(window, cx);
            return;
        }
        if self.pin_drag.is_some() {
            self.update_pin_drag(f32::from(e.position.y), cx);
        }
        if let Some(drag) = &self.column_drag {
            let width = drag.start_width + f32::from(e.position.x - drag.start_mouse_x);
            // The width the column is shown in: without the folders (`MainWidget`).
            let body = f32::from(window.viewport_size().width) - self.filters_width();
            self.columns.drag_to(width, self.selected.is_none(), body);
            cx.notify();
        }
        self.drag_scrollbar(Area::History, e.position.y, cx);
        self.drag_scrollbar(Area::Dialogs, e.position.y, cx);
        self.drag_scrollbar(Area::Settings, e.position.y, cx);
        self.drag_scrollbar(Area::Emoji, e.position.y, cx);
    }

    fn drag_ended(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.row_press = None;
        self.finish_pin_drag(cx);
        if self.column_drag.take().is_some() {
            let body = f32::from(window.viewport_size().width) - self.filters_width();
            self.columns.drag_finished(self.selected.is_none(), body);
            if let Some(path) = &self.columns_path {
                self.columns.save(path);
            }
            cx.notify();
        }
        self.end_drag(Area::History, cx);
        self.end_drag(Area::Dialogs, cx);
        self.end_drag(Area::Settings, cx);
        self.end_drag(Area::Emoji, cx);
    }

    fn pinned_order(&self) -> Vec<PeerKey> {
        self.dialogs
            .iter()
            .take_while(|d| d.pinned)
            .map(|d| d.peer)
            .collect()
    }

    fn row_pressed(&mut self, peer: PeerKey, y: Pixels) {
        self.row_press = Some(RowPress { peer, start_y: y });
        self.suppress_open = false;
    }

    /// `checkReorderPinnedStart`: a pressed pinned chat moved far enough vertically.
    fn row_press_moved(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(press) = &self.row_press else { return };
        if self.pin_drag.is_some() || f32::from(y - press.start_y).abs() < REORDER_THRESHOLD {
            return;
        }
        let (peer, start_y) = (press.peer, f32::from(press.start_y));
        self.row_press = None;
        let order = self.pinned_order();
        if order.len() < 2 || !order.contains(&peer) {
            return;
        }
        // `startReorderPinned`: yadd animates from 0 to the mouse.
        self.pin_drag = Some(PinDrag {
            peer,
            start_y,
            yadd: 0.,
            anim_from: 0.,
            anim_start: Some(Instant::now()),
            start_order: order,
        });
        self.update_pin_drag(f32::from(y), cx);
    }

    /// The current offset of a row sliding into place.
    fn row_shift(&self, peer: PeerKey) -> f32 {
        match self.row_shifts.get(&peer) {
            Some((from, start)) => {
                let t = start.elapsed().as_secs_f32() / PINNED_SHIFT_DURATION.as_secs_f32();
                if t >= 1. {
                    0.
                } else {
                    from * (1. - sine_in_out(t))
                }
            }
            None => 0.,
        }
    }

    /// `updateReorderPinned`: the dragged chat swaps places with a neighbour once it
    /// covers half of it; the neighbour slides into the freed slot.
    fn update_pin_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(mut drag) = self.pin_drag.take() else {
            return;
        };
        let pinned = self.dialogs.iter().take_while(|d| d.pinned).count();
        let Some(mut index) = self.dialogs[..pinned]
            .iter()
            .position(|d| d.peer == drag.peer)
        else {
            return;
        };
        let height = lg(DIALOG_ROW_HEIGHT);
        let threshold = (height / 2.).max(height - height / 2.);
        let now = Instant::now();
        let shown_was = drag.shown_yadd();
        let start_was = drag.start_y;
        while index > 0 && drag.start_y - y >= threshold {
            self.dialogs.swap(index, index - 1);
            let displaced = self.dialogs[index].peer;
            let from = self.row_shift(displaced) - height;
            self.row_shifts.insert(displaced, (from, now));
            index -= 1;
            drag.start_y -= height;
        }
        while index + 1 < pinned && y - drag.start_y >= threshold {
            self.dialogs.swap(index, index + 1);
            let displaced = self.dialogs[index].peer;
            let from = self.row_shift(displaced) + height;
            self.row_shifts.insert(displaced, (from, now));
            index += 1;
            drag.start_y += height;
        }
        drag.yadd = y - drag.start_y;
        // `yadd = anim::value(yaddWas - shiftHeight, ...)`: a running slide goes on
        // from where it was drawn.
        if drag.animating() {
            drag.anim_from = shown_was - (drag.start_y - start_was);
        }
        self.pin_drag = Some(drag);
        cx.notify();
    }

    /// The dragged chat is released: it slides into its slot, and a new order goes to
    /// the server (`savePinnedOrder`). No chat opens.
    fn finish_pin_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.pin_drag.take() else {
            return;
        };
        self.row_shifts
            .insert(drag.peer, (drag.shown_yadd(), Instant::now()));
        self.suppress_open = true;
        let order = self.pinned_order();
        if order != drag.start_order {
            self.pinned_override = Some((order.clone(), Instant::now()));
            let _ = self.commands.send(Command::ReorderPinned(order));
        }
        cx.notify();
    }

    /// Scrolls `area` to `to` the way tdesktop does: from at most one screen away, in
    /// `slideDuration` (a longer way jumps first).
    fn start_scroll_anim(&mut self, area: Area, to: f32) {
        let (viewport, max, offset) = self.scroll_metrics(area);
        let to = to.clamp(0., max.max(0.));
        let mut from = offset;
        let mut ease_out = false;
        if (to - from).abs() > viewport {
            from = if to > from {
                to - viewport
            } else {
                to + viewport
            };
            self.set_scroll(area, from);
            // The chat ends a jump with easeOutCubic, the chat list keeps sineInOut.
            ease_out = area == Area::History;
        }
        self.scroll_anim = Some(ScrollAnim {
            area,
            from,
            to,
            start: Instant::now(),
            ease_out,
        });
    }

    /// One frame of the scroll animation.
    fn tick_scroll_anim(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(a) = &self.scroll_anim else { return };
        let t = (a.start.elapsed().as_secs_f32() / SLIDE_DURATION.as_secs_f32()).min(1.);
        let eased = if a.ease_out {
            ease_out_cubic(t)
        } else {
            sine_in_out(t)
        };
        let (area, to) = (a.area, a.to);
        self.set_scroll(area, a.from + (a.to - a.from) * eased);
        self.bar_mut(area).last_scroll = Some(Instant::now());
        if t < 1. {
            window.request_animation_frame();
            return;
        }
        self.scroll_anim = None;
        if area == Area::History {
            let (_, max, _) = self.scroll_metrics(area);
            if to >= max - 1. && !self.messages.is_empty() {
                self.stick_to_bottom = true;
                self.messages_state
                    .scroll_to_reveal_item(self.messages.len() - 1);
                self.mark_read(self.messages.len());
            }
        }
        self.scrolled(area, cx);
    }

    /// The user scrolled `area` by hand: an animation there stops.
    fn stop_scroll_anim(&mut self, area: Area) {
        if self.scroll_anim.as_ref().is_some_and(|a| a.area == area) {
            self.scroll_anim = None;
        }
    }

    /// While dragging: one layer over the whole window takes the mouse (no hover effects
    /// underneath, the drag cursor everywhere), and window-level listeners follow the
    /// mouse even outside it.
    fn render_drag_capture(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.dragging() {
            return None;
        }
        let view = cx.entity().downgrade();
        let cursor = if self.column_drag.is_some() {
            gpui::CursorStyle::ResizeLeftRight
        } else {
            gpui::CursorStyle::Arrow
        };
        Some(
            div()
                .id("drag-capture")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .cursor(cursor)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let moved = view.clone();
                            window.on_mouse_event(
                                move |e: &gpui::MouseMoveEvent, phase, window, cx| {
                                    if phase == gpui::DispatchPhase::Capture {
                                        let _ = moved
                                            .update(cx, |this, cx| this.drag_moved(e, window, cx));
                                    }
                                },
                            );
                            let released = view.clone();
                            window.on_mouse_event(
                                move |e: &gpui::MouseUpEvent, phase, window, cx| {
                                    if phase == gpui::DispatchPhase::Capture
                                        && matches!(
                                            e.button,
                                            MouseButton::Left | MouseButton::Right
                                        )
                                    {
                                        let _ = released
                                            .update(cx, |this, cx| this.drag_ended(window, cx));
                                    }
                                },
                            );
                        },
                    )
                    .size_full(),
                ),
        )
    }

    /// The chat background (under messages and the empty-chat placeholder): the night
    /// theme's solid color, or the day wallpaper (`day_background`).
    fn with_chat_background(&self, d: gpui::Div, day: Option<AnyElement>) -> gpui::Div {
        match day {
            Some(day) => d.relative().overflow_hidden().child(day),
            None => d.bg(c(tdr_theme::NIGHT_CHAT_BACKGROUND)),
        }
    }

    /// The day wallpaper, made for the whole area below the title bar right of the chat
    /// list (as tdesktop fills the main section) and shown from `top` down. It is drawn
    /// for the exact pixel size in the background; meanwhile the last one, or the bare
    /// gradient, is stretched over the area.
    fn day_background(
        &mut self,
        top: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.dark {
            return None;
        }
        let width = self.chat_width(window);
        let height = f32::from(window.viewport_size().height) - TITLE_HEIGHT;
        let scale = window.scale_factor();
        let size = (
            (width * scale).round().max(1.) as u32,
            (height * scale).round().max(1.) as u32,
        );
        if self.day_background.as_ref().is_none_or(|(s, _)| *s != size) && !self.day_background_busy
        {
            self.day_background_busy = true;
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { crate::wallpaper::day_background(size.0, size.1) })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.day_background_busy = false;
                    if let Some((_, old)) = view.day_background.replace((size, Arc::new(image))) {
                        // Its texture would stay in the atlas otherwise.
                        cx.drop_image(old, None);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        let element = match &self.day_background {
            Some((_, image)) => img(image.clone())
                .absolute()
                .left_0()
                .top(px(-top))
                .w(px(width))
                .h(px(height))
                .object_fit(gpui::ObjectFit::Fill)
                .into_any_element(),
            None => div()
                .absolute()
                .left_0()
                .top(px(-top))
                .w(px(width))
                .h(px(height))
                .overflow_hidden()
                .child(crate::wallpaper::default_gradient())
                .into_any_element(),
        };
        Some(element)
    }

    /// The window gained or lost focus: messages count as seen only in an active window.
    pub fn window_activation_changed(&mut self, active: bool) {
        self.window_active = active;
        if active && self.stick_to_bottom {
            self.mark_read(self.messages.len());
        }
    }

    pub(crate) fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::CustomEmoji { doc, bytes } => {
                crate::custom_emoji::loaded(&doc, bytes);
                cx.notify();
            }
            Update::CustomEmojiFailed(id) => crate::custom_emoji::failed(id),
            Update::EmojiKeywords {
                remote,
                lang,
                version,
                changes,
            } => {
                let changes = changes
                    .into_iter()
                    .map(|c| {
                        if c.added {
                            tdr_emoji::keywords::Change::Add {
                                keyword: c.keyword,
                                emoticons: c.emoticons,
                            }
                        } else {
                            tdr_emoji::keywords::Change::Delete {
                                keyword: c.keyword,
                                emoticons: c.emoticons,
                            }
                        }
                    })
                    .collect();
                crate::emoji_suggest::apply_difference(remote, &lang, version, changes);
                self.emoji_keywords_came(cx);
            }
            Update::EmojiSets {
                installed,
                featured,
            } => {
                self.emoji_panel.set_sets(installed, featured);
                cx.notify();
            }
            Update::EmojiSetLoaded { set, found } => {
                self.emoji_set_loaded(set, found);
                cx.notify();
            }
            Update::Archive(info) => {
                // An emptied archive closes (`Widget::refreshFolderTopBar`).
                if info.is_none() && self.archive_open {
                    self.archive_open = false;
                    self.dialogs = self
                        .folder_lists
                        .get(&self.active_folder)
                        .cloned()
                        .unwrap_or_default();
                    let _ = self.commands.send(Command::CloseArchive);
                }
                self.archive = info;
            }
            Update::Dialogs { folder, mut list } => {
                // `Data::ChatFilters`: every folder's list is kept; a list arriving for
                // another folder than the open one waits there.
                self.folder_lists.insert(folder, list.clone());
                if folder != self.shown_list_id() {
                    return;
                }
                // The open chat stays open when the list no longer has it (another
                // folder is shown): its header data is kept aside.
                if let Some(peer) = self.selected
                    && !list.iter().any(|d| d.peer == peer)
                    && let Some(item) = self.dialogs.iter().find(|d| d.peer == peer)
                {
                    self.extra_chats.insert(peer, item.clone());
                }
                if let Some((order, at)) = &self.pinned_override {
                    let current: Vec<PeerKey> = list
                        .iter()
                        .take_while(|d| d.pinned)
                        .map(|d| d.peer)
                        .collect();
                    if current == *order || at.elapsed() > Duration::from_secs(5) {
                        self.pinned_override = None;
                    } else {
                        apply_pinned_order(&mut list, order);
                    }
                }
                if self.pin_drag.is_some() {
                    apply_pinned_order(&mut list, &self.pinned_order());
                }
                if self.dialogs.is_empty() {
                    tracing::info!(elapsed = ?crate::STARTED.elapsed(), chats = list.len(), "ui: chat list received");
                }
                self.dialogs = list;
                if let Some(peer) = self.reveal_pending
                    && self.selected == Some(peer)
                {
                    self.reveal_in_list(peer);
                }
                if let Some(i) = self.open_on_start.take()
                    && let Some(peer) = self.dialogs.get(i).map(|d| d.peer)
                {
                    self.start_opening(peer);
                }
            }
            Update::ThreadCount { peer, root, count } => {
                if self.selected == Some(peer)
                    && let Some(t) = &mut self.thread
                    && t.root.is_none_or(|r| r == root)
                {
                    t.count = Some(count);
                }
            }
            Update::Topics { peer, topics } => {
                if let Some(forum) = &mut self.forum
                    && forum.peer == peer
                {
                    forum.topics = Some(topics);
                }
            }
            Update::History {
                peer,
                messages,
                complete,
                newer,
                focus,
                thread,
            } => {
                // A thread's history goes to the thread view, the chat's to the chat.
                let ours = match (&mut self.thread, thread) {
                    (None, None) => true,
                    (Some(t), Some(root)) if t.root.is_none_or(|r| r == root) => {
                        t.root = Some(root);
                        true
                    }
                    _ => false,
                };
                if self.selected == Some(peer) && ours {
                    self.history_complete = complete;
                    self.loading_older = false;
                    self.loading_newer = false;
                    self.newer_available = newer;
                    if newer {
                        self.stick_to_bottom = false;
                    }
                    if let Some(id) = focus {
                        if messages.iter().any(|m| m.id == id) {
                            self.pending_focus = Some(id);
                            self.unread_target = None;
                            self.stick_to_bottom = false;
                        } else {
                            // `countInitialScrollTop`: shown as usual, with a toast.
                            self.pending_focus = None;
                            self.show_toast("Message doesn't exist.", None);
                        }
                    }
                    self.set_messages(messages);
                    // A short chat may not fill the view, so no scroll would ever ask for more.
                    if self.messages.len() < 20 {
                        self.load_older();
                    }
                }
            }
            Update::Draft { peer, text } => {
                // tdesktop's `applyCloudDraft`: the open chat's field shows it at once.
                self.drafts.insert(peer, text.clone());
                if self.selected == Some(peer) {
                    self.draft_generation += 1;
                    self.draft_dirty_since = None;
                    let text = text.unwrap_or_default();
                    let tags = self.tags_of(peer, &text);
                    self.set_input_text_with_tags(text, tags, cx);
                }
            }
            Update::ChatInfo(item) => {
                self.extra_chats.insert(item.peer, *item);
            }
            Update::Members { peer, text } => {
                self.members.insert(peer, text);
            }
            Update::Userpic {
                peer,
                photo_id,
                image,
            } => {
                self.userpics.insert(peer, (photo_id, image));
            }
            Update::Connected(connected) => self.set_connected(connected, cx),
            // The accounts keep these (the menu reads them there).
            Update::Me(_) | Update::AccountState(_) => {}
            Update::Folders(list) => {
                if !list.iter().any(|f| f.id == self.active_folder) {
                    self.active_folder = 0;
                }
                self.folders = list;
            }
            Update::Toast(text) => self.show_toast(&text, None),
            Update::PhoneResolved { phone, user } => self.phone_resolved(&phone, user),
            Update::BankCard { number, data } => self.bank_card_resolved(&number, data),
            Update::Resolved { peer, msg_id } => {
                self.text_ui.pending_open = Some((peer, msg_id));
            }
            Update::Fatal(e) => self.fatal = Some(e),
            // `Accounts` takes the account away.
            Update::LoggedOut => {}
        }
        cx.notify();
    }

    /// Replaces the shown messages, keeping the scroll position (or the bottom). The
    /// first history of a chat with unread messages opens at the first unread one with
    /// the "Unread messages" bar, as in tdesktop.
    fn set_messages(&mut self, new: Vec<MessageItem>) {
        let mut reveal = None;
        // A jumped-to message: shown in the middle, highlighted.
        if let Some(id) = self.pending_focus
            && let Some(ix) = new.iter().position(|m| m.id == id)
        {
            self.pending_focus = None;
            self.highlight = Some((id, Instant::now()));
            self.center_on = Some(ix);
            reveal = Some(ix);
        }
        if let Some(read_till) = self.unread_target
            && !new.is_empty()
        {
            self.unread_target = None;
            if let Some(ix) = new.iter().position(|m| m.id > read_till && !m.out) {
                self.unread_bar = Some(new[ix].id);
                self.stick_to_bottom = false;
                reveal = Some(ix);
            }
        }
        self.update_list(new);
        if let Some(ix) = reveal {
            self.messages_state.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
        } else if self.stick_to_bottom && !self.messages.is_empty() {
            // Past the last item: the bottom-aligned list then follows its end, also
            // while the new items get their real heights (revealing the last item
            // would use its estimated height and cut it).
            self.messages_state.scroll_to(ListOffset {
                item_ix: self.messages.len(),
                offset_in_item: px(0.),
            });
            self.mark_read(self.messages.len());
        }
    }

    /// Tells the list exactly what changed: older messages prepended, new ones appended,
    /// edits in place. (A full reset would lose the position.) Neighbours of changed
    /// messages are re-measured too: grouping and date dividers depend on them.
    fn update_list(&mut self, new: Vec<MessageItem>) {
        let old = std::mem::take(&mut self.messages);
        let same_block = old.first().and_then(|first| {
            let k = new.iter().position(|m| m.id == first.id)?;
            let matches = new.len() >= k + old.len()
                && new[k..k + old.len()]
                    .iter()
                    .zip(&old)
                    .all(|(a, b)| a.id == b.id);
            matches.then_some(k)
        });
        match same_block {
            Some(prepended) => {
                let changed: Vec<usize> = (0..old.len())
                    .filter(|&i| new[prepended + i] != old[i])
                    .collect();
                let appended = new.len() - prepended - old.len();
                let old_len = old.len();
                self.messages = new;
                if prepended > 0 {
                    // The old first message may now be attached to the new previous one.
                    self.messages_state.splice(0..0, prepended);
                    self.messages_state.splice(prepended..prepended + 1, 1);
                }
                for i in changed {
                    let at = prepended + i;
                    self.messages_state.splice(at..at + 1, 1);
                }
                if appended > 0 {
                    let at = prepended + old_len;
                    // The old last message loses its avatar if the new one continues the group.
                    if at > 0 {
                        self.messages_state.splice(at - 1..at, 1);
                    }
                    self.messages_state.splice(at..at, appended);
                }
            }
            None => {
                let top = self.messages_state.logical_scroll_top();
                let at_bottom = top.item_ix >= old.len();
                let len = new.len();
                self.messages = new;
                self.messages_state.reset(len);
                if !at_bottom && len > 0 {
                    self.messages_state.scroll_to(ListOffset {
                        item_ix: top.item_ix.min(len - 1),
                        offset_in_item: top.offset_in_item,
                    });
                }
            }
        }
    }

    /// The messages before `visible_end` were seen: tell the server (tdesktop's
    /// `readInboxTill`), if the window is active.
    fn mark_read(&mut self, visible_end: usize) {
        let Some(peer) = self.selected else { return };
        if self.thread.is_some() {
            return;
        }
        if !self.window_active || visible_end == 0 {
            return;
        }
        let Some(last) = self.messages.get(visible_end.min(self.messages.len()) - 1) else {
            return;
        };
        let has_unread = self.messages[..visible_end.min(self.messages.len())]
            .iter()
            .any(|m| !m.out && m.id > self.read_till);
        if last.id > self.read_till && has_unread {
            self.read_till = last.id;
            let _ = self.commands.send(Command::Read {
                peer,
                max_id: last.id,
            });
        }
    }

    fn load_older(&mut self) {
        if self.loading_older || self.history_complete || self.messages.is_empty() {
            return;
        }
        if let Some(peer) = self.selected {
            self.loading_older = true;
            let _ = self.commands.send(Command::LoadOlder(peer));
        }
    }

    fn bar_mut(&mut self, area: Area) -> &mut Scrollbar {
        match area {
            Area::History => &mut self.history_bar,
            Area::Dialogs => &mut self.dialogs_bar,
            Area::Settings => &mut self.settings_bar,
            Area::Emoji => &mut self.emoji_bar,
        }
    }

    fn bar(&self, area: Area) -> &Scrollbar {
        match area {
            Area::History => &self.history_bar,
            Area::Dialogs => &self.dialogs_bar,
            Area::Settings => &self.settings_bar,
            Area::Emoji => &self.emoji_bar,
        }
    }

    /// A list scrolled: show its scrollbar (and the floating date) for a moment
    /// (`ScrollArea::scrolled` -> `hideTimeout(st.hiding)`).
    fn scrolled(&mut self, area: Area, cx: &mut Context<Self>) {
        let bar = self.bar_mut(area);
        bar.last_scroll = Some(Instant::now());
        bar.hide_timeout(SCROLL_HIDE_AFTER);
        cx.notify();
        Self::wake_after(SCROLL_HIDE_AFTER, cx);
    }

    /// The mouse entered or left a list (`ScrollArea::enterEventHook/leaveEventHook`):
    /// its scrollbar shows for a second, or hides at once.
    fn area_hovered(&mut self, area: Area, hovered: bool, cx: &mut Context<Self>) {
        let bar = self.bar_mut(area);
        if hovered {
            bar.hide_timeout(SCROLL_HIDE_AFTER);
            Self::wake_after(SCROLL_HIDE_AFTER, cx);
        } else if bar.drag.is_none() {
            bar.hide_timeout(Duration::ZERO);
        }
        cx.notify();
    }

    /// A render after `delay` (timers are checked while rendering).
    fn wake_after(delay: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(delay + Duration::from_millis(20))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The draft of a chat as this view knows it.
    fn draft_of(&self, peer: PeerKey) -> Option<String> {
        match self.drafts.get(&peer) {
            Some(text) => text.clone(),
            None => self
                .dialogs
                .iter()
                .find(|d| d.peer == peer)
                .and_then(|d| d.draft.clone()),
        }
        .filter(|t| !t.trim().is_empty())
    }

    /// Keeps the unsent text of the open chat as its draft, locally and (shortly) in
    /// the cloud: tdesktop saves both when a chat is left.
    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).text();
        if let Some(prev) = self.selected {
            let tags = self.input.read(cx).tags();
            if tags.is_empty() {
                self.draft_tags.remove(&prev);
            } else {
                self.draft_tags.insert(prev, (text.clone(), tags));
            }
            self.drafts
                .insert(prev, (!text.trim().is_empty()).then(|| text.clone()));
            self.draft_dirty_since = None;
            self.draft_generation += 1;
            let _ = self.commands.send(Command::SaveDraft {
                peer: prev,
                text,
                cloud: true,
            });
        }
    }

    /// Replaces the field's text without treating it as typing.
    fn set_input_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_input_text_with_tags(text, Vec::new(), cx);
    }

    fn set_input_text_with_tags(
        &mut self,
        text: String,
        tags: Vec<TagSpan>,
        cx: &mut Context<Self>,
    ) {
        self.input_text = text.clone();
        self.input
            .update(cx, |input, cx| input.set_text_with_tags(text, tags, cx));
    }

    /// Quotes in the field have the user's own color (`coloredQuoteCache` of the
    /// session user's color index): the color of our messages' author.
    fn update_quote_color(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.messages.iter().find(|m| m.out && m.service.is_none()) {
            let color = name_color(self.palette(), m.author.color);
            self.input.update(cx, |i, cx| i.set_quote_color(color, cx));
        }
    }

    /// The field gets the draft of `peer` (with its formatting).
    fn restore_draft(&mut self, peer: PeerKey, cx: &mut Context<Self>) {
        let text = self.draft_of(peer).unwrap_or_default();
        let tags = self.tags_of(peer, &text);
        self.set_input_text_with_tags(text, tags, cx);
    }

    fn tags_of(&self, peer: PeerKey, text: &str) -> Vec<TagSpan> {
        match self.draft_tags.get(&peer) {
            Some((saved, tags)) if saved == text => tags.clone(),
            _ => Vec::new(),
        }
    }

    /// The field may have changed: tdesktop's draft timers. The local draft is saved a
    /// second after typing stops (at the latest every 5 s while typing on), the cloud
    /// one after 14 s without typing.
    fn input_changed(&mut self, cx: &mut Context<Self>) {
        const SAVE_DRAFT: Duration = Duration::from_millis(1000); // kSaveDraftTimeout
        const SAVE_DRAFT_ANYWAY: Duration = Duration::from_secs(5); // kSaveDraftAnywayTimeout
        const SAVE_CLOUD_DRAFT_IDLE: Duration = Duration::from_secs(14); // kSaveCloudDraftIdleTimeout
        let text = self.input.read(cx).text();
        self.update_send_state(!text.is_empty());
        if !text.is_empty() {
            self.refresh_emoji_keywords();
        }
        if text == self.input_text {
            return;
        }
        self.input_text = text.clone();
        let Some(peer) = self.selected else { return };
        self.drafts
            .insert(peer, (!text.trim().is_empty()).then(|| text.clone()));
        self.draft_generation += 1;
        let generation = self.draft_generation;
        let since = *self.draft_dirty_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= SAVE_DRAFT_ANYWAY {
            self.draft_dirty_since = Some(Instant::now());
            let _ = self.commands.send(Command::SaveDraft {
                peer,
                text: text.clone(),
                cloud: false,
            });
        }
        for (delay, cloud) in [(SAVE_DRAFT, false), (SAVE_CLOUD_DRAFT_IDLE, true)] {
            let text = text.clone();
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, _| {
                    if this.draft_generation == generation && this.selected == Some(peer) {
                        if !cloud {
                            this.draft_dirty_since = None;
                        }
                        let _ = this.commands.send(Command::SaveDraft { peer, text, cloud });
                    }
                });
            })
            .detach();
        }
    }

    /// Esc: closes the main menu, or else the open chat (back to "Select a chat"), as
    /// in tdesktop.
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.hide_menu() || self.clear_text_selection() {
        } else if self.layers.any() {
            self.layers.hide_current();
        } else if let Some(peer) = self.selected
            && self.reply_to.remove(&peer).is_some()
        {
            // `HistoryWidget::escape`: the reply goes first (`cancelReply`).
        } else if !self.back_stack.is_empty() {
            self.go_back(window, cx);
            return;
        } else if self.selected.is_some() {
            if self.one_column() {
                self.start_column_back();
            } else {
                self.close_chat(window, cx);
            }
        } else if self.forum.is_some() {
            self.close_forum(cx);
        } else if self.archive_open {
            // `Widget::escape` with an open folder: `closeFolder`.
            self.close_archive(cx);
        } else {
            return;
        }
        cx.notify();
    }

    /// No chat open any more ("Select a chat", or the list in one column).
    fn close_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_draft(cx);
        self.set_input_text(String::new(), cx);
        self.leave_current(cx);
        self.history_bar.drag = None;
        self.unread_target = None;
        let _ = self.commands.send(Command::CloseChat);
        window.focus(&self.focus);
    }

    /// Selects a chat and asks for its history.
    fn start_opening(&mut self, peer: PeerKey) {
        if self.one_column() && self.selected.is_none() {
            self.column_slide = Some(ColumnSlide {
                start: Instant::now(),
                to_chat: true,
            });
        }
        let dialog = self.dialogs.iter().find(|d| d.peer == peer);
        let read_till = dialog.map_or(0, |d| d.read_inbox_max_id);
        self.unread_target = dialog.filter(|d| d.unread > 0).map(|_| read_till);
        self.unread_bar = None;
        self.read_till = read_till;
        self.selected = Some(peer);
        self.messages.clear();
        self.messages_state.reset(0);
        self.history_complete = false;
        self.loading_older = false;
        self.loading_newer = false;
        self.newer_available = false;
        self.pending_focus = None;
        self.highlight = None;
        self.stick_to_bottom = true;
        let _ = self.commands.send(Command::OpenChat(peer));
    }

    fn open(&mut self, peer: PeerKey, window: &mut Window, cx: &mut Context<Self>) {
        // A forum shows its topics instead (`Widget::openForum`).
        if self.chat_item(peer).is_some_and(|c| c.rights.forum) {
            self.open_forum(peer, cx);
            return;
        }
        // From the chat list: the back stack is cleared (`Way::ClearStack`); its chats
        // keep their places like any chat left.
        for pane in std::mem::take(&mut self.back_stack) {
            self.remember(pane);
        }
        self.slide = None;
        if self.selected == Some(peer) {
            return;
        }
        // Keep the unsent text of the chat we leave; restore the one of the chat we open.
        self.save_draft(cx);
        self.restore_draft(peer, cx);
        self.leave_current(cx);
        // Opened again: where it was (`showHistory` prefers the saved scroll state to
        // the unread bar), else at the first unread message or the end.
        match self.remembered.remove(&peer) {
            Some(pane) => {
                self.restore_pane(pane);
                let _ = self.commands.send(Command::ReopenChat(peer));
            }
            None => self.start_opening(peer),
        }
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// The login screens over this view: for the first account (no way back) or for
    /// "Add Account" (`Main::Domain::addActivated`).
    pub fn show_intro(&mut self, can_back: bool, cx: &mut Context<Self>) {
        let accounts = self.accounts.read(cx);
        let (id, session) = accounts.new_account();
        let login = accounts
            .config()
            .filter(|_| !accounts.is_demo())
            .map(|config| crate::accounts::start_login(config.clone(), session, can_back));
        let palette = self.palette();
        let intro =
            cx.new(|cx| crate::intro::IntroView::new(palette, can_back, !can_back, login, cx));
        cx.subscribe(
            &intro,
            |this: &mut Self, _, event: &crate::intro::IntroEvent, cx| {
                match event {
                    crate::intro::IntroEvent::Back => {
                        if let Some((view, id)) = this.intro.take() {
                            this.intro_slide = Some(IntroSlide {
                                start: Instant::now(),
                                leaving: Some(view),
                            });
                            // A login left halfway leaves nothing behind.
                            let base = this.accounts.read(cx).base().to_path_buf();
                            if id != 1 && !crate::accounts::logged_in(&base, id) {
                                let _ = std::fs::remove_dir_all(crate::accounts::account_dir(
                                    &base, id,
                                ));
                            }
                        }
                        cx.notify();
                    }
                    crate::intro::IntroEvent::ChooseCountry => this.open_country_box(cx),
                    crate::intro::IntroEvent::Settings => {
                        this.layers.show_special(layers::Special::IntroSettings);
                        cx.notify();
                    }
                    crate::intro::IntroEvent::Done => {
                        if let Some((_, id)) = this.intro.as_ref() {
                            let id = *id;
                            this.accounts.update(cx, |a, cx| a.logged_in(id, cx));
                        }
                    }
                }
            },
        )
        .detach();
        self.intro = Some((intro, id));
        cx.notify();
    }

    fn start_add_account(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.layers.reset();
        self.show_intro(true, cx);
        self.intro_slide = Some(IntroSlide {
            start: Instant::now(),
            leaving: None,
        });
    }

    /// The login view shown now (taken over by the next view of the window).
    pub(crate) fn intro_view(&self) -> Option<Entity<crate::intro::IntroView>> {
        self.intro.as_ref().map(|(i, _)| i.clone())
    }

    /// "Add Account" (the main menu, Settings' menu): the login, or the toast about
    /// the limit (`AccountsLimitBox`).
    fn add_account_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accounts = self.accounts.read(cx);
        if accounts.slots.len() >= accounts.max_accounts() {
            self.show_toast("You have reached the limit of connected accounts.", None);
        } else {
            self.start_add_account(window, cx);
        }
    }

    /// Light or dark.
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        let palette = self.palette();
        self.input
            .update(cx, |input, cx| input.set_palette(palette, cx));
        self.search
            .update(cx, |input, cx| input.set_palette(palette, cx));
        if let Some((intro, _)) = &self.intro {
            intro.update(cx, |v, cx| v.set_palette(palette, cx));
        }
        cx.notify();
    }

    /// Viewport height, scrollable height and offset of a list (the history counts
    /// loaded messages only, as in tdesktop).
    fn scroll_metrics(&self, area: Area) -> (f32, f32, f32) {
        let (viewport, max, offset) = match area {
            Area::History => (
                f32::from(self.messages_state.viewport_bounds().size.height),
                f32::from(self.messages_state.max_offset_for_scrollbar().height),
                -f32::from(self.messages_state.scroll_px_offset_for_scrollbar().y),
            ),
            Area::Dialogs => {
                let base = self.dialogs_scroll.0.borrow().base_handle.clone();
                (
                    f32::from(base.bounds().size.height),
                    f32::from(base.max_offset().height),
                    -f32::from(base.offset().y),
                )
            }
            Area::Settings => {
                let s = &self.layers.settings_scroll;
                (
                    f32::from(s.bounds().size.height),
                    f32::from(s.max_offset().height),
                    -f32::from(s.offset().y),
                )
            }
            Area::Emoji => {
                let s = &self.emoji_panel.scroll;
                (
                    f32::from(s.bounds().size.height),
                    f32::from(s.max_offset().height),
                    -f32::from(s.offset().y),
                )
            }
        };
        (viewport, max, offset.clamp(0., max.max(0.)))
    }

    fn set_scroll(&mut self, area: Area, target: f32) {
        match area {
            Area::History => {
                self.messages_state
                    .set_offset_from_scrollbar(point(px(0.), px(-target)));
            }
            Area::Dialogs => {
                let base = self.dialogs_scroll.0.borrow().base_handle.clone();
                base.set_offset(point(px(0.), px(-target)));
            }
            Area::Emoji => {
                self.emoji_panel
                    .scroll
                    .set_offset(point(px(0.), px(-target)));
            }
            Area::Settings => {
                self.layers
                    .settings_scroll
                    .set_offset(point(px(0.), px(-target)));
            }
        }
    }

    /// A userpic: the photo when it is loaded (asked for on first sight), the empty
    /// userpic meanwhile.
    fn userpic(
        &self,
        size: f32,
        peer: Option<PeerKey>,
        photo: Option<(i64, i32)>,
        initials: &str,
        color: u8,
        saved: bool,
    ) -> AnyElement {
        let p = self.palette();
        if !saved && let (Some(peer), Some((photo_id, dc_id))) = (peer, photo) {
            if let Some((loaded, image)) = self.userpics.get(&peer)
                && *loaded == photo_id
            {
                return img(image.clone())
                    .flex_none()
                    .size(px(size))
                    .rounded_full()
                    .into_any_element();
            }
            if self.userpic_requests.borrow_mut().insert(photo_id) {
                let _ = self.commands.send(Command::LoadUserpic {
                    peer,
                    photo_id,
                    dc_id,
                });
            }
        }
        // `PeerData::userpicCloudImage`: "Telegram" without a loaded photo shows the
        // app's logo (`Window::LogoNoMargin`).
        if !saved && peer == Some(PeerKey::user(tdr_data::SERVICE_NOTIFICATIONS_ID)) {
            return img(crate::icons::telegram_logo())
                .flex_none()
                .size(px(size))
                .rounded_full()
                .into_any_element();
        }
        empty_userpic(p, size, initials, color, saved).into_any_element()
    }

    /// A name cut to leave room for its badge, the badge right after it
    /// (`PeerBadge::drawGetWidth`: the icon at `min(nameWidth, width - iconWidth)`).
    fn name_with_badge(
        &self,
        name: &str,
        badge: crate::bridge::PeerBadge,
        active: bool,
        window: &Window,
    ) -> AnyElement {
        let p = self.palette();
        let (bg, fg) = if active {
            (
                p.dialogs_verified_icon_bg_active,
                p.dialogs_verified_icon_fg_active,
            )
        } else {
            (p.dialogs_verified_icon_bg, p.dialogs_verified_icon_fg)
        };
        let icon_el = match badge {
            crate::bridge::PeerBadge::None => return one_line(name.to_string()).into_any_element(),
            crate::bridge::PeerBadge::Verified => div()
                .relative()
                .child(icon(Icon::VerifiedStar, bg, window))
                .child(div().absolute().left_0().top_0().child(icon(
                    Icon::VerifiedCheck,
                    fg,
                    window,
                ))),
            crate::bridge::PeerBadge::Premium => div().child(icon(Icon::PremiumStar, bg, window)),
        };
        let name_w = text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD);
        div()
            .w_full()
            .flex()
            .items_start()
            .child(
                div()
                    .flex_shrink()
                    .min_w(gpui::px(0.))
                    .w(gpui::px(name_w.ceil() + 1.))
                    .child(one_line(name.to_string())),
            )
            .child(icon_el.flex_none())
            .into_any_element()
    }

    /// The custom title bar of tdesktop on Windows: no text, caption buttons on the right.
    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let active = window.is_window_active();
        let bg = if active {
            p.title_bg_active
        } else {
            p.title_bg
        };
        let button =
            |id: &'static str, glyph: &'static str, area: WindowControlArea, close: bool| {
                let (fg, bg_over, fg_over) = match (close, active) {
                    (true, true) => (
                        p.title_button_close_fg_active,
                        p.title_button_close_bg_active_over,
                        p.title_button_close_fg_active_over,
                    ),
                    (true, false) => (
                        p.title_button_close_fg,
                        p.title_button_close_bg_over,
                        p.title_button_close_fg_over,
                    ),
                    (false, true) => (
                        p.title_button_fg_active,
                        p.title_button_bg_active_over,
                        p.title_button_fg_active_over,
                    ),
                    (false, false) => (
                        p.title_button_fg,
                        p.title_button_bg_over,
                        p.title_button_fg_over,
                    ),
                };
                // GPUI 0.2.2 applies `hover` styles at paint time, after text was shaped with
                // its color, so a hover text color never shows: track the hover ourselves.
                let hovered = self.title_hovered == Some(id);
                div()
                    .id(id)
                    .w(px(TITLE_BUTTON_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(self.icon_font.clone())
                    .text_size(px(10.))
                    .text_color(c(if hovered { fg_over } else { fg }))
                    .when(hovered, |d| d.bg(c(bg_over)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.title_hovered = Some(id);
                        } else if this.title_hovered == Some(id) {
                            this.title_hovered = None;
                        }
                        cx.notify();
                    }))
                    .window_control_area(area)
                    .child(glyph)
            };
        let max_glyph = if window.is_maximized() {
            "\u{E923}"
        } else {
            "\u{E922}"
        };
        div()
            .h(px(TITLE_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .bg(c(bg))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(button(
                "title-min",
                "\u{E921}",
                WindowControlArea::Min,
                false,
            ))
            .child(button(
                "title-max",
                max_glyph,
                WindowControlArea::Max,
                false,
            ))
            .child(button(
                "title-close",
                "\u{E8BB}",
                WindowControlArea::Close,
                true,
            ))
    }

    /// A chat-list row (`Dialogs::Ui::PaintRow`).
    /// `floating`: the dragged pinned chat, drawn over the list (pressed, so with the
    /// hover background); its own slot in the list stays empty.
    fn render_dialog(
        &self,
        item: &DialogItem,
        floating: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !floating && self.pin_drag.as_ref().is_some_and(|d| d.peer == item.peer) {
            return div().h(px(DIALOG_ROW_HEIGHT)).w_full().into_any_element();
        }
        let p = self.palette();
        let active = self.selected == Some(item.peer);
        let (bg, name_fg, text_fg, date_fg, draft_fg, service_fg, icon_fg, sent_fg) = if active {
            (
                p.dialogs_bg_active,
                p.dialogs_name_fg_active,
                p.dialogs_text_fg_active,
                p.dialogs_date_fg_active,
                p.dialogs_draft_fg_active,
                p.dialogs_text_fg_service_active,
                p.dialogs_chat_icon_fg_active,
                p.dialogs_sent_icon_fg_active,
            )
        } else {
            (
                p.dialogs_bg,
                p.dialogs_name_fg,
                p.dialogs_text_fg,
                p.dialogs_date_fg,
                p.dialogs_draft_fg,
                p.dialogs_text_fg_service,
                p.dialogs_chat_icon_fg,
                p.dialogs_sent_icon_fg,
            )
        };
        let bg = if floating && !active {
            p.dialogs_bg_over
        } else {
            bg
        };
        let peer = item.peer;
        let narrow = self.narrow(window);
        let shift = self.row_shift(peer);
        let draft = (!active).then(|| self.draft_of(peer)).flatten();
        let has_draft = draft.is_some();
        // Unread badge: grey for muted chats (`UnreadBadgeStyle::muted`), the number as
        // `FormatUnreadCounter` writes it.
        let text_font = scale::font(FONT_SIZE);
        let unread_font = scale::font(UNREAD_FONT_SIZE);
        let badge_text = if narrow && item.unread > 999_999 {
            "99999+".to_string()
        } else {
            item.unread.to_string()
        };
        // `PaintUnreadBadge`: at least as wide as it is high, the number centered.
        let badge_width = (text_width(window, &badge_text, UNREAD_FONT_SIZE, FontWeight::BOLD)
            .ceil()
            + 2. * lg(UNREAD_PADDING))
        .max(lg(UNREAD_HEIGHT));
        let badge = (item.unread > 0).then(|| {
            let badge_bg = match (active, item.muted) {
                (true, true) => p.dialogs_unread_bg_muted_active,
                (true, false) => p.dialogs_unread_bg_active,
                (false, true) => p.dialogs_unread_bg_muted,
                (false, false) => p.dialogs_unread_bg,
            };
            let badge_fg = if active {
                p.dialogs_unread_fg_active
            } else {
                p.dialogs_unread_fg
            };
            div()
                .w(dpx((badge_width * scale::get()).round()))
                .h(px(UNREAD_HEIGHT))
                .rounded(px(UNREAD_HEIGHT / 2.))
                .bg(c(badge_bg))
                .text_color(c(badge_fg))
                .text_size(unread_font.size)
                .line_height(unread_font.line_height())
                .font_weight(FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .child(badge_text)
        });
        let row = div()
            .id(SharedString::from(format!(
                "dialog-{:?}-{}",
                peer.kind, peer.id
            )))
            .h(px(DIALOG_ROW_HEIGHT))
            .w_full()
            .relative()
            .top(gpui::px(shift))
            .bg(c(bg))
            .when(!active && self.pin_drag.is_none(), |d| {
                d.hover(|s| s.bg(c(p.dialogs_bg_over)))
            })
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, _| {
                    this.row_pressed(peer, e.position.y)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, _| {
                    this.row_pressed(peer, e.position.y)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !std::mem::take(&mut this.suppress_open) {
                    this.open(peer, window, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(DIALOG_PADDING_X))
                    .top(px(DIALOG_PADDING_Y))
                    .child(self.userpic(
                        DIALOG_PHOTO,
                        Some(peer),
                        item.photo,
                        &item.initials,
                        item.color,
                        item.is_saved,
                    )),
            );
        // The online dot on the userpic's corner (`PaintCornerBadgeFrame`): a
        // `dialogsOnlineBadgeSize` circle at `dialogsOnlineBadgeSkip` from the corner,
        // its `dialogsOnlineBadgeStroke` outline cut out of the userpic (the row's
        // background shows). Not under a narrow row's counter.
        let counter_on_userpic = narrow && item.unread > 0 && !has_draft;
        let online = item
            .lastseen
            .is_some_and(|l| present::is_online(l, chrono::Local::now()));
        let row = if online && !counter_on_userpic {
            let photo = scale::device(DIALOG_PHOTO);
            let size = scale::device(10.);
            let stroke = scale::device(2.);
            let (x, y) = (photo - size, photo - scale::device(2.) - size);
            let ring_bg = if active {
                p.dialogs_bg_active
            } else if floating {
                p.dialogs_bg_over
            } else {
                p.dialogs_bg
            };
            let dot_fg = if active {
                p.dialogs_online_badge_fg_active
            } else {
                p.dialogs_online_badge_fg
            };
            let left = scale::device(DIALOG_PADDING_X) + x - stroke / 2.;
            let top = scale::device(DIALOG_PADDING_Y) + y - stroke / 2.;
            row.group("dialog-row").child(
                div()
                    .absolute()
                    .left(dpx(left))
                    .top(dpx(top))
                    .size(dpx(size + stroke))
                    .rounded_full()
                    .bg(c(ring_bg))
                    .when(!active && !floating, |d| {
                        d.group_hover("dialog-row", |s| s.bg(c(p.dialogs_bg_over)))
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(dpx(size - stroke)).rounded_full().bg(c(dot_fg))),
            )
        } else {
            row
        };
        // The unread top of the badge, aligned to the text line's baseline
        // (`PaintWideCounter` / `PaintNarrowCounter`).
        let badge_top = |text_top: f32| {
            let top = scale::device(text_top) + text_font.ascent * scale::get()
                - unread_font.ascent * scale::get()
                - ((scale::device(UNREAD_HEIGHT) - unread_font.height * scale::get()) / 2.).floor();
            dpx(top.round())
        };
        if narrow {
            // Userpics only; the counter sits on the userpic's bottom right corner
            // (`PaintNarrowCounter`), hidden while there is a draft.
            return row
                .children(badge.filter(|_| !has_draft).map(|b| {
                    div()
                        .absolute()
                        .top(px(DIALOG_PADDING_Y + DIALOG_PHOTO - UNREAD_HEIGHT))
                        .left(dpx(scale::device(DIALOG_PADDING_X)
                            + scale::device(DIALOG_PHOTO)
                            - (badge_width * scale::get()).round()))
                        .child(b)
                }))
                .into_any_element();
        }
        // Right of the name: the date at the padding, the send state in
        // `dialogsSendStateSkip` before it (`PaintRowTopRight`, `sendStateIcon`).
        let date_width = text_width(window, &item.time, DATE_FONT_SIZE, FontWeight::NORMAL).ceil();
        let date_right = lg(DIALOG_PADDING_X);
        let mut name_right = date_right + date_width + lg(DIALOG_DATE_SKIP);
        let send_state = item.last_read.filter(|_| !has_draft).map(|read| {
            // dialogsSentIcon / dialogsReceivedIcon: offsets (10, 4) and (5, 4).
            let (kind, offset_x) = if read {
                (Icon::DialogsReceived, 5.)
            } else {
                (Icon::DialogsSent, 10.)
            };
            name_right += lg(DIALOG_SEND_STATE_SKIP);
            let left_from_right = name_right - lg(offset_x);
            let icon = icon(kind, sent_fg, window);
            div()
                .absolute()
                .top(gpui::px(lg(DIALOG_NAME_TOP) + lg(4.)))
                .right(gpui::px(left_from_right - icon_width(kind, window)))
                .child(icon)
        });
        // Group / channel / bot icon at the name's left (`ChatTypeIcon`, offsets
        // (1, 4) and (3, 4)); the name follows after `dialogsChatTypeSkip`.
        let type_icon = match item.kind {
            ChatKind::Group => Some((Icon::ChatGroup, 1.)),
            ChatKind::Channel => Some((Icon::ChatChannel, 3.)),
            ChatKind::Bot => Some((Icon::ChatBot, 1.)),
            ChatKind::User => None,
        }
        .filter(|_| !item.is_saved);
        let name_left = lg(DIALOG_NAME_LEFT)
            + type_icon.map_or(0., |(kind, offset)| {
                lg(offset) + icon_width(kind, window) + lg(CHAT_TYPE_SKIP)
            });
        let type_icon = type_icon.map(|(kind, offset)| {
            div()
                .absolute()
                .left(gpui::px(lg(DIALOG_NAME_LEFT) + lg(offset)))
                .top(gpui::px(lg(DIALOG_NAME_TOP) + lg(4.)))
                .child(icon(kind, icon_fg, window))
        });
        // The text line ends before the badge or the pin.
        let pinned = item.unread == 0 && item.pinned;
        let text_right = lg(DIALOG_PADDING_X)
            + if badge.is_some() {
                badge_width + lg(UNREAD_PADDING)
            } else if pinned {
                icon_width(Icon::DialogsPinned, window) + lg(UNREAD_PADDING)
            } else {
                0.
            };
        let last_line = match draft {
            Some(d) => div()
                .flex()
                .child(div().flex_none().text_color(c(draft_fg)).child("Draft: "))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(c(text_fg))
                        .child(one_line(d)),
                ),
            None => {
                let last = &item.last;
                let rest = match &last.accent {
                    Some(accent) => div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(c(service_fg))
                        .child(one_line(accent.clone())),
                    None => div().flex_1().min_w(px(0.)).text_color(c(text_fg)).child(
                        one_line(last.text.clone()).spoilers(last.spoilers.clone(), text_fg),
                    ),
                };
                div()
                    .flex()
                    .children(last.prefix.as_ref().map(|name| {
                        // A short first name: plain text (one_line needs a sized parent).
                        div()
                            .flex_none()
                            .text_color(c(service_fg))
                            .child(format!("{name}:\u{a0}"))
                    }))
                    .child(rest)
            }
        };
        let pin_fg = if active {
            p.dialogs_unread_bg_muted_active
        } else {
            p.dialogs_unread_bg_muted
        };
        row.children(type_icon)
            .child(
                div()
                    .absolute()
                    .left(gpui::px(name_left))
                    .right(gpui::px(name_right))
                    .top(px(DIALOG_NAME_TOP))
                    .h(text_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_fg))
                    .child(self.name_with_badge(&item.name, item.badge, active, window)),
            )
            .children(send_state)
            .child(
                div()
                    .absolute()
                    .right(gpui::px(date_right))
                    .top(px(DIALOG_NAME_TOP))
                    .text_size(scale::font(DATE_FONT_SIZE).size)
                    .text_color(c(date_fg))
                    .child(item.time.clone()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(DIALOG_NAME_LEFT))
                    .right(gpui::px(text_right))
                    .top(px(DIALOG_TEXT_TOP))
                    .h(text_font.line_height())
                    .child(last_line),
            )
            .children(badge.map(|b| {
                div()
                    .absolute()
                    .right(px(DIALOG_PADDING_X))
                    .top(badge_top(DIALOG_TEXT_TOP))
                    .child(b)
            }))
            .when(pinned, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(DIALOG_PADDING_X))
                        .top(px(DIALOG_TEXT_TOP))
                        .child(icon(Icon::DialogsPinned, pin_fg, window)),
                )
            })
            .into_any_element()
    }

    /// Whether messages `a` and `b` (in this order) belong to one group: same author,
    /// close in time, same day, no service message or unread bar between.
    fn attached(pane: PaneRef, a: &MessageItem, b: &MessageItem) -> bool {
        a.service.is_none()
            && b.service.is_none()
            && a.post.is_none()
            && b.post.is_none()
            && a.out == b.out
            && a.author == b.author
            && (b.date - a.date).abs() < ATTACH_SECONDS
            && present::day_of(a.date) == present::day_of(b.date)
            && pane.unread_bar != Some(b.id)
    }

    /// The open chat's message `i`.
    fn render_message(&self, i: usize, window: &Window, view: &WeakEntity<Self>) -> AnyElement {
        let pane = PaneRef {
            peer: self.selected,
            messages: &self.messages,
            unread_bar: self.unread_bar,
            highlight: self.highlight_opacity(),
            live: true,
            thread_root: self.thread.as_ref().and_then(|t| t.root),
        };
        self.render_message_in(pane, i, window, view)
    }

    /// Message `i` of the chat drawn beside the open one during a slide.
    fn render_slide_message(
        &self,
        i: usize,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let Some(slide) = &self.slide else {
            return div().into_any_element();
        };
        let pane = PaneRef {
            peer: Some(slide.pane.peer),
            messages: &slide.pane.messages,
            unread_bar: slide.pane.unread_bar,
            highlight: None,
            live: false,
            thread_root: slide.pane.thread.as_ref().and_then(|t| t.root),
        };
        self.render_message_in(pane, i, window, view)
    }

    /// A message row: a date divider when the day changes, the "Unread messages" bar,
    /// then the message — a bubble or a service line.
    fn render_message_in(
        &self,
        pane: PaneRef,
        i: usize,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let Some(item) = pane.messages.get(i) else {
            return div().into_any_element();
        };
        let p = self.palette();
        let chat_width = self.chat_width(window);
        let column = service_column(chat_width);
        let day = present::day_of(item.date);
        let new_day = i == 0 || present::day_of(pane.messages[i - 1].date) != day;
        let today = chrono::Local::now().date_naive();
        let divider = new_day.then_some(day).flatten().map(|day| {
            let text = present::day_divider(day, today);
            let w = text_width(window, &text, FONT_SIZE, FontWeight::SEMIBOLD).ceil() + 1.;
            div()
                .w(px(column))
                .flex()
                .justify_center()
                .pt(px(SERVICE_MARGIN_TOP))
                .pb(px(SERVICE_MARGIN_BOTTOM))
                .child(service_pill(p, text, w))
        });
        // The bar spans the chat; its text is centered in the service column.
        let unread_bar = (pane.unread_bar == Some(item.id)).then(|| {
            div()
                .w_full()
                .mt(px(UNREAD_BAR_MARGIN))
                .h(px(UNREAD_BAR_HEIGHT))
                .flex()
                .items_center()
                .bg(c(p.history_unread_bar_bg))
                .border_t_1()
                .border_b_1()
                .border_color(c(p.history_unread_bar_border))
                .text_color(c(p.history_unread_bar_fg))
                .font_weight(FontWeight::SEMIBOLD)
                .child(
                    div()
                        .w(px(column))
                        .flex()
                        .justify_center()
                        .child("Unread messages"),
                )
        });
        let body = match &item.service {
            Some(text) => {
                let max = SERVICE_MAX_WIDTH.min(column - 2. * MSG_SIDE - 2. * SERVICE_PADDING_X);
                let w = (text_width(window, text, FONT_SIZE, FontWeight::SEMIBOLD) + 1.)
                    .min(max)
                    .ceil();
                div()
                    .w(px(column))
                    .flex()
                    .justify_center()
                    .pt(px(SERVICE_MARGIN_TOP))
                    .pb(px(SERVICE_MARGIN_BOTTOM))
                    .child(service_pill(p, text.clone(), w))
                    .into_any_element()
            }
            None => self.render_bubble(pane, i, item, chat_width, window, view),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .children(divider)
            .children(unread_bar)
            .child(body)
            .into_any_element()
    }

    /// A message bubble. tdesktop's layouts: in a wide chat (≥ 880 px) every message is
    /// on the left with userpics; in a narrow one outgoing messages go right and userpics
    /// are shown for group members only. Consecutive messages of one author form a
    /// group: no gap, small corners on the group side, userpic and name once.
    ///
    /// Bubbles get an explicit width from the measured text: GPUI 0.2.2 otherwise
    /// measures wrapped text at one width and paints it at another.
    fn render_bubble(
        &self,
        pane: PaneRef,
        i: usize,
        item: &MessageItem,
        chat_width: f32,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let chat = pane.peer.and_then(|peer| self.chat_item(peer));
        let is_group = chat.is_some_and(|d| d.is_group());
        let is_saved = chat.is_some_and(|d| d.is_saved);
        let wide = chat_width >= CHAT_WIDE_WIDTH;
        // `isCommentsRootView`: a post's copy over its comments is centered, as
        // wide as a message can be, without a userpic or a name.
        let comments_root = pane.thread_root == Some(item.id) && item.forwarded.is_some();
        let right_side = item.out && !wide && !comments_root;
        // `hasFromPhoto`: groups, and forwards from others in Saved Messages; posts
        // hide their author (`isPostHidingAuthor`).
        let post = item.post.as_ref();
        let with_userpic =
            post.is_none() && !comments_root && (wide || ((is_group || is_saved) && !item.out));

        let prev_attached = i > 0 && Self::attached(pane, &pane.messages[i - 1], item);
        let next_attached = pane
            .messages
            .get(i + 1)
            .is_some_and(|n| Self::attached(pane, item, n));

        let (bg, fg, date_fg) = if item.out {
            (p.msg_out_bg, p.history_text_out_fg, p.msg_out_date_fg)
        } else {
            (p.msg_in_bg, p.history_text_in_fg, p.msg_in_date_fg)
        };
        // The round button beside the bubble takes `msgPhotoSkip` more
        // (`countGeometry`: `contentWidth -= size + (msgPhotoSkip - size)`).
        let original = item.original;
        let reserved = MSG_SIDE
            + MSG_MARGIN_FAR
            + if with_userpic { MSG_PHOTO_SKIP } else { 0. }
            + if original.is_some() && with_userpic {
                MSG_PHOTO_SKIP
            } else {
                0.
            };
        let (pad_x, pad_y) = (lg(MSG_PADDING_X), lg(MSG_PADDING_Y));
        let max_text =
            (lg(MSG_MAX_WIDTH).min(chat_width - lg(reserved)) - 2. * pad_x).max(lg(120.));
        let text_font = scale::font(FONT_SIZE);
        let date_font = scale::font(DATE_FONT_SIZE);
        let sender = item
            .sender
            .as_ref()
            .filter(|_| !prev_attached && !comments_root);
        // Media is not drawn yet: its caption stands in ("Photo", "Voice message").
        let media = item.media.as_ref();
        let text = if item.text.is_empty() {
            media.cloned().unwrap_or_default()
        } else {
            item.text.clone()
        };
        let media_line = media.filter(|_| !item.text.is_empty());
        let time_text = if item.edited {
            format!("edited {}", item.time)
        } else {
            item.time.clone()
        };
        // `BottomInfo::layoutDateText`: "Author, edited 12:30".
        let time_text = match post.and_then(|p| p.signature.as_ref()) {
            Some(author) => format!("{author}, {time_text}"),
            None => time_text,
        };
        // The views before it: the eye (`historyViewsWidth`) and the count, then
        // `historyViewsSpace`.
        let views = post.and_then(|p| p.views.clone());
        let views_w = views.as_ref().map_or(0., |v| {
            lg(VIEWS_WIDTH)
                + text_width(window, v, DATE_FONT_SIZE, FontWeight::NORMAL).ceil()
                + lg(VIEWS_SPACE)
        });
        let comments = post.and_then(|p| p.comments.clone());
        // The button is as wide as its parts at least (less the bubble's padding).
        let repliers = post.map_or(&[][..], |p| p.repliers.as_slice());
        let comments_w = comments.as_ref().map_or(0., |t| {
            text_width(window, t, FONT_SIZE, FontWeight::SEMIBOLD)
                + lg(COMMENTS_SKIP_LEFT + COMMENTS_SKIP_TEXT + COMMENTS_SKIP_RIGHT)
                + if repliers.is_empty() {
                    icon_width(Icon::HistoryComments, window)
                } else {
                    threads::commenters_width(repliers.len())
                }
                + icon_width(Icon::HistoryCommentsOpen, window)
                - 2. * lg(MSG_PADDING_X)
        });
        let comments_h = if comments.is_some() {
            lg(COMMENTS_HEIGHT)
        } else {
            0.
        };
        // `BottomInfo`: the date, then `historySendStateSpace` for the checks. The text
        // ends with a skip block that size (`skipBlockWidth/Height`): on its last line
        // when it fits there, otherwise on a line of its own.
        let date_w = text_width(window, &time_text, DATE_FONT_SIZE, FontWeight::NORMAL).ceil();
        let info_w = views_w
            + date_w
            + if item.read.is_some() {
                lg(SEND_STATE_SPACE)
            } else {
                0.
            };
        let skip_w = lg(MSG_DATE_SPACE) + info_w - lg(MSG_DATE_DELTA_X);
        let skip_h = date_font.height - lg(MSG_DATE_DELTA_Y);
        // The text (`rich_text`): its natural width has the skip block at its end.
        let text_key = (pane.peer.unwrap_or(PeerKey::user(0)), item.id);
        let caption_only = item.text.is_empty();
        let entities: &[crate::rich_text::TextEntity] =
            if caption_only { &[] } else { &item.entities };
        let measured = self.rich_measured(text_key, &text, entities, window);
        let text_w = measured.natural_width(skip_w);
        let name_w = sender.map_or(0., |(name, _)| {
            text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD)
        });
        // "Forwarded from [userpic] Name" (`HistoryMessageForwarded::text`).
        const FORWARDED_FROM: &str = "Forwarded from ";
        let forwarded = item.forwarded.as_ref();
        let forwarded_w = forwarded.map_or(0., |f| {
            text_width(window, FORWARDED_FROM, FONT_SIZE, FontWeight::SEMIBOLD)
                + f.author
                    .as_ref()
                    .map_or(0., |_| lg(FWD_USERPIC) + lg(FWD_USERPIC_SKIP))
                + text_width(window, &f.name, FONT_SIZE, FontWeight::SEMIBOLD)
        });
        let media_w =
            media_line.map_or(0., |m| text_width(window, m, FONT_SIZE, FontWeight::NORMAL));
        // `Reply`: inside a thread its reply to the root is not shown.
        let reply = item
            .reply
            .as_ref()
            .filter(|r| pane.thread_root != Some(r.msg_id));
        let reply_w = reply.map_or(0., |r| {
            lg(REPLY_PADDING[0])
                + text_width(window, &r.name, FONT_SIZE, FontWeight::SEMIBOLD).max(text_width(
                    window,
                    &r.text,
                    FONT_SIZE,
                    FontWeight::NORMAL,
                ))
                + lg(REPLY_PADDING[2])
        });
        let reply_h = if reply.is_some() {
            lg(REPLY_TOP)
                + lg(REPLY_PADDING[1])
                + 2. * text_font.height
                + lg(REPLY_PADDING[3])
                + lg(REPLY_BOTTOM)
        } else {
            0.
        };
        // Whole device pixels; the text is laid out at this width.
        let inner = (text_w
            .max(name_w)
            .max(media_w)
            .max(forwarded_w)
            .max(comments_w)
            .max(reply_w)
            .max(if comments_root { max_text } else { 0. })
            .min(max_text)
            * scale::get())
        .ceil()
            / scale::get();
        let text_layout = self.rich_layout(text_key, &measured, inner, (skip_w, skip_h));
        let text_h = text_layout.height;

        let small = px(MSG_RADIUS_SMALL);
        let large = px(MSG_RADIUS_LARGE);
        // The group side is the one with the userpic (left) or, for outgoing messages in
        // a narrow chat, the right side. The last message of a group has a tail there,
        // where its bottom corner is square (`countMessageRounding`, `Corner::Tail`).
        let tail_corner = if next_attached { small } else { px(0.) };
        let (tl, tr, bl, br) = if right_side {
            (
                large,
                if prev_attached { small } else { large },
                large,
                tail_corner,
            )
        } else {
            (
                if prev_attached { small } else { large },
                large,
                tail_corner,
                large,
            )
        };
        let shadow = if item.out {
            p.msg_out_shadow
        } else {
            p.msg_in_shadow
        };

        // The info sits in the bubble's corner: `msgPadding - msgDateDelta` from its
        // right and bottom edges; the checks at `historySendStatePosition` from the
        // info's right end and the date line's bottom.
        let info = div()
            .absolute()
            .right(gpui::px(pad_x - lg(MSG_DATE_DELTA_X)))
            .bottom(gpui::px(pad_y - lg(MSG_DATE_DELTA_Y) + comments_h))
            .w(gpui::px(info_w))
            .h(date_font.line_height())
            .flex()
            .text_size(date_font.size)
            .line_height(date_font.line_height())
            .text_color(c(date_fg))
            .children(views.map(|v| {
                div()
                    .flex_none()
                    .flex()
                    .pr(px(VIEWS_SPACE))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .w(px(VIEWS_WIDTH))
                            .h(date_font.line_height())
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(gpui::px(date_font.height + lg(VIEWS_TOP)))
                                    .child(icon(Icon::HistoryViews, date_fg, window)),
                            ),
                    )
                    .child(v)
            }))
            .child(time_text)
            .children(item.read.map(|read| {
                let kind = if read {
                    Icon::HistoryReceived
                } else {
                    Icon::HistorySent
                };
                div()
                    .absolute()
                    // historyReceivedIcon / historySentIcon are offset by (2, 4).
                    .left(gpui::px(info_w + lg(SEND_STATE_X) + lg(2.)))
                    .top(gpui::px(date_font.height + lg(SEND_STATE_Y) + lg(4.)))
                    .child(icon(kind, p.history_out_icon_fg, window))
            }));
        // Captions stand in for media: in the date color when they are the whole text.
        let text_colors = self.text_colors(item, item.text.is_empty() && media.is_some());
        let (text_state, text_dragging) = self.text_state(text_key);
        let text_view = {
            let view = view.clone();
            let layout = text_layout.clone();
            let live = pane.live;
            crate::rich_text::view(
                SharedString::from(format!(
                    "msg-text-{}-{}-{}",
                    text_key.0.id, item.id, live as u8
                )),
                text_layout,
                text_colors,
                if live { text_state } else { Default::default() },
                live && text_dragging,
                move |event, window, cx| {
                    if live {
                        let _ = view.update(cx, |this, cx| {
                            this.on_text_event(text_key, &layout, event, window, cx);
                        });
                    }
                },
            )
        };
        let bubble = div()
            .relative()
            .flex_none()
            // `msgShadow` under the bubble and its rounded corners.
            .when(!crate::profile::off("shadows"), |d| {
                d.shadow(vec![gpui::BoxShadow {
                    color: c(shadow).into(),
                    offset: point(gpui::px(0.), px(MSG_SHADOW)),
                    blur_radius: gpui::px(0.),
                    spread_radius: gpui::px(0.),
                }])
            })
            .w(gpui::px(inner + 2. * pad_x))
            .px(gpui::px(pad_x))
            .py(gpui::px(pad_y))
            .rounded_tl(tl)
            .rounded_tr(tr)
            .rounded_bl(bl)
            .rounded_br(br)
            .bg(c(bg))
            .text_color(c(fg))
            .flex()
            .flex_col()
            .children(sender.map(|(name, color)| {
                div()
                    .w(gpui::px(inner))
                    .h(text_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_color(p, *color)))
                    .child(one_line(name.clone()))
            }))
            .children(forwarded.map(|f| {
                let fwd_fg = if item.out {
                    p.msg_out_service_fg
                } else {
                    p.msg_in_service_fg
                };
                div()
                    .w(gpui::px(inner))
                    .h(text_font.line_height())
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(fwd_fg))
                    .child(div().flex_none().child(FORWARDED_FROM))
                    .children(f.author.as_ref().map(|a| {
                        div()
                            .flex_none()
                            .mr(px(FWD_USERPIC_SKIP))
                            .child(self.userpic(
                                FWD_USERPIC,
                                a.key,
                                a.photo,
                                &a.initials,
                                a.color,
                                false,
                            ))
                    }))
                    .child(div().flex_1().min_w(px(0.)).child(one_line(f.name.clone())))
            }))
            .children(
                reply.map(|r| self.render_reply_box(r, item.out, inner, pane.live, window, view)),
            )
            .children(media_line.map(|m| {
                div()
                    .w(gpui::px(inner))
                    .h(text_font.line_height())
                    .text_color(c(date_fg))
                    .child(one_line(m.clone()))
            }))
            .child(text_view)
            .child(info)
            .when(comments_h > 0., |d| d.pb(gpui::px(pad_y + comments_h)))
            .children(comments.map(|text| {
                let discussion = post.and_then(|p| p.discussion);
                let channel = pane.peer;
                let post_id = item.id;
                let live = pane.live;
                let view = view.clone();
                self.render_comments_button(item.id, text, repliers, bg, inner + 2. * pad_x, window)
                    .on_click(move |_, _, cx| {
                        if let (true, Some(channel)) = (live, channel) {
                            let _ = view.update(cx, |this, cx| {
                                this.open_comments(channel, post_id, discussion, cx);
                            });
                        }
                    })
            }));
        // The tail (`bubble_tail`) hangs outside the square corner; the shadow line
        // continues under it (`PaintBubbleGeneric`).
        let tail = (!next_attached).then(|| {
            let (tail_w, tail_h) = (lg(TAIL_SIZE.0), lg(TAIL_SIZE.1));
            let fill = c(bg);
            div()
                .absolute()
                .bottom(-px(MSG_SHADOW))
                .when(right_side, |d| d.right(gpui::px(-tail_w)))
                .when(!right_side, |d| d.left(gpui::px(-tail_w)))
                .w(gpui::px(tail_w))
                .flex()
                .flex_col()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            paint_tail(bounds, right_side, fill, window);
                        },
                    )
                    .w(gpui::px(tail_w))
                    .h(gpui::px(tail_h)),
                )
                .child(div().w_full().h(px(MSG_SHADOW)).bg(c(shadow)))
        });
        // The bubble's height, for the button beside it.
        let bubble_h = 2. * pad_y
            + comments_h
            + reply_h
            + text_h
            + text_font.height
                * (sender.is_some() as u8 + forwarded.is_some() as u8 + media_line.is_some() as u8)
                    as f32;
        // "Go to original" (`drawRightAction`): a round button beside the bubble, at
        // its bottom (`historyFastShareBottom` up, centered on short bubbles).
        // `displayFastShare`: channel posts get the round "share" button there
        // (forwarding is not done yet: a stub).
        let share = (original.is_none() && post.is_some()).then(|| {
            let size = lg(FAST_SHARE_SIZE);
            let skip = ((bubble_h - size) / 2.).clamp(0., lg(FAST_SHARE_BOTTOM));
            let side = gpui::px(-(size + lg(FAST_SHARE_LEFT)));
            div()
                .id(SharedString::from(format!("fast-share-{}", item.id)))
                .absolute()
                .bottom(gpui::px(skip))
                .when(right_side, |d| d.left(side))
                .when(!right_side, |d| d.right(side))
                .size(gpui::px(size))
                .rounded_full()
                .bg(c(p.msg_service_bg))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(icon(Icon::FastShare, p.msg_service_fg, window))
        });
        let go_to = original.map(|(peer, msg_id)| {
            let size = lg(FAST_SHARE_SIZE);
            let skip = ((bubble_h - size) / 2.).clamp(0., lg(FAST_SHARE_BOTTOM));
            let side = gpui::px(-(size + lg(FAST_SHARE_LEFT)));
            let view = view.clone();
            let live = pane.live;
            div()
                .id(SharedString::from(format!("go-to-original-{}", item.id)))
                .absolute()
                .bottom(gpui::px(skip))
                .when(right_side, |d| d.left(side))
                .when(!right_side, |d| d.right(side))
                .size(gpui::px(size))
                .rounded_full()
                .bg(c(p.msg_service_bg))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(icon(Icon::GoToOriginal, p.msg_service_fg, window))
                .on_click(move |_, window, cx| {
                    if live {
                        let _ = view.update(cx, |this, cx| {
                            this.go_to_original(peer, Some(msg_id), window, cx);
                        });
                    }
                })
        });
        // "Click to copy" beside the bubble, at the hovered line of one-line code.
        let code_pill = pane
            .live
            .then(|| {
                let text_top = pad_y
                    + reply_h
                    + text_font.height
                        * (sender.is_some() as u8
                            + forwarded.is_some() as u8
                            + media_line.is_some() as u8) as f32;
                let gap = lg(FAST_SHARE_LEFT)
                    + if original.is_some() || post.is_some() {
                        lg(FAST_SHARE_SIZE) + lg(FAST_SHARE_LEFT)
                    } else {
                        0.
                    };
                // Where the bubble stands in the chat (the row's padding, the
                // userpic column, the right side for outgoing).
                let bubble_w = inner + 2. * pad_x;
                let bubble_left = if right_side {
                    chat_width - lg(MSG_SIDE) - bubble_w
                } else {
                    lg(MSG_SIDE) + if with_userpic { lg(MSG_PHOTO_SKIP) } else { 0. }
                };
                self.code_hint_pill(
                    text_key,
                    text_top,
                    (bubble_left, bubble_w, chat_width),
                    gap,
                    right_side,
                    window,
                )
            })
            .flatten();
        let menu_view = view.clone();
        let live = pane.live;
        let bubble = div()
            .relative()
            .flex_none()
            .child(bubble)
            .children(tail)
            .children(go_to)
            .children(share)
            .children(code_pill)
            // The message's menu (over the text the text answers it, with links).
            .on_mouse_down(MouseButton::Right, move |e, _, cx| {
                if live {
                    let _ = menu_view.update(cx, |this, cx| {
                        this.open_message_menu(text_key, None, None, e.position);
                        cx.notify();
                    });
                }
            });
        // The jumped-to message's highlight (`paintHighlight`): the row, from
        // `min(marginTop, marginBottom)` above the bubble to as much below it.
        let top_margin = if prev_attached {
            MSG_MARGIN_TOP_ATTACHED
        } else {
            MSG_MARGIN_TOP
        };
        let highlight = pane
            .highlight
            .filter(|(id, _)| *id == item.id)
            .map(|(_, opacity)| {
                let fill = lg(top_margin).min(lg(MSG_MARGIN_BOTTOM));
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(gpui::px(lg(top_margin) - fill))
                    .bottom_0()
                    .bg(c(p.msg_select_overlay))
                    .opacity(opacity)
            });

        let avatar_slot = with_userpic.then(|| {
            let author = &item.author;
            div()
                .flex_none()
                .w(px(MSG_PHOTO_SKIP))
                .flex()
                .items_end()
                .when(!next_attached, |d| {
                    d.child(self.userpic(
                        MSG_PHOTO_SIZE,
                        author.key,
                        author.photo,
                        &author.initials,
                        author.color,
                        false,
                    ))
                })
        });
        div()
            .relative()
            .w_full()
            .flex()
            .items_end()
            .pt(px(top_margin))
            .pb(px(MSG_MARGIN_BOTTOM))
            .children(highlight)
            .px(px(MSG_SIDE))
            .when(right_side, |d| d.justify_end())
            // Centered in the messages' column, as the service lines are.
            .when(comments_root, |d| {
                d.justify_center()
                    .max_w(gpui::px(service_column(chat_width)))
            })
            .children(avatar_slot)
            .child(bubble)
            .into_any_element()
    }

    /// The replied message over the text (`Reply::paint`): a tinted box in the
    /// sender's color with its outline, the sender's name and the first line; a
    /// click shows the message.
    fn render_reply_box(
        &self,
        r: &crate::bridge::ReplyLine,
        out: bool,
        width: f32,
        live: bool,
        _window: &Window,
        view: &WeakEntity<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let font = scale::font(FONT_SIZE);
        // `FromNameFg` for a colored sender in incoming messages, the message's
        // reply colors otherwise.
        let (bar, name_fg, text_fg) = match (out, r.color) {
            (true, _) => (
                p.msg_out_reply_bar_color,
                p.msg_out_service_fg,
                p.history_text_out_fg,
            ),
            (false, Some(color)) => {
                let c = name_color(p, color);
                (c, c, p.history_text_in_fg)
            }
            (false, None) => (
                p.msg_in_reply_bar_color,
                p.msg_in_service_fg,
                p.history_text_in_fg,
            ),
        };
        let quote = r.quote;
        let (peer, msg_id) = (r.peer, r.msg_id);
        let view = view.clone();
        div()
            .id(SharedString::from(format!("reply-{}-{}", peer.id, msg_id)))
            .relative()
            .w(gpui::px(width))
            .mt(px(REPLY_TOP))
            .mb(px(REPLY_BOTTOM))
            .pl(px(REPLY_PADDING[0]))
            .pr(px(REPLY_PADDING[2]))
            .pt(px(REPLY_PADDING[1]))
            .pb(px(REPLY_PADDING[3]))
            .cursor_pointer()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        crate::rich_text::paint_quote_box(bounds, bar, quote, window);
                    },
                )
                .absolute()
                .left_0()
                .top_0()
                .size_full(),
            )
            .child(
                div()
                    .h(font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_fg))
                    .child(one_line(r.name.clone())),
            )
            .child(
                div()
                    .h(font.line_height())
                    .text_color(c(text_fg))
                    .child(one_line(r.text.clone())),
            )
            .on_click(move |_, window, cx| {
                if live {
                    let _ = view.update(cx, |this, cx| {
                        this.go_to_original(peer, Some(msg_id), window, cx);
                    });
                }
            })
            .into_any_element()
    }

    /// "Reply" in a message's menu: the field replies to it, the bar over the field
    /// says to whom (`HistoryWidget::replyToMessage`).
    fn start_reply(
        &mut self,
        peer: PeerKey,
        msg_id: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.messages.iter().find(|m| m.id == msg_id) else {
            return;
        };
        let name = match &item.sender {
            Some((name, _)) => name.clone(),
            None if item.out => self
                .accounts
                .read(cx)
                .active()
                .and_then(|s| s.me.as_ref().map(|m| m.name.clone()))
                .unwrap_or_default(),
            None => self
                .chat_item(peer)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
        };
        let text = item
            .text
            .lines()
            .next()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| item.media.clone())
            .unwrap_or_default();
        self.reply_to
            .insert(peer, ReplyDraft { msg_id, name, text });
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// The bar over the field while it replies (`HistoryWidget::drawField`): the
    /// reply icon, "Reply to Name", the message's first line, the cancel cross.
    fn render_reply_bar(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let peer = self.selected?;
        let draft = self.reply_to.get(&peer)?;
        let p = self.palette();
        let font = scale::font(FONT_SIZE);
        let close = self
            .icon_button(
                "reply-cancel",
                Icon::BoxButtonClose,
                (REPLY_BAR_HEIGHT, REPLY_BAR_HEIGHT),
                None,
                ((4., 4.), 40.),
                (p.history_reply_cancel_fg, p.history_reply_cancel_fg_over),
                window,
                cx,
                move |this, _, cx| {
                    this.reply_to.remove(&peer);
                    cx.notify();
                },
            )
            .absolute()
            .right_0()
            .top_0();
        Some(
            div()
                .relative()
                .flex_none()
                .w_full()
                .h(px(REPLY_BAR_HEIGHT))
                .bg(c(p.history_reply_bg))
                .child(
                    div()
                        .absolute()
                        .left(px(REPLY_BAR_ICON.0))
                        .top(px(REPLY_BAR_ICON.1))
                        .child(icon(Icon::InputReply, p.history_reply_icon_fg, window)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(REPLY_BAR_SKIP))
                        .right(px(REPLY_BAR_HEIGHT))
                        .top(px(REPLY_BAR_TEXT_TOP))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(font.line_height())
                                .flex()
                                .text_color(c(p.window_active_text_fg))
                                .child(div().flex_none().child("Reply to\u{a0}"))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(one_line(draft.name.clone())),
                                ),
                        )
                        .child(
                            div()
                                .h(font.line_height())
                                .text_color(c(p.history_compose_area_fg))
                                .child(one_line(draft.text.clone())),
                        ),
                )
                .child(close)
                .into_any_element(),
        )
    }

    /// A post's comments button (`Message::paintCommentsButton`): a faint line over
    /// it, the bubble icon, "N comments" / "Leave a comment" in semibold and the
    /// arrow at the right. Opening the comments is not done yet (a stub).
    fn render_comments_button(
        &self,
        id: i32,
        text: String,
        repliers: &[crate::bridge::Author],
        bubble_bg: tdr_theme::Color,
        width: f32,
        window: &Window,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let fg = p.msg_file_thumb_link_in_fg;
        let font = scale::font(FONT_SIZE);
        let height = lg(COMMENTS_HEIGHT);
        div()
            .id(SharedString::from(format!("comments-{id}")))
            .absolute()
            .left_0()
            .bottom_0()
            .w(gpui::px(width))
            .h(gpui::px(height))
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(gpui::px(lg(1.)))
                    .bg(c(p.msg_in_date_fg))
                    .opacity(0.3),
            )
            .child(
                div()
                    .absolute()
                    .left(px(COMMENTS_SKIP_LEFT))
                    .right(px(COMMENTS_SKIP_RIGHT))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(if repliers.is_empty() {
                        icon(Icon::HistoryComments, fg, window).into_any_element()
                    } else {
                        self.render_commenters(repliers, bubble_bg)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .pl(px(COMMENTS_SKIP_TEXT))
                            .text_size(font.size)
                            .line_height(font.line_height())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(fg))
                            .child(one_line(text)),
                    )
                    .child(icon(Icon::HistoryCommentsOpen, fg, window)),
            )
    }

    /// An overlay scrollbar (`historyScroll` in the chat, the default `ScrollArea` style
    /// in the chat list): shown while scrolling and on hover.
    fn render_scrollbar(
        &self,
        area: Area,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let p = self.palette();
        let (viewport, max, offset) = self.scroll_metrics(area);
        if max <= 1. || viewport <= 0. {
            return None;
        }
        let bar_state = self.bar(area);
        let opacity = bar_state.opacity.value();
        let (bg, bg_over, bar_bg, bar_bg_over) = match area {
            Area::History => (
                p.history_scroll_bg,
                p.history_scroll_bg_over,
                p.history_scroll_bar_bg,
                p.history_scroll_bar_bg_over,
            ),
            Area::Dialogs | Area::Settings | Area::Emoji => (
                p.scroll_bg,
                p.scroll_bg_over,
                p.scroll_bar_bg,
                p.scroll_bar_bg_over,
            ),
        };
        let track_color = mix(c(bg), c(bg_over), bar_state.over_fade.value(), opacity);
        let bar_color = mix(
            c(bar_bg),
            c(bar_bg_over),
            bar_state.bar_fade.value(),
            opacity,
        );
        // Each style size converted on its own, as tdesktop does: at 125 % the chat
        // list's bar is 12 - 2 * 4 = 4 px wide, not (10 - 6) * 1.25 = 5.
        let (width, round) = area.bar_style();
        let delta = lg(SCROLL_DELTA);
        let track = viewport - 2. * delta;
        let bar = (track * viewport / (viewport + max)).max(lg(SCROLL_MIN_BAR));
        let bar_top = delta + (track - bar) * (offset / max);
        let id = match area {
            Area::History => "history-scroll",
            Area::Dialogs => "dialogs-scroll",
            Area::Settings => "settings-scrollbar",
            Area::Emoji => "emoji-scrollbar",
        };
        Some(
            div()
                .id(id)
                .absolute()
                .top_0()
                .right_0()
                .w(px(width))
                .h_full()
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.bar_mut(area).set_over(*hovered);
                    let delay = this.bar(area).hide_in;
                    Self::wake_after(delay, cx);
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        let (_, bar, bar_top) = this.bar_geometry(area);
                        let local = f32::from(e.position.y) - this.area_top(area);
                        // On the thumb: hold it there; on the track: by its middle.
                        let grab = if (bar_top..=bar_top + bar).contains(&local) {
                            local - bar_top
                        } else {
                            bar / 2.
                        };
                        this.bar_mut(area).drag = Some(ScrollDrag {
                            grab,
                            mouse_y: e.position.y,
                            applied_max: -1.,
                        });
                        this.bar_mut(area).hide_at = None;
                        this.apply_scrollbar_drag(area);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .when(opacity > 0., |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(gpui::px(delta))
                            .left(gpui::px(delta))
                            .w(gpui::px(lg(width) - 2. * delta))
                            .h(gpui::px(track))
                            .rounded(px(round))
                            .bg(track_color),
                    )
                    .child(
                        div()
                            .id(match area {
                                Area::History => "history-scroll-bar",
                                Area::Dialogs => "dialogs-scroll-bar",
                                Area::Settings => "settings-scroll-bar",
                                Area::Emoji => "emoji-scroll-bar",
                            })
                            .absolute()
                            .top(gpui::px(bar_top))
                            .left(gpui::px(delta))
                            .w(gpui::px(lg(width) - 2. * delta))
                            .h(gpui::px(bar))
                            .rounded(px(round))
                            .bg(bar_color)
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                this.bar_mut(area).over_bar = *hovered;
                                cx.notify();
                            })),
                    )
                }),
        )
    }

    /// The open chat's column during a slide (`Window::SlideAnimation`, without fade):
    /// the arriving chat comes from the right edge over the other one, which moves
    /// `slideShift` to the left and darkens (`slideFadeOutBg`), a shadow at the
    /// arriving chat's edge; going back plays it in reverse.
    fn with_slide(
        &mut self,
        live: gpui::Div,
        border: Rgba,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Some(slide) = &self.slide else {
            return live;
        };
        let t = (slide.start.elapsed().as_secs_f32() / SLIDE_DURATION.as_secs_f32()).min(1.);
        let forward = slide.forward;
        // Back animates from 1 to 0 with the same curve.
        let progress = if forward {
            ease_out_circ(t)
        } else {
            1. - ease_out_circ(t)
        };
        let peer = slide.pane.peer;
        let still_back = if forward {
            self.back_stack.len() > 1
        } else {
            true
        };
        let p = self.palette();
        let Some(other_chat) = self.chat_item(peer).cloned() else {
            return live;
        };
        let header = self.render_header(&other_chat, border, still_back, window, cx);
        let view = cx.entity();
        let weak = view.downgrade();
        let state = slide.pane.state.clone();
        let messages = list(state, move |i, window, cx| {
            view.read(cx).render_slide_message(i, window, &weak)
        })
        .size_full()
        .pb(px(8.));
        let day = self.day_background(TOP_BAR_HEIGHT, window, cx);
        let feed = self
            .with_chat_background(div(), day)
            .relative()
            .overflow_hidden()
            .flex_1()
            .min_h(px(0.))
            .child(messages);
        // The other chat's field, as it was (no caret, no typing).
        let composer = div()
            .flex_none()
            .min_h(px(46.))
            .flex()
            .items_center()
            .pl(px(chrome::COMPOSE_BUTTON_WIDTH))
            .bg(c(p.history_compose_area_bg))
            .border_t_1()
            .border_color(border)
            .text_color(c(p.history_compose_area_fg))
            .child(
                div().text_color(c(p.placeholder_fg)).child(
                    self.draft_of(peer)
                        .unwrap_or_else(|| "Write a message…".into()),
                ),
            );
        let still = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.window_bg))
            .child(header)
            .child(feed)
            .child(composer);
        let live = live.size_full();
        let (under, over) = if forward {
            (still.into_any_element(), live.into_any_element())
        } else {
            (live.into_any_element(), still.into_any_element())
        };
        let width = self.chat_width(window);
        let coord_over = width * (1. - progress);
        let coord_under = -lg(SLIDE_SHIFT) * progress;
        window.request_animation_frame();
        div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(coord_under))
                    .w(gpui::px(width))
                    .h_full()
                    .child(under),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(gpui::px(coord_over))
                    .h_full()
                    .bg(c(p.slide_fade_out_bg))
                    .opacity(progress),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(coord_over))
                    .w(gpui::px(width))
                    .h_full()
                    .child(over),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(
                        coord_over - crate::icons::icon_width(Icon::SlideShadow, window),
                    ))
                    .h_full()
                    .opacity(progress)
                    .child(crate::icons::stretched(
                        Icon::SlideShadow,
                        p.slide_fade_out_shadow_fg,
                        window,
                    )),
            )
    }

    /// The round "scroll down" button of the chat (`historyToDown`), or the chat list's
    /// "scroll to top" one (`dialogsToUp`: the same with the arrow flipped, 1 px lower).
    fn render_jump_button(
        &self,
        id: &'static str,
        arrow: Icon,
        arrow_shift: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = self.palette();
        let hovered = self.jump_hovered == Some(id);
        let (bg, fg) = if hovered {
            (p.history_to_down_bg_over, p.history_to_down_fg_over)
        } else {
            (p.history_to_down_bg, p.history_to_down_fg)
        };
        let layer =
            |top: f32, element: gpui::Div| div().absolute().top(px(top)).left_0().child(element);
        div()
            .id(id)
            .absolute()
            .right(px(TO_DOWN_RIGHT))
            .w(px(TO_DOWN_WIDTH))
            .h(px(TO_DOWN_HEIGHT))
            // Clicks stay on the button; the wheel still scrolls the list under it.
            .block_mouse_except_scroll()
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.jump_hovered = Some(id);
                } else if this.jump_hovered == Some(id) {
                    this.jump_hovered = None;
                }
                cx.notify();
            }))
            .child(layer(
                TO_DOWN_ICON_TOP,
                icon(Icon::DownShadow, p.history_to_down_shadow, window),
            ))
            .child(layer(TO_DOWN_ICON_TOP, icon(Icon::DownCircle, bg, window)))
            .child(layer(
                TO_DOWN_ICON_TOP + arrow_shift,
                icon(arrow, fg, window),
            ))
    }

    /// The list's top in window coordinates.
    fn area_top(&self, area: Area) -> f32 {
        match area {
            Area::History => f32::from(self.messages_state.viewport_bounds().origin.y),
            Area::Dialogs => {
                let base = self.dialogs_scroll.0.borrow().base_handle.clone();
                f32::from(base.bounds().origin.y)
            }
            Area::Settings => f32::from(self.layers.settings_scroll.bounds().origin.y),
            Area::Emoji => f32::from(self.emoji_panel.scroll.bounds().origin.y),
        }
    }

    /// The scrollbar's track height, thumb height and thumb top (in the list).
    fn bar_geometry(&self, area: Area) -> (f32, f32, f32) {
        let (viewport, max, offset) = self.scroll_metrics(area);
        let delta = lg(SCROLL_DELTA);
        let track = viewport - 2. * delta;
        let bar = (track * viewport / (viewport + max)).max(lg(SCROLL_MIN_BAR));
        let top = delta + (track - bar) * if max > 0. { offset / max } else { 0. };
        (track, bar, top)
    }

    fn drag_scrollbar(&mut self, area: Area, mouse_y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.bar_mut(area).drag else {
            return;
        };
        drag.mouse_y = mouse_y;
        self.apply_scrollbar_drag(area);
        cx.notify();
    }

    /// Puts the grabbed point of the thumb under the mouse. Near the top of the chat
    /// older messages load (also while the mouse stands still: the next frames apply
    /// the position again as the list grows).
    fn apply_scrollbar_drag(&mut self, area: Area) {
        let Some((grab, mouse_y)) = self.bar(area).drag.as_ref().map(|d| (d.grab, d.mouse_y))
        else {
            return;
        };
        let (_, max, _) = self.scroll_metrics(area);
        let (track, bar, _) = self.bar_geometry(area);
        let local = f32::from(mouse_y) - self.area_top(area);
        let movable = (track - bar).max(1.);
        let share = ((local - grab - lg(SCROLL_DELTA)) / movable).clamp(0., 1.);
        let target = share * max;
        self.set_scroll(area, target);
        if let Some(drag) = &mut self.bar_mut(area).drag {
            drag.applied_max = max;
        }
        if area == Area::History {
            self.stick_to_bottom = target >= max - 1.;
            if self.messages_state.logical_scroll_top().item_ix <= LOAD_OLDER_THRESHOLD {
                self.load_older();
            }
        }
        self.bar_mut(area).last_scroll = Some(Instant::now());
    }

    fn end_drag(&mut self, area: Area, cx: &mut Context<Self>) {
        if self.bar_mut(area).drag.take().is_some() {
            let bar = self.bar_mut(area);
            if !bar.over {
                bar.hide_at = Some(Instant::now() + bar.hide_in);
                let delay = bar.hide_in;
                Self::wake_after(delay, cx);
            }
            cx.notify();
        }
    }

    /// While scrolling, the date of the topmost visible message floats at the top
    /// (tdesktop's `_scrollDateShown`).
    fn floating_date(&self) -> Option<String> {
        if self.scroll_date.value() <= 0. {
            return None;
        }
        let top = self.messages_state.logical_scroll_top();
        let item = self
            .messages
            .get(top.item_ix)
            .or_else(|| self.messages.last())?;
        let today = chrono::Local::now().date_naive();
        present::day_of(item.date).map(|day| present::day_divider(day, today))
    }

    fn render_header(
        &self,
        chat: &DialogItem,
        border: Rgba,
        back: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let p = self.palette();
        let thread_header = self.thread_header();
        let status = match (&thread_header, chat.lastseen) {
            (Some((_, status)), _) => status.clone(),
            (None, Some(lastseen)) => present::lastseen_text(lastseen, chrono::Local::now()),
            (None, None) => self
                .members
                .get(&chat.peer)
                .cloned()
                .unwrap_or_else(|| chat.status.clone()),
        };
        let status_fg = if status == "online" {
            p.window_active_text_fg
        } else {
            p.window_sub_text_fg
        };
        // The back arrow (`TopBarWidget::_back`) when there is a chat to go back to,
        // and always in one column, with the other chats' unread count over it.
        let back = back || self.one_column();
        let unread = self
            .one_column()
            .then(|| self.others_unread_badge(chat, cx))
            .flatten()
            .map(|(count, muted)| {
                div()
                    .absolute()
                    .right_0()
                    .top(px(BACK_UNREAD_TOP))
                    .child(self.count_badge(&main_menu::short_count(count), muted, window))
            });
        let back_button = back.then(|| {
            div()
                .id("chat-back")
                .relative()
                .flex_none()
                .w(px(BACK_WIDTH))
                .h_full()
                .flex()
                .items_center()
                .pl(px(BACK_ICON_LEFT))
                .cursor_pointer()
                .child(icon(Icon::Back, p.box_title_close_fg, window))
                .children(unread)
                .on_click(cx.listener(|this, _, window, cx| this.header_back(window, cx)))
        });
        div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .bg(c(p.window_bg))
            .border_b_1()
            .border_color(border)
            .children(back_button)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .pl(px(if back { 0. } else { 16. }))
                    .pr(px(16.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(p.window_bold_fg))
                            .child(match &thread_header {
                                Some((title, _)) => one_line(title.clone()).into_any_element(),
                                None => self
                                    .name_with_badge(&chat.name, chat.badge, false, window)
                                    .into_any_element(),
                            }),
                    )
                    .when(!status.is_empty(), |d| {
                        d.child(div().text_color(c(status_fg)).child(one_line(status)))
                    }),
            )
            .child(self.render_top_bar_buttons(chat, window, cx))
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::profile::off("view") {
            // Performance experiment: an empty frame, every vsync.
            window.request_animation_frame();
            return div()
                .size_full()
                .bg(c(self.palette().window_bg))
                .into_any_element();
        }
        crate::profile::time("MainView::render", || self.render_view(window, cx))
    }
}

impl MainView {
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Every size below is converted for this scale as tdesktop does.
        scale::set(window.scale_factor());
        self.update_quote_color(cx);
        // Custom emoji: last frame's replaced images leave the GPU atlas; the next
        // animation frame (or a look at a loading emoji) is a timer, not every
        // vsync.
        crate::repaint::begin();
        crate::repaint::set_paused(!window.is_window_active());
        crate::custom_emoji::begin_frame(window);
        crate::vector::begin_frame(window);
        self.update_emoji_suggest(window, cx);
        // Custom emoji painted in the last frame and not known yet.
        let wanted = crate::custom_emoji::take_wanted();
        if !wanted.is_empty() {
            let _ = self.commands.send(Command::LoadCustomEmoji(wanted));
        }
        if !self.first_list_frame_logged && !self.dialogs.is_empty() {
            self.first_list_frame_logged = true;
            tracing::info!(
                elapsed = ?crate::STARTED.elapsed(),
                chats = self.dialogs.len(),
                "first frame with the chat list"
            );
        }
        self.window_w = f32::from(window.viewport_size().width);
        // The one-column slide: back to the list closes the chat when done; out of
        // one column there is nothing to slide.
        if let Some(slide) = self.column_slide
            && (slide.start.elapsed() >= SLIDE_DURATION || !self.one_column())
        {
            self.column_slide = None;
            if !slide.to_chat && self.selected.is_some() {
                self.close_chat(window, cx);
            }
        }
        self.tick_scroll_anim(window, cx);
        if let Some((peer, msg_id)) = self.text_ui.pending_open.take() {
            self.go_to_original(peer, msg_id, window, cx);
        }
        self.hide_spoilers_on_chat_change();
        #[cfg(windows)]
        crate::tray_menu::DARK.store(self.dark, std::sync::atomic::Ordering::Relaxed);
        if self.tick_code_hint() | self.tick_ripple() {
            window.request_animation_frame();
        }
        self.tick_chrome(window);
        if self
            .text_ui
            .toast
            .as_ref()
            .is_some_and(|t| t.level().is_none())
        {
            self.text_ui.toast = None;
        }
        if self
            .slide
            .as_ref()
            .is_some_and(|s| s.start.elapsed() >= SLIDE_DURATION)
        {
            self.slide = None;
        }
        if self.highlight.is_some() {
            if self.highlight_opacity().is_some() {
                window.request_animation_frame();
            } else {
                self.highlight = None;
            }
        }
        // A jumped-to message goes to the middle once it is laid out
        // (`itemTopForHighlight`).
        if let Some(ix) = self.center_on
            && let Some(item) = self.messages_state.bounds_for_item(ix)
        {
            self.center_on = None;
            let viewport = self.messages_state.viewport_bounds();
            let target = viewport.top() + (viewport.size.height - item.size.height) / 2.;
            let delta = item.top() - target;
            if delta.abs() > gpui::px(1.) {
                self.messages_state.scroll_by(delta);
            }
        }
        // A dragged scrollbar keeps its thumb under the mouse as the list grows.
        for area in [Area::History, Area::Dialogs, Area::Settings, Area::Emoji] {
            let (_, max, _) = self.scroll_metrics(area);
            if self
                .bar(area)
                .drag
                .as_ref()
                .is_some_and(|d| (d.applied_max - max).abs() > 0.5)
            {
                self.apply_scrollbar_drag(area);
            }
        }
        let bars_busy =
            self.history_bar.tick() | self.dialogs_bar.tick() | self.settings_bar.tick();
        // The floating date: shown while scrolling, hidden a second later, fading.
        let date_on = self
            .history_bar
            .last_scroll
            .is_some_and(|t| t.elapsed() < SCROLL_DATE_HIDE_AFTER);
        self.scroll_date.set(if date_on { 1. } else { 0. });
        if bars_busy
            || self.pin_drag.as_ref().is_some_and(PinDrag::animating)
            || self.scroll_date.animating()
            || self.to_down_shown.animating()
            || self.to_up_shown.animating()
        {
            window.request_animation_frame();
        }
        self.row_shifts
            .retain(|_, (_, start)| start.elapsed() < PINNED_SHIFT_DURATION);
        if !self.row_shifts.is_empty() {
            window.request_animation_frame();
        }
        let p = self.palette();
        let border = c(p.shadow_fg);
        let dialogs_width = self.dialogs_width(window);
        let narrow = self.narrow(window);

        let top_bar = self.render_dialogs_header(dialogs_width, narrow, window, cx);

        let skip = self.list_skip();
        let dialogs = uniform_list(
            "dialogs",
            self.dialogs.len() + skip,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                range
                    .filter_map(|i| {
                        if i < skip {
                            Some(this.render_archive_row(window, cx))
                        } else {
                            let item = this.dialogs.get(i - skip).cloned()?;
                            Some(this.render_dialog(&item, false, window, cx))
                        }
                    })
                    // Each item is laid out on its own, where a root's `top` offset
                    // is ignored: the row slides inside a fixed slot.
                    .map(|row| div().w_full().h(px(DIALOG_ROW_HEIGHT)).child(row))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.dialogs_scroll.clone())
        .size_full();

        // "Scroll to top" (`Dialogs::Widget::_scrollToTop`): once the list is scrolled
        // down by half of `historyToDownShownAfter`, and not at its very end; never in
        // the narrow column.
        let (dialogs_viewport, dialogs_max, dialogs_offset) = self.scroll_metrics(Area::Dialogs);
        // More chats when the end of the list is near (tdesktop preloads a few screens).
        if !self.dialogs.is_empty()
            && dialogs_max - dialogs_offset < 2. * dialogs_viewport
            && self.more_dialogs_requested != self.dialogs.len()
        {
            self.more_dialogs_requested = self.dialogs.len();
            let _ = self.commands.send(Command::LoadMoreDialogs);
        }
        // The dragged pinned chat, over the list.
        let floating_row = self.pin_drag.as_ref().and_then(|drag| {
            let index = self.dialogs.iter().position(|d| d.peer == drag.peer)?;
            let item = self.dialogs[index].clone();
            let top =
                (index + skip) as f32 * lg(DIALOG_ROW_HEIGHT) + drag.shown_yadd() - dialogs_offset;
            Some(
                div()
                    .absolute()
                    .left_0()
                    .top(px(top))
                    .w_full()
                    .child(self.render_dialog(&item, true, window, cx)),
            )
        });
        let show_to_up = !narrow
            && dialogs_offset > TO_DOWN_SHOWN_AFTER / 2.
            && dialogs_offset < dialogs_max - 0.5;
        if narrow {
            // `startScrollUpButtonAnimation`: no animation in the small column.
            self.to_up_shown = Fade::new(0., TO_DOWN_DURATION);
        } else {
            self.to_up_shown.set(if show_to_up { 1. } else { 0. });
        }
        let up_shown = self.to_up_shown.value();
        let to_up = (up_shown > 0.).then(|| {
            // `updateScrollUpPosition`: top = height - (button + margin) * shown.
            let slide = (lg(TO_DOWN_HEIGHT) + lg(TO_UP_BOTTOM)) * up_shown;
            self.render_jump_button("dialogs-to-up", Icon::UpArrow, 1., window, cx)
                .bottom(gpui::px(slide - lg(TO_DOWN_HEIGHT)))
                .on_click(cx.listener(|this, _, _, cx| {
                    // `scrollToDefaultChecked`: not again while it runs.
                    if this.scroll_anim.is_none() {
                        this.start_scroll_anim(Area::Dialogs, 0.);
                    }
                    cx.notify();
                }))
        });

        let dialogs_area = div()
            .id("dialogs-area")
            .relative()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.area_hovered(Area::Dialogs, *hovered, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.stop_scroll_anim(Area::Dialogs);
                this.scrolled(Area::Dialogs, cx);
            }))
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                this.row_press_moved(e.position.y, cx);
            }))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.row_press = None),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.row_press = None),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.row_press = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.row_press = None),
            )
            .child(self.folder_slide_view(dialogs.into_any_element(), dialogs_width, window, cx))
            .children(floating_row)
            .children(self.render_scrollbar(Area::Dialogs, cx))
            .children(to_up)
            .children(self.render_connecting(window, cx));

        let one_column = self.one_column();
        let tabs = if self.archive_open {
            None
        } else {
            self.render_folder_tabs(window, cx)
        };
        let forum_shown = self.forum_shown();
        let forum_view = self
            .forum
            .clone()
            .or_else(|| self.forum_hiding.as_ref().map(|h| h.0.clone()));
        // Settled: the narrowed list and the topics side by side; else the topics
        // slide over the wide list.
        let forum_open = self.forum.is_some() && forum_shown >= 1.;
        let top_bar = if forum_open {
            self.render_forum_strip_header(window, cx)
        } else if self.archive_open {
            self.render_archive_bar(window, cx)
        } else {
            top_bar
        };
        let strip = lg(threads::FORUM_STRIP_WIDTH).min(dialogs_width);
        let live_list = div()
            .when(forum_open, |d| d.w(gpui::px(strip)).flex_none().h_full())
            .when(!forum_open, |d| d.size_full())
            .flex()
            .flex_col()
            .bg(c(p.dialogs_bg))
            .child(top_bar)
            .children(tabs.filter(|_| !forum_open))
            .child(dialogs_area);
        let topics_width = (dialogs_width - strip).max(0.);
        let live_list = match forum_view {
            Some(forum) if forum_open => {
                let topics = self.render_topics_column(&forum, 1., topics_width, window, cx);
                div().size_full().flex().child(live_list).child(topics)
            }
            Some(forum) => {
                let topics =
                    self.render_topics_column(&forum, forum_shown, topics_width, window, cx);
                let strip_header = self.render_forum_strip_header(window, cx);
                div()
                    .relative()
                    .size_full()
                    .child(live_list)
                    // The narrow top (back arrow) fades in over the wide one.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(gpui::px(strip))
                            .opacity(forum_shown)
                            .child(strip_header),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(gpui::px(strip))
                            .h_full()
                            .child(topics),
                    )
            }
            None => live_list,
        };
        let list_content = self.archive_slide_view(live_list, dialogs_width, narrow, window, cx);
        let left = div()
            .w(gpui::px(dialogs_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(c(p.dialogs_bg))
            .when(!one_column, |d| d.border_r_1().border_color(border))
            .child(list_content);

        let day = {
            let top = if self.selected.is_some() {
                TOP_BAR_HEIGHT
            } else {
                0.
            };
            self.day_background(top, window, cx)
        };
        let right = match self.selected.and_then(|peer| self.chat_item(peer)) {
            None => self
                .with_chat_background(div(), day)
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(12.))
                        .bg(c(p.msg_service_bg))
                        .text_color(c(p.msg_service_fg))
                        .child(
                            self.fatal
                                .clone()
                                .unwrap_or_else(|| "Select a chat to start messaging".into()),
                        ),
                ),
            Some(chat) => {
                let chat = chat.clone();
                let header =
                    self.render_header(&chat, border, !self.back_stack.is_empty(), window, cx);
                let view = cx.entity();
                let weak = view.downgrade();
                let messages = list(self.messages_state.clone(), move |i, window, cx| {
                    view.read(cx).render_message(i, window, &weak)
                })
                .size_full()
                .pb(px(8.));
                let (_, max, offset) = self.scroll_metrics(Area::History);
                let show_to_down = max - offset > TO_DOWN_SHOWN_AFTER;
                self.to_down_shown.set(if show_to_down { 1. } else { 0. });
                let down_shown = self.to_down_shown.value();
                let to_down = (down_shown > 0.).then(|| {
                    let slide = (lg(TO_DOWN_HEIGHT) + lg(TO_DOWN_BOTTOM)) * down_shown;
                    self.render_jump_button("history-to-down", Icon::DownArrow, 0., window, cx)
                        .bottom(gpui::px(slide - lg(TO_DOWN_HEIGHT)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            // From a window around an older message: the newest ones.
                            if this.newer_available {
                                this.jump_to_latest();
                            } else {
                                this.start_scroll_anim(Area::History, f32::MAX);
                            }
                            cx.notify();
                        }))
                });
                let column = service_column(self.chat_width(window));
                let date_opacity = self.scroll_date.value();
                let floating_date = self.floating_date().map(|text| {
                    let w = text_width(window, &text, FONT_SIZE, FontWeight::SEMIBOLD).ceil() + 1.;
                    div()
                        .opacity(date_opacity)
                        .absolute()
                        .top(px(SERVICE_MARGIN_TOP))
                        .left_0()
                        .w(px(column))
                        .flex()
                        .justify_center()
                        .child(service_pill(p, text, w))
                });
                let scrollbar = self.render_scrollbar(Area::History, cx);
                let feed = self
                    .with_chat_background(div(), day)
                    .id("history")
                    .relative()
                    .overflow_hidden()
                    .flex_1()
                    .min_h(px(0.))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.area_hovered(Area::History, *hovered, cx);
                    }))
                    .child(messages)
                    .children(floating_date)
                    .children(scrollbar)
                    .children(to_down);
                let chat_h = f32::from(window.viewport_size().height) - lg(TITLE_HEIGHT);
                let feed = feed.children(self.render_emoji_panel(chat_h, window, cx));
                let has_text = !self.input.read(cx).is_empty();
                let field_extras = self.render_field_extras(self.chat_width(window), window, cx);
                let reply_bar = self.render_reply_bar(window, cx);
                let replying = reply_bar.is_some();
                let composer = div()
                    .relative()
                    .flex_none()
                    .min_h(px(46.))
                    .flex()
                    .items_end()
                    .bg(c(p.history_compose_area_bg))
                    .text_color(c(p.history_compose_area_fg))
                    .when(!replying, |d| d.border_t_1().border_color(border))
                    .child(self.render_attach(window, cx))
                    .child(
                        div()
                            .id("composer-field")
                            .flex_1()
                            .min_w(px(0.))
                            .py(gpui::px((lg(46.) - scale::font(FONT_SIZE).height) / 2.))
                            .cursor(gpui::CursorStyle::IBeam)
                            // The whole field area takes presses, not just its text
                            // lines (tdesktop's field fills the composer's height). The
                            // default is prevented: the history's focus tracking would
                            // take the focus back.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                                    window.prevent_default();
                                    window.focus(&this.input.focus_handle(cx));
                                    let (position, shift, count) =
                                        (e.position, e.modifiers.shift, e.click_count);
                                    this.input
                                        .update(cx, |i, cx| i.press_at(position, shift, count, cx));
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, e: &gpui::MouseDownEvent, window, cx| {
                                    window.prevent_default();
                                    window.focus(&this.input.focus_handle(cx));
                                    this.open_field_menu(e.position, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.input.update(cx, |i, _| i.release());
                                }),
                            )
                            .child(self.input.clone()),
                    )
                    .child(
                        // `TabbedPanel::eventFilter`: the panel follows the mouse over
                        // the button.
                        div()
                            .id("compose-emoji-hover")
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                this.emoji_button_hovered(*hovered, cx);
                            }))
                            .child(self.render_emoji(window, cx)),
                    )
                    .child(self.render_send(has_text, window, cx))
                    .children(field_extras);
                let live = div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(feed)
                    .child(
                        self.thread
                            .is_none()
                            .then(|| {
                                self.render_compose_bar(&chat, self.chat_width(window), window, cx)
                            })
                            .flatten()
                            .unwrap_or_else(|| {
                                div()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .when(replying, |d| d.border_t_1().border_color(border))
                                    .children(reply_bar)
                                    .child(composer)
                                    .into_any_element()
                            }),
                    );
                self.with_slide(live, border, window, cx)
            }
        };
        // One column: the list or the chat, or both while one slides over the other.
        let main_columns: Vec<AnyElement> = if one_column {
            match self.column_slide {
                Some(slide) => {
                    let over = div().size_full().flex().child(right);
                    let width = f32::from(window.viewport_size().width) - self.filters_width();
                    vec![
                        self.column_slide_view(
                            div().size_full().child(left),
                            over,
                            slide,
                            width,
                            window,
                        )
                        .into_any_element(),
                    ]
                }
                None if self.selected.is_some() => vec![right.into_any_element()],
                None => vec![left.into_any_element()],
            }
        } else {
            let body_h = f32::from(window.viewport_size().height) - lg(TITLE_HEIGHT);
            let mut columns = vec![left.into_any_element(), right.into_any_element()];
            if let Some(section) = self.render_emoji_section(body_h, window, cx) {
                columns.push(section);
            }
            columns
        };
        let layers = self.render_layers(window, cx);
        // The login screens and the account slide over each other
        // (`Window::SlideAnimation` from the right, `MainWindow::setupIntro` /
        // `setupMain`): the arriving one over, the other shifting under it.
        let slide = self
            .intro_slide
            .take()
            .filter(|s| s.start.elapsed() < SLIDE_DURATION);
        let intro_view = self.intro.as_ref().map(|(i, _)| i.clone());
        let leaving_intro = slide.as_ref().and_then(|s| s.leaving.clone());
        let show_main = intro_view.is_none() || slide.is_some();
        // Focus tracking stays below the title bar: a focusable element swallows mouse
        // downs (prevent_default), and GPUI then skips the native caption-button
        // handling (minimize / maximize / close / drag would do nothing).
        let main_body = if show_main {
            Some(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .track_focus(&self.focus)
                    .children(self.render_folders(window, cx))
                    .children(main_columns)
                    // The column edge (`_firstColumnResizeArea`): at the chat's left side.
                    .when(!one_column, |d| {
                        d.child(
                            div()
                                .id("column-resize")
                                .absolute()
                                .top_0()
                                .left(px(dialogs_width + self.filters_width()))
                                .w(px(RESIZE_WIDTH))
                                .h_full()
                                .cursor(gpui::CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                                        this.column_drag = Some(ColumnDrag {
                                            start_mouse_x: e.position.x,
                                            start_width: dialogs_width,
                                        });
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        )
                    }),
            )
        } else {
            None
        };
        let intro_body = |intro: Entity<crate::intro::IntroView>| {
            div()
                .relative()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .child(div().size_full().child(intro))
        };
        let width = f32::from(window.viewport_size().width);
        let body: AnyElement = match (intro_view, main_body, slide.as_ref()) {
            // "Add Account": the login over the account.
            (Some(intro), Some(main), Some(s)) => {
                let start = s.start;
                self.column_slide_view(
                    main,
                    intro_body(intro),
                    ColumnSlide {
                        start,
                        to_chat: true,
                    },
                    width,
                    window,
                )
                .into_any_element()
            }
            (Some(intro), _, _) => intro_body(intro).into_any_element(),
            // Back, or logged in: the account over the login screens.
            (None, Some(main), Some(s)) => match &leaving_intro {
                Some(old) => self
                    .column_slide_view(
                        intro_body(old.clone()),
                        main,
                        ColumnSlide {
                            start: s.start,
                            to_chat: true,
                        },
                        width,
                        window,
                    )
                    .into_any_element(),
                None => main.into_any_element(),
            },
            (None, Some(main), None) => main.into_any_element(),
            (None, None, _) => div().into_any_element(),
        };
        self.intro_slide = slide;

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(FONT_FAMILY)
            .text_size(scale::font(FONT_SIZE).size)
            .line_height(scale::font(FONT_SIZE).line_height())
            .bg(c(p.window_bg))
            .text_color(c(p.window_fg))
            // On the root: with nothing focused, GPUI dispatches keys to the root only.
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(|this, _: &crate::text_input::Copy, _, cx| {
                this.copy_selected_text(cx);
            }))
            .child(self.render_title_bar(window, cx))
            .child(body)
            .children(layers)
            .children(self.render_emoji_suggestions(window, cx))
            .children(self.render_drag_capture(cx))
            .children(self.render_menu(window, cx))
            .children(self.text_ui.toast.as_ref().and_then(|t| {
                let size = window.viewport_size();
                let title = lg(TITLE_HEIGHT);
                crate::toast::render(
                    t,
                    p,
                    0.,
                    title,
                    f32::from(size.width),
                    f32::from(size.height) - title,
                    window,
                )
            }))
            .into_any_element()
    }
}
