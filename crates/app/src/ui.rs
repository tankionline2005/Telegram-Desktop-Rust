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
    ListState, MouseButton, PathBuilder, Pixels, RenderImage, Rgba, SharedString, TextRun,
    UniformListScrollHandle, Window, WindowControlArea, canvas, div, font, img, linear_color_stop,
    linear_gradient, list, point, prelude::*, rgba, uniform_list,
};
use tdr_data::describe::PeerKind as ChatKind;
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Command, DialogItem, MessageItem, Update};
use crate::icons::{Icon, icon, icon_width};
use crate::one_line::one_line;
use crate::placement::{COLUMN_MIN_LEFT, Columns};
use crate::present;
use crate::scale::{self, dpx, lg, px};
use crate::text_input::{Submit, TextInput};

gpui::actions!(main_view, [Escape]);

/// Window-wide keys (the text field's own keys are in `text_input::bind_keys`).
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new("escape", Escape, None)]);
}

// widgets.style: windowTitleHeight, windowTitleButtonWidth
const TITLE_HEIGHT: f32 = 24.;
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
// window_main_menu.style: mainMenuWidth
const MAIN_MENU_WIDTH: f32 = 274.;
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
fn text_width(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
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

/// The widths of the lines `text` wraps into at `wrap` (GPUI's own wrapping, the one
/// that paints it), paragraph by paragraph.
fn wrapped_line_widths(
    window: &Window,
    text: &str,
    size: f32,
    weight: FontWeight,
    wrap: f32,
) -> Vec<f32> {
    let mut f = font(FONT_FAMILY);
    f.weight = weight;
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push(0.);
            continue;
        }
        let run = TextRun {
            len: paragraph.len(),
            font: f.clone(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_text(
            SharedString::from(paragraph.to_string()),
            px(size),
            &[run],
            Some(gpui::px(wrap)),
            None,
        );
        let Ok(lines) = shaped else {
            out.push(text_width(window, paragraph, size, weight));
            continue;
        };
        for line in lines {
            let layout = &line.unwrapped_layout;
            let mut start = 0.;
            for b in line.wrap_boundaries() {
                let x = layout
                    .runs
                    .get(b.run_ix)
                    .and_then(|r| r.glyphs.get(b.glyph_ix))
                    .map_or(start, |g| f32::from(g.position.x));
                out.push(x - start);
                start = x;
            }
            out.push(f32::from(layout.width) - start);
        }
    }
    out
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
    use gpui::{PathStyle, StrokeOptions};
    use lyon::tessellation::{LineCap, LineJoin};
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
    let at = |x: f32, y: f32| {
        point(
            bounds.left() + gpui::px(x / scale),
            bounds.top() + gpui::px(y / scale),
        )
    };
    let stroke = |join: LineJoin| {
        PathStyle::Stroke(
            StrokeOptions::default()
                .with_line_width(thickness / scale)
                .with_line_cap(LineCap::Butt)
                .with_line_join(join),
        )
    };
    let mut upper =
        PathBuilder::stroke(gpui::px(thickness / scale)).with_style(stroke(LineJoin::Round));
    upper.move_to(at(left, half));
    upper.line_to(at(left, top));
    upper.line_to(at(right, top));
    upper.line_to(at(right, half));
    let mut lower =
        PathBuilder::stroke(gpui::px(thickness / scale)).with_style(stroke(LineJoin::Miter));
    lower.move_to(at(left, half));
    lower.line_to(at(left, bottom));
    lower.line_to(at(middle, bottom - add));
    lower.line_to(at(right, bottom));
    lower.line_to(at(right, half));
    for path in [upper, lower] {
        if let Ok(path) = path.build() {
            window.paint_path(path, c(fg));
        }
    }
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
    /// The row's offset from its slot.
    yadd: f32,
    start_order: Vec<PeerKey>,
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
struct ScrollDrag {
    start_mouse_y: Pixels,
    start_offset: Pixels,
}

/// One overlay scrollbar (tdesktop's `ScrollArea`): shown while scrolling and on hover.
#[derive(Default)]
struct Scrollbar {
    last_scroll: Option<Instant>,
    hovered: bool,
    drag: Option<ScrollDrag>,
}

impl Scrollbar {
    fn visible(&self) -> bool {
        self.last_scroll
            .is_some_and(|t| t.elapsed() < SCROLL_HIDE_AFTER)
            || self.hovered
            || self.drag.is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    History,
    Dialogs,
}

impl Area {
    /// The scrollbar's (width, corner radius).
    fn bar_style(self) -> (f32, f32) {
        match self {
            Area::History => (12., 3.),
            Area::Dialogs => (10., 2.),
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
    /// The main menu's "Night Mode" row is under the mouse.
    menu_hovered: bool,
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
    /// The text the input field had at the last check (to notice typing).
    input_text: String,
    /// Bumped on every edit: pending draft-save timers check it.
    draft_generation: u64,
    /// The first unsaved edit (tdesktop's `_saveDraftStart`).
    draft_dirty_since: Option<Instant>,
    main_menu_open: bool,
    /// The caption button under the mouse.
    title_hovered: Option<&'static str>,
    /// The "scroll down" / "scroll to top" button under the mouse.
    jump_hovered: Option<&'static str>,
    input: Entity<TextInput>,
    status: String,
    fatal: Option<String>,
    commands: mpsc::UnboundedSender<Command>,
    focus: FocusHandle,
    /// Font with the Windows caption-button glyphs.
    icon_font: SharedString,
    first_list_frame_logged: bool,
    /// Demo/screenshot helper: open the chat at this list index once the list arrives.
    pub open_on_start: Option<usize>,
}

impl MainView {
    pub fn new(
        commands: mpsc::UnboundedSender<Command>,
        mut updates: mpsc::UnboundedReceiver<Update>,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = tdr_theme::palette(dark);
        let input = cx.new(|cx| TextInput::new("Write a message…", palette, cx));
        cx.observe(&input, |this: &mut Self, _, cx| this.input_changed(cx))
            .detach();
        cx.subscribe(&input, |this: &mut Self, _, Submit(text), cx| {
            if let Some(peer) = this.selected {
                // Sending clears the draft (the data thread clears the stored one).
                this.drafts.insert(peer, None);
                this.input_text.clear();
                this.draft_dirty_since = None;
                this.draft_generation += 1;
                this.stick_to_bottom = true;
                // tdesktop drops the bar once you write.
                this.unread_bar = None;
                let _ = this.commands.send(Command::Send {
                    peer,
                    text: text.clone(),
                });
            }
            cx.notify();
        })
        .detach();

        // Updates from the data thread.
        cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                if this.update(cx, |view, cx| view.apply(update, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        let messages_state = ListState::new(0, ListAlignment::Bottom, px(600.));
        let view = cx.entity().downgrade();
        messages_state.set_scroll_handler(move |event, _window, cx| {
            let near_top = event.visible_range.start <= LOAD_OLDER_THRESHOLD;
            let at_bottom = event.visible_range.end >= event.count;
            let visible_end = event.visible_range.end;
            let view = view.clone();
            cx.defer(move |cx| {
                let _ = view.update(cx, |v, cx| {
                    v.stick_to_bottom = at_bottom;
                    v.stop_scroll_anim(Area::History);
                    v.scrolled(Area::History, cx);
                    v.mark_read(visible_end);
                    if near_top {
                        v.load_older();
                    }
                });
            });
        });

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
            menu_hovered: false,
            day_background: None,
            day_background_busy: false,
            unread_target: None,
            unread_bar: None,
            read_till: 0,
            window_active: true,
            members: HashMap::new(),
            userpics: HashMap::new(),
            userpic_requests: RefCell::new(HashSet::new()),
            drafts: HashMap::new(),
            input_text: String::new(),
            draft_generation: 0,
            draft_dirty_since: None,
            main_menu_open: false,
            title_hovered: None,
            jump_hovered: None,
            input,
            status: String::new(),
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

    /// Where the chat list widths are kept; loads them.
    pub fn set_columns_path(&mut self, path: PathBuf) {
        self.columns = Columns::load(&path);
        self.columns_path = Some(path);
    }

    /// The chat list's width in this window.
    fn dialogs_width(&self, window: &Window) -> f32 {
        self.columns.width(
            self.selected.is_none(),
            f32::from(window.viewport_size().width),
        )
    }

    /// The chat list shows userpics only (tdesktop's `narrow` row context).
    fn narrow(&self, window: &Window) -> bool {
        self.dialogs_width(window) < COLUMN_MIN_LEFT / 2.
    }

    /// The open chat's width.
    fn chat_width(&self, window: &Window) -> f32 {
        f32::from(window.viewport_size().width) - self.dialogs_width(window)
    }

    fn dragging(&self) -> bool {
        self.column_drag.is_some()
            || self.pin_drag.is_some()
            || self.history_bar.drag.is_some()
            || self.dialogs_bar.drag.is_some()
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
            let body = f32::from(window.viewport_size().width);
            self.columns.drag_to(width, self.selected.is_none(), body);
            cx.notify();
        }
        self.drag_scrollbar(Area::History, e.position.y, cx);
        self.drag_scrollbar(Area::Dialogs, e.position.y, cx);
    }

    fn drag_ended(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.row_press = None;
        self.finish_pin_drag(cx);
        if self.column_drag.take().is_some() {
            let body = f32::from(window.viewport_size().width);
            self.columns.drag_finished(self.selected.is_none(), body);
            if let Some(path) = &self.columns_path {
                self.columns.save(path);
            }
            cx.notify();
        }
        self.end_drag(Area::History, cx);
        self.end_drag(Area::Dialogs, cx);
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
        self.pin_drag = Some(PinDrag {
            peer,
            start_y,
            yadd: 0.,
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
        let height = DIALOG_ROW_HEIGHT;
        let threshold = (height / 2.).max(height - height / 2.);
        let now = Instant::now();
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
            .insert(drag.peer, (drag.yadd, Instant::now()));
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

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Dialogs(mut list) => {
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
                if let Some(i) = self.open_on_start.take()
                    && let Some(peer) = self.dialogs.get(i).map(|d| d.peer)
                {
                    self.start_opening(peer);
                }
            }
            Update::History {
                peer,
                messages,
                complete,
            } => {
                if self.selected == Some(peer) {
                    self.history_complete = complete;
                    self.loading_older = false;
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
                    self.set_input_text(text.unwrap_or_default(), cx);
                }
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
            Update::Status(s) => self.status = s,
            Update::Fatal(e) => self.fatal = Some(e),
        }
        cx.notify();
    }

    /// Replaces the shown messages, keeping the scroll position (or the bottom). The
    /// first history of a chat with unread messages opens at the first unread one with
    /// the "Unread messages" bar, as in tdesktop.
    fn set_messages(&mut self, new: Vec<MessageItem>) {
        let mut reveal = None;
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
            self.messages_state
                .scroll_to_reveal_item(self.messages.len() - 1);
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
        }
    }

    fn bar(&self, area: Area) -> &Scrollbar {
        match area {
            Area::History => &self.history_bar,
            Area::Dialogs => &self.dialogs_bar,
        }
    }

    /// A list scrolled: show its scrollbar (and the floating date) for a moment.
    fn scrolled(&mut self, area: Area, cx: &mut Context<Self>) {
        self.bar_mut(area).last_scroll = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SCROLL_HIDE_AFTER + Duration::from_millis(20))
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
        self.input_text = text.clone();
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// The field may have changed: tdesktop's draft timers. The local draft is saved a
    /// second after typing stops (at the latest every 5 s while typing on), the cloud
    /// one after 14 s without typing.
    fn input_changed(&mut self, cx: &mut Context<Self>) {
        const SAVE_DRAFT: Duration = Duration::from_millis(1000); // kSaveDraftTimeout
        const SAVE_DRAFT_ANYWAY: Duration = Duration::from_secs(5); // kSaveDraftAnywayTimeout
        const SAVE_CLOUD_DRAFT_IDLE: Duration = Duration::from_secs(14); // kSaveCloudDraftIdleTimeout
        let text = self.input.read(cx).text();
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
        if self.main_menu_open {
            self.main_menu_open = false;
        } else if self.selected.is_some() {
            self.save_draft(cx);
            self.set_input_text(String::new(), cx);
            self.selected = None;
            self.messages.clear();
            self.messages_state.reset(0);
            self.history_bar.drag = None;
            self.unread_bar = None;
            self.unread_target = None;
            let _ = self.commands.send(Command::CloseChat);
            window.focus(&self.focus);
        } else {
            return;
        }
        cx.notify();
    }

    /// Selects a chat and asks for its history.
    fn start_opening(&mut self, peer: PeerKey) {
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
        self.stick_to_bottom = true;
        let _ = self.commands.send(Command::OpenChat(peer));
    }

    fn open(&mut self, peer: PeerKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(peer) {
            return;
        }
        // Keep the unsent text of the chat we leave; restore the one of the chat we open.
        self.save_draft(cx);
        let draft = self.draft_of(peer).unwrap_or_default();
        self.set_input_text(draft, cx);
        self.start_opening(peer);
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        let palette = self.palette();
        self.input
            .update(cx, |input, cx| input.set_palette(palette, cx));
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
        empty_userpic(p, size, initials, color, saved).into_any_element()
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
                    None => div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(c(text_fg))
                        .child(one_line(last.text.clone())),
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
                    .child(one_line(item.name.clone())),
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
    fn attached(&self, a: &MessageItem, b: &MessageItem) -> bool {
        a.service.is_none()
            && b.service.is_none()
            && a.out == b.out
            && a.author == b.author
            && (b.date - a.date).abs() < ATTACH_SECONDS
            && present::day_of(a.date) == present::day_of(b.date)
            && self.unread_bar != Some(b.id)
    }

    /// A message row: a date divider when the day changes, the "Unread messages" bar,
    /// then the message — a bubble or a service line.
    fn render_message(&self, i: usize, window: &Window) -> AnyElement {
        let Some(item) = self.messages.get(i) else {
            return div().into_any_element();
        };
        let p = self.palette();
        let chat_width = self.chat_width(window);
        let column = service_column(chat_width);
        let day = present::day_of(item.date);
        let new_day = i == 0 || present::day_of(self.messages[i - 1].date) != day;
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
        let unread_bar = (self.unread_bar == Some(item.id)).then(|| {
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
            None => self.render_bubble(i, item, chat_width, window),
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
        i: usize,
        item: &MessageItem,
        chat_width: f32,
        window: &Window,
    ) -> AnyElement {
        let p = self.palette();
        let chat = self
            .selected
            .and_then(|peer| self.dialogs.iter().find(|d| d.peer == peer));
        let is_group = chat.is_some_and(|d| d.is_group());
        let wide = chat_width >= CHAT_WIDE_WIDTH;
        let right_side = item.out && !wide;
        let with_userpic = if wide { true } else { is_group && !item.out };

        let prev_attached = i > 0 && self.attached(&self.messages[i - 1], item);
        let next_attached = self
            .messages
            .get(i + 1)
            .is_some_and(|n| self.attached(item, n));

        let (bg, fg, date_fg) = if item.out {
            (p.msg_out_bg, p.history_text_out_fg, p.msg_out_date_fg)
        } else {
            (p.msg_in_bg, p.history_text_in_fg, p.msg_in_date_fg)
        };
        let reserved = MSG_SIDE + MSG_MARGIN_FAR + if with_userpic { MSG_PHOTO_SKIP } else { 0. };
        let (pad_x, pad_y) = (lg(MSG_PADDING_X), lg(MSG_PADDING_Y));
        let max_text =
            (lg(MSG_MAX_WIDTH).min(chat_width - lg(reserved)) - 2. * pad_x).max(lg(120.));
        let text_font = scale::font(FONT_SIZE);
        let date_font = scale::font(DATE_FONT_SIZE);
        let sender = item.sender.as_ref().filter(|_| !prev_attached);
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
        // `BottomInfo`: the date, then `historySendStateSpace` for the checks. The text
        // ends with a skip block that size (`skipBlockWidth/Height`): on its last line
        // when it fits there, otherwise on a line of its own.
        let date_w = text_width(window, &time_text, DATE_FONT_SIZE, FontWeight::NORMAL).ceil();
        let info_w = date_w
            + if item.read.is_some() {
                lg(SEND_STATE_SPACE)
            } else {
                0.
            };
        let skip_w = lg(MSG_DATE_SPACE) + info_w - lg(MSG_DATE_DELTA_X);
        let skip_h = date_font.height - lg(MSG_DATE_DELTA_Y);
        let lines = wrapped_line_widths(window, &text, FONT_SIZE, FontWeight::NORMAL, max_text);
        let last = lines.last().copied().unwrap_or(0.);
        let skip_inline = last + skip_w <= max_text;
        let text_w = lines
            .iter()
            .copied()
            .fold(0., f32::max)
            .max(if skip_inline { last + skip_w } else { skip_w });
        let text_h = lines.len() as f32 * text_font.height + if skip_inline { 0. } else { skip_h };
        let name_w = sender.map_or(0., |(name, _)| {
            text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD)
        });
        let media_w =
            media_line.map_or(0., |m| text_width(window, m, FONT_SIZE, FontWeight::NORMAL));
        // +1 px: sub-pixel rounding must not push the last word to a new line.
        let inner = ((text_w.max(name_w).max(media_w) + 1.).min(max_text + 1.) * scale::get())
            .ceil()
            / scale::get();

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
            .bottom(gpui::px(pad_y - lg(MSG_DATE_DELTA_Y)))
            .w(gpui::px(info_w))
            .h(date_font.line_height())
            .text_size(date_font.size)
            .line_height(date_font.line_height())
            .text_color(c(date_fg))
            .child(time_text)
            .children(item.read.map(|read| {
                let kind = if read {
                    Icon::HistoryReceived
                } else {
                    Icon::HistorySent
                };
                div()
                    .absolute()
                    .left(gpui::px(info_w + lg(SEND_STATE_X)))
                    .top(gpui::px(date_font.height + lg(SEND_STATE_Y)))
                    .child(icon(kind, p.history_out_icon_fg, window))
            }));
        // Captions stand in for media: in the date color when they are the whole text.
        let text_color = if item.text.is_empty() && media.is_some() {
            date_fg
        } else {
            fg
        };
        let bubble = div()
            .relative()
            .flex_none()
            // `msgShadow` under the bubble and its rounded corners.
            .shadow(vec![gpui::BoxShadow {
                color: c(shadow).into(),
                offset: point(gpui::px(0.), px(MSG_SHADOW)),
                blur_radius: gpui::px(0.),
                spread_radius: gpui::px(0.),
            }])
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
            .children(media_line.map(|m| {
                div()
                    .w(gpui::px(inner))
                    .h(text_font.line_height())
                    .text_color(c(date_fg))
                    .child(one_line(m.clone()))
            }))
            .child(
                div()
                    .w(gpui::px(inner))
                    .h(gpui::px(text_h))
                    .text_color(c(text_color))
                    .child(text),
            )
            .child(info);
        // The tail (`bubble_tail`) hangs outside the square corner; the shadow line
        // continues under it (`PaintBubbleGeneric`).
        let tail = (!next_attached).then(|| {
            let kind = if right_side {
                Icon::BubbleTailRight
            } else {
                Icon::BubbleTail
            };
            let tail_w = icon_width(kind, window);
            div()
                .absolute()
                .bottom(-px(MSG_SHADOW))
                .when(right_side, |d| d.right(gpui::px(-tail_w)))
                .when(!right_side, |d| d.left(gpui::px(-tail_w)))
                .w(gpui::px(tail_w))
                .flex()
                .flex_col()
                .child(icon(kind, bg, window))
                .child(div().w_full().h(px(MSG_SHADOW)).bg(c(shadow)))
        });
        let bubble = div().relative().flex_none().child(bubble).children(tail);

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
            .w_full()
            .flex()
            .items_end()
            .pt(px(if prev_attached {
                MSG_MARGIN_TOP_ATTACHED
            } else {
                MSG_MARGIN_TOP
            }))
            .pb(px(MSG_MARGIN_BOTTOM))
            .px(px(MSG_SIDE))
            .when(right_side, |d| d.justify_end())
            .children(avatar_slot)
            .child(bubble)
            .into_any_element()
    }

    fn render_main_menu(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let hovered = self.menu_hovered;
        // `ToggleView::paint` with `defaultSettingsToggle`: a 28x14 track in the
        // foreground color and a 16 px ring (2 px pen) filled with the window color.
        let toggle = {
            let on = self.dark;
            let fg = if on {
                p.window_bg_active
            } else if hovered {
                p.menu_icon_fg_over
            } else {
                p.menu_icon_fg
            };
            const BORDER: f32 = 2.;
            const SHIFT: f32 = 1.;
            const DIAMETER: f32 = 16.;
            const WIDTH: f32 = 14.;
            let knob_left = BORDER + if on { WIDTH } else { 0. };
            div()
                .relative()
                .flex_none()
                .w(px(DIAMETER + WIDTH + 2. * BORDER))
                .h(px(DIAMETER + 2. * BORDER))
                .child(
                    div()
                        .absolute()
                        .left(px(BORDER + SHIFT))
                        .top(px(BORDER + SHIFT))
                        .w(px(DIAMETER + WIDTH - 2. * SHIFT))
                        .h(px(DIAMETER - 2. * SHIFT))
                        .rounded(px((DIAMETER - 2. * SHIFT) / 2.))
                        .bg(c(fg)),
                )
                .child(
                    // The pen is centered on the circle's outline.
                    div()
                        .absolute()
                        .left(px(knob_left - BORDER / 2.))
                        .top(px(BORDER / 2.))
                        .size(px(DIAMETER + BORDER))
                        .rounded_full()
                        .border_2()
                        .border_color(c(fg))
                        .bg(c(p.window_bg)),
                )
        };
        let (row_bg, text_fg, icon_fg) = if hovered {
            (p.window_bg_over, p.window_bold_fg_over, p.menu_icon_fg_over)
        } else {
            (p.window_bg, p.window_bold_fg, p.menu_icon_fg)
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .child(
                div()
                    .id("main-menu")
                    .w(px(MAIN_MENU_WIDTH))
                    .h_full()
                    .bg(c(p.main_menu_bg))
                    .occlude()
                    .flex()
                    .flex_col()
                    .pt(px(8.))
                    .child(
                        // `mainMenuButton`: padding (61, 11, 20, 9), iconLeft 21,
                        // toggleSkip 19, semibold text.
                        div()
                            .id("menu-night-mode")
                            .relative()
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .bg(c(row_bg))
                            .cursor_pointer()
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                this.menu_hovered = *hovered;
                                cx.notify();
                            }))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(21.))
                                    .top_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .child(icon(Icon::NightMode, icon_fg, window)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .pl(px(61.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(text_fg))
                                    .child("Night Mode"),
                            )
                            .child(div().pr(px(19.)).child(toggle)),
                    ),
            )
            .child(
                div()
                    .id("main-menu-backdrop")
                    .flex_1()
                    .h_full()
                    .bg(c(p.layer_bg))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main_menu_open = false;
                        cx.notify();
                    })),
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
        let visible = bar_state.visible();
        let over = bar_state.hovered || bar_state.drag.is_some();
        let (track_bg, bar_bg, bar_bg_over) = match area {
            Area::History => (
                p.history_scroll_bg_over,
                p.history_scroll_bar_bg,
                p.history_scroll_bar_bg_over,
            ),
            Area::Dialogs => (p.scroll_bg_over, p.scroll_bar_bg, p.scroll_bar_bg_over),
        };
        let (width, round) = area.bar_style();
        let track = viewport - 2. * SCROLL_DELTA;
        let bar = (track * viewport / (viewport + max)).max(SCROLL_MIN_BAR);
        let bar_top = SCROLL_DELTA + (track - bar) * (offset / max);
        let id = match area {
            Area::History => "history-scroll",
            Area::Dialogs => "dialogs-scroll",
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
                    this.bar_mut(area).hovered = *hovered;
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        let (_, _, offset) = this.scroll_metrics(area);
                        this.bar_mut(area).drag = Some(ScrollDrag {
                            start_mouse_y: e.position.y,
                            start_offset: px(offset),
                        });
                        if area == Area::History {
                            this.messages_state.scrollbar_drag_started();
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .when(visible, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(SCROLL_DELTA))
                            .left(px(SCROLL_DELTA))
                            .w(px(width - 2. * SCROLL_DELTA))
                            .h(px(track))
                            .rounded(px(round))
                            .when(over, |d| d.bg(c(track_bg))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(bar_top))
                            .left(px(SCROLL_DELTA))
                            .w(px(width - 2. * SCROLL_DELTA))
                            .h(px(bar))
                            .rounded(px(round))
                            .bg(c(if over { bar_bg_over } else { bar_bg })),
                    )
                }),
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
            |top: f32, element: gpui::Img| div().absolute().top(px(top)).left_0().child(element);
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

    fn drag_scrollbar(&mut self, area: Area, mouse_y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &self.bar(area).drag else {
            return;
        };
        let (start_mouse_y, start_offset) = (drag.start_mouse_y, drag.start_offset);
        let (viewport, max, _) = self.scroll_metrics(area);
        let track = viewport - 2. * SCROLL_DELTA;
        let bar = (track * viewport / (viewport + max)).max(SCROLL_MIN_BAR);
        let movable = (track - bar).max(1.);
        let delta = f32::from(mouse_y - start_mouse_y) * max / movable;
        let target = (f32::from(start_offset) + delta).clamp(0., max);
        self.set_scroll(area, target);
        if area == Area::History {
            self.stick_to_bottom = target >= max - 1.;
        }
        self.bar_mut(area).last_scroll = Some(Instant::now());
        cx.notify();
    }

    fn end_drag(&mut self, area: Area, cx: &mut Context<Self>) {
        if self.bar_mut(area).drag.take().is_some() {
            if area == Area::History {
                self.messages_state.scrollbar_drag_ended();
            }
            cx.notify();
        }
    }

    /// While scrolling, the date of the topmost visible message floats at the top
    /// (tdesktop's `_scrollDateShown`).
    fn floating_date(&self) -> Option<String> {
        if !self.history_bar.visible() || self.history_bar.hovered {
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

    fn render_header(&self, chat: &DialogItem, border: Rgba) -> impl IntoElement + use<> {
        let p = self.palette();
        let status = self
            .members
            .get(&chat.peer)
            .cloned()
            .unwrap_or_else(|| chat.status.clone());
        let status_fg = if status == "online" {
            p.window_active_text_fg
        } else {
            p.window_sub_text_fg
        };
        div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .justify_center()
            .px(px(16.))
            .bg(c(p.window_bg))
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.window_bold_fg))
                    .child(one_line(chat.name.clone())),
            )
            .when(!status.is_empty(), |d| {
                d.child(div().text_color(c(status_fg)).child(one_line(status)))
            })
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every size below is converted for this scale as tdesktop does.
        scale::set(window.scale_factor());
        if !self.first_list_frame_logged && !self.dialogs.is_empty() {
            self.first_list_frame_logged = true;
            tracing::info!(
                elapsed = ?crate::STARTED.elapsed(),
                chats = self.dialogs.len(),
                "first frame with the chat list"
            );
        }
        self.tick_scroll_anim(window, cx);
        self.row_shifts
            .retain(|_, (_, start)| start.elapsed() < PINNED_SHIFT_DURATION);
        if !self.row_shifts.is_empty() {
            window.request_animation_frame();
        }
        let p = self.palette();
        let border = c(p.shadow_fg);
        let dialogs_width = self.dialogs_width(window);
        let narrow = self.narrow(window);

        let top_bar = div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            // Narrow: the menu button alone, centered over the userpics.
            .px(px(if narrow {
                (dialogs_width - 40.) / 2.
            } else {
                8.
            }))
            .gap(px(8.))
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .id("main-menu-toggle")
                    .size(px(40.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(c(p.window_bg_over)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main_menu_open = !this.main_menu_open;
                        cx.notify();
                    }))
                    .child(icon(Icon::DialogsMenu, p.dialogs_menu_icon_fg, window)),
            )
            .when(!narrow, |d| {
                d.child(
                    div()
                        .flex_1()
                        .text_color(c(p.window_sub_text_fg))
                        .child(self.status.clone()),
                )
            });

        let dialogs = uniform_list(
            "dialogs",
            self.dialogs.len(),
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                range
                    .filter_map(|i| this.dialogs.get(i).cloned())
                    .map(|item| this.render_dialog(&item, false, window, cx))
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
            let top = index as f32 * DIALOG_ROW_HEIGHT + drag.yadd - dialogs_offset;
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
        let to_up = show_to_up.then(|| {
            self.render_jump_button("dialogs-to-up", Icon::UpArrow, 1., window, cx)
                .bottom(px(TO_UP_BOTTOM))
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
            .child(dialogs)
            .children(floating_row)
            .children(self.render_scrollbar(Area::Dialogs, cx))
            .children(to_up);

        let left = div()
            .w(px(dialogs_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(p.dialogs_bg))
            .border_r_1()
            .border_color(border)
            .child(top_bar)
            .child(dialogs_area);

        let day = {
            let top = if self.selected.is_some() {
                TOP_BAR_HEIGHT
            } else {
                0.
            };
            self.day_background(top, window, cx)
        };
        let right = match self
            .selected
            .and_then(|peer| self.dialogs.iter().find(|d| d.peer == peer))
        {
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
                let header = self.render_header(chat, border);
                let view = cx.entity();
                let messages = list(self.messages_state.clone(), move |i, window, cx| {
                    view.read(cx).render_message(i, window)
                })
                .size_full()
                .pb(px(8.));
                let (_, max, offset) = self.scroll_metrics(Area::History);
                let show_to_down = max - offset > TO_DOWN_SHOWN_AFTER;
                let to_down = show_to_down.then(|| {
                    self.render_jump_button("history-to-down", Icon::DownArrow, 0., window, cx)
                        .bottom(px(TO_DOWN_BOTTOM))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_scroll_anim(Area::History, f32::MAX);
                            cx.notify();
                        }))
                });
                let column = service_column(self.chat_width(window));
                let floating_date = self.floating_date().map(|text| {
                    let w = text_width(window, &text, FONT_SIZE, FontWeight::SEMIBOLD).ceil() + 1.;
                    div()
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
                    .flex_1()
                    .min_h(px(0.))
                    .child(messages)
                    .children(floating_date)
                    .children(scrollbar)
                    .children(to_down);
                let composer = div()
                    .flex_none()
                    .min_h(px(46.))
                    .flex()
                    .items_end()
                    .pl(px(16.))
                    .bg(c(p.history_compose_area_bg))
                    .text_color(c(p.history_compose_area_fg))
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .id("composer-field")
                            .flex_1()
                            .min_w(px(0.))
                            .py(gpui::px((lg(46.) - scale::font(FONT_SIZE).height) / 2.))
                            .cursor(gpui::CursorStyle::IBeam)
                            // The whole field area focuses the input, not just its text line.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    window.focus(&this.input.focus_handle(cx))
                                }),
                            )
                            .child(self.input.clone()),
                    )
                    .child(
                        div()
                            .id("send")
                            .flex_none()
                            .w(px(44.))
                            .h(px(46.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let focus = this.input.focus_handle(cx);
                                window.focus(&focus);
                                window.dispatch_action(Box::new(crate::text_input::Enter), cx);
                            }))
                            .child(icon(Icon::Send, p.history_send_icon_fg, window)),
                    );
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(feed)
                    .child(composer)
            }
        };

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
            .child(self.render_title_bar(window, cx))
            // Focus tracking stays below the title bar: a focusable element swallows mouse
            // downs (prevent_default), and GPUI then skips the native caption-button
            // handling (minimize / maximize / close / drag would do nothing).
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .track_focus(&self.focus)
                    .child(left)
                    .child(right)
                    // The column edge (`_firstColumnResizeArea`): at the chat's left side.
                    .child(
                        div()
                            .id("column-resize")
                            .absolute()
                            .top_0()
                            .left(px(dialogs_width))
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
                    .when(self.main_menu_open, |d| {
                        d.child(self.render_main_menu(window, cx))
                    }),
            )
            .children(self.render_drag_capture(cx))
    }
}
