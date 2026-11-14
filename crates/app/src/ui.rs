//! The main window: custom title bar, chat list on the left, the open chat on the right.
//!
//! Sizes follow tdesktop's styles (`dialogs.style`, `ui/chat/chat.style`,
//! `chat_helpers.style`, `window/window.style`, `lib_ui/ui/widgets/widgets.style`),
//! colors come from its palette (`tdr-theme`), icons from its resources (`icons`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListOffset,
    ListState, MouseButton, PathBuilder, Pixels, Rgba, SharedString, TextRun, Window,
    WindowControlArea, canvas, div, font, linear_color_stop, linear_gradient, list, point,
    prelude::*, px, rgba, uniform_list,
};
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Author, Command, DialogItem, MessageItem, Update};
use crate::icons::{Icon, icon};
use crate::text_input::{LINE_HEIGHT, Submit, TextInput};

// widgets.style: windowTitleHeight, windowTitleButtonWidth
const TITLE_HEIGHT: f32 = 24.;
const TITLE_BUTTON_WIDTH: f32 = 36.;
// dialogs.style: defaultDialogRow
const DIALOG_ROW_HEIGHT: f32 = 62.;
const DIALOG_PHOTO: f32 = 46.;
const DIALOG_PADDING_X: f32 = 10.;
const DIALOG_PADDING_Y: f32 = 8.;
const DIALOGS_WIDTH: f32 = 320.;
const TOP_BAR_HEIGHT: f32 = 54.;
// window.style: adaptiveChatWideWidth
const CHAT_WIDE_WIDTH: f32 = 880.;
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
/// Messages of one author closer than this are drawn as a group
/// (tdesktop: `kAttachMessageToPreviousSecondsDelta`).
const ATTACH_SECONDS: i32 = 900;
// chat_helpers.style: historyToDown*, historyScroll
const TO_DOWN_WIDTH: f32 = 52.;
const TO_DOWN_HEIGHT: f32 = 62.;
const TO_DOWN_RIGHT: f32 = 12.;
const TO_DOWN_BOTTOM: f32 = 10.;
const TO_DOWN_SHOWN_AFTER: f32 = 480.;
const SCROLL_WIDTH: f32 = 12.;
const SCROLL_DELTA: f32 = 3.;
const SCROLL_MIN_BAR: f32 = 20.;
const SCROLL_HIDE_AFTER: Duration = Duration::from_millis(1000);
// basic.style: fsize
const FONT_SIZE: f32 = 13.;
const DATE_FONT_SIZE: f32 = 12.;
const FONT_FAMILY: &str = "Open Sans";
/// Ask for older messages when the top of the list is this close (in items).
const LOAD_OLDER_THRESHOLD: usize = 3;

fn c(color: Color) -> Rgba {
    rgba(color.rgba())
}

/// One line of text cut with "…". (GPUI 0.2.2 caches the size of `nowrap` text from its
/// first, unconstrained measurement and then never truncates it; a one-line clamp of
/// wrapping text is re-measured with the real width.)
fn one_line(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .w_full()
        .overflow_hidden()
        .line_clamp(1)
        .text_ellipsis()
        .child(text.into())
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

/// tdesktop's two-color userpic gradients, in `color_index` order
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
fn userpic(p: &Palette, size: f32, initials: &str, color: u8, saved: bool) -> gpui::Div {
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

/// The bookmark from tdesktop's `PaintSavedMessagesInner`, same proportions.
fn paint_bookmark(bounds: gpui::Bounds<Pixels>, size: f32, fg: Color, window: &mut Window) {
    let s = size.round();
    let thickness = (s * 0.055).round();
    let increment = (thickness as i32 % 2 + s as i32 % 2) as f32;
    let width = (s * 0.15).round() * 2. + increment;
    let height = (s * 0.19).round() * 2. + increment;
    let add = (s * 0.064).round();
    let left = (s - width) / 2.;
    let top = (s - height) / 2.;
    let (right, bottom) = (left + width, top + height);
    let middle = (left + right) / 2.;
    let half = (top + bottom) / 2.;
    let at = |x: f32, y: f32| point(bounds.left() + px(x), bounds.top() + px(y));
    let mut b = PathBuilder::stroke(px(thickness));
    b.move_to(at(right, half));
    b.line_to(at(right, top));
    b.line_to(at(left, top));
    b.line_to(at(left, bottom));
    b.line_to(at(middle, bottom - add));
    b.line_to(at(right, bottom));
    b.line_to(at(right, half));
    if let Ok(path) = b.build() {
        window.paint_path(path, c(fg));
    }
}

/// Scrollbar drag in progress: where it started.
struct ScrollDrag {
    start_mouse_y: Pixels,
    start_offset: Pixels,
}

pub struct MainView {
    dark: bool,
    dialogs: Vec<DialogItem>,
    selected: Option<PeerKey>,
    messages: Vec<MessageItem>,
    messages_state: ListState,
    history_complete: bool,
    loading_older: bool,
    /// Keep the newest message in view (true until the user scrolls up, and again once
    /// they scroll back to the bottom).
    stick_to_bottom: bool,
    last_scroll: Option<Instant>,
    scroll_hovered: bool,
    scroll_drag: Option<ScrollDrag>,
    /// Unsent text per chat (tdesktop keeps a local draft per history).
    drafts: HashMap<PeerKey, String>,
    main_menu_open: bool,
    /// The caption button under the mouse.
    title_hovered: Option<&'static str>,
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
        cx.subscribe(&input, |this: &mut Self, _, Submit(text), cx| {
            if let Some(peer) = this.selected {
                this.drafts.remove(&peer);
                this.stick_to_bottom = true;
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
            let view = view.clone();
            cx.defer(move |cx| {
                let _ = view.update(cx, |v, cx| {
                    v.stick_to_bottom = at_bottom;
                    v.scrolled(cx);
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
            selected: None,
            messages: Vec::new(),
            messages_state,
            history_complete: false,
            loading_older: false,
            stick_to_bottom: true,
            last_scroll: None,
            scroll_hovered: false,
            scroll_drag: None,
            drafts: HashMap::new(),
            main_menu_open: false,
            title_hovered: None,
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

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Dialogs(list) => {
                if self.dialogs.is_empty() {
                    tracing::info!(elapsed = ?crate::STARTED.elapsed(), chats = list.len(), "ui: chat list received");
                }
                self.dialogs = list;
                if let Some(i) = self.open_on_start.take()
                    && let Some(peer) = self.dialogs.get(i).map(|d| d.peer)
                {
                    self.selected = Some(peer);
                    let _ = self.commands.send(Command::OpenChat(peer));
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
            Update::Status(s) => self.status = s,
            Update::Fatal(e) => self.fatal = Some(e),
        }
        cx.notify();
    }

    /// Replaces the shown messages, keeping the scroll position (or the bottom).
    fn set_messages(&mut self, new: Vec<MessageItem>) {
        self.update_list(new);
        if self.stick_to_bottom && !self.messages.is_empty() {
            self.messages_state
                .scroll_to_reveal_item(self.messages.len() - 1);
        }
    }

    /// Tells the list exactly what changed: older messages prepended, new ones appended,
    /// edits in place. (A full reset would lose the position.) Neighbours of changed
    /// messages are re-measured too: grouping changes their margins and corners.
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

    fn load_older(&mut self) {
        if self.loading_older || self.history_complete || self.messages.is_empty() {
            return;
        }
        if let Some(peer) = self.selected {
            self.loading_older = true;
            let _ = self.commands.send(Command::LoadOlder(peer));
        }
    }

    /// The list scrolled: show the scrollbar for a moment.
    fn scrolled(&mut self, cx: &mut Context<Self>) {
        self.last_scroll = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SCROLL_HIDE_AFTER + Duration::from_millis(20))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn open(&mut self, peer: PeerKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(peer) {
            return;
        }
        // Keep the unsent text of the chat we leave; restore the one of the chat we open.
        let text = self.input.read(cx).text();
        if let Some(prev) = self.selected {
            if text.trim().is_empty() {
                self.drafts.remove(&prev);
            } else {
                self.drafts.insert(prev, text);
            }
        }
        let draft = self.drafts.get(&peer).cloned().unwrap_or_default();
        self.input.update(cx, |input, cx| input.set_text(draft, cx));

        self.selected = Some(peer);
        self.messages.clear();
        self.messages_state.reset(0);
        self.history_complete = false;
        self.loading_older = false;
        self.stick_to_bottom = true;
        let _ = self.commands.send(Command::OpenChat(peer));
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

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.stick_to_bottom = true;
        if !self.messages.is_empty() {
            self.messages_state
                .scroll_to_reveal_item(self.messages.len() - 1);
        }
        cx.notify();
    }

    /// Content height and scroll offset of the message list (loaded messages only, as in
    /// tdesktop).
    fn scroll_metrics(&self) -> (f32, f32, f32) {
        let viewport = f32::from(self.messages_state.viewport_bounds().size.height);
        let max = f32::from(self.messages_state.max_offset_for_scrollbar().height);
        let offset = -f32::from(self.messages_state.scroll_px_offset_for_scrollbar().y);
        (viewport, max, offset.clamp(0., max))
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

    fn render_dialog(
        &self,
        item: &DialogItem,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let active = self.selected == Some(item.peer);
        let (bg, name_fg, text_fg, date_fg, draft_fg, pin_fg) = if active {
            (
                p.dialogs_bg_active,
                p.dialogs_name_fg_active,
                p.dialogs_text_fg_active,
                p.dialogs_date_fg_active,
                p.dialogs_draft_fg_active,
                p.dialogs_unread_bg_muted_active,
            )
        } else {
            (
                p.dialogs_bg,
                p.dialogs_name_fg,
                p.dialogs_text_fg,
                p.dialogs_date_fg,
                p.dialogs_draft_fg,
                p.dialogs_unread_bg_muted,
            )
        };
        let peer = item.peer;
        let draft = (!active).then(|| self.drafts.get(&peer)).flatten();
        let badge = (item.unread > 0).then(|| {
            let (badge_bg, badge_fg) = if active {
                (p.dialogs_unread_bg_active, p.dialogs_unread_fg_active)
            } else {
                (p.dialogs_unread_bg, p.dialogs_unread_fg)
            };
            div()
                .flex_none()
                .min_w(px(20.))
                .h(px(20.))
                .px(px(6.))
                .rounded(px(10.))
                .bg(c(badge_bg))
                .text_color(c(badge_fg))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .flex()
                .items_center()
                .justify_center()
                .child(if item.unread > 9999 {
                    "9999+".to_string()
                } else {
                    item.unread.to_string()
                })
        });
        let last_line = match draft {
            Some(d) => div()
                .flex()
                .min_w(px(0.))
                .child(div().flex_none().text_color(c(draft_fg)).child("Draft: "))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(c(text_fg))
                        .child(one_line(d.replace('\n', " "))),
                ),
            None => div()
                .text_color(c(text_fg))
                .child(one_line(item.last.clone())),
        };
        div()
            .id(SharedString::from(format!(
                "dialog-{:?}-{}",
                peer.kind, peer.id
            )))
            .h(px(DIALOG_ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .px(px(DIALOG_PADDING_X))
            .py(px(DIALOG_PADDING_Y))
            .gap(px(12.))
            .bg(c(bg))
            .when(!active, |d| d.hover(|s| s.bg(c(p.dialogs_bg_over))))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.open(peer, window, cx)))
            .child(userpic(
                p,
                DIALOG_PHOTO,
                &item.initials,
                item.color,
                item.is_saved,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(name_fg))
                                    .child(one_line(item.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(DATE_FONT_SIZE))
                                    .text_color(c(date_fg))
                                    .child(item.time.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w(px(0.)).child(last_line))
                            .children(badge)
                            .when(item.unread == 0 && item.pinned, |d| {
                                d.child(icon(Icon::DialogsPinned, pin_fg, window))
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Whether messages `a` and `b` (in this order) belong to one group.
    fn attached(a: &MessageItem, b: &MessageItem) -> bool {
        a.out == b.out && a.author == b.author && (b.date - a.date).abs() < ATTACH_SECONDS
    }

    /// A message row. tdesktop's layouts: in a wide chat (≥ 880 px) every message is on
    /// the left with userpics; in a narrow one outgoing messages go right and userpics
    /// are shown for group members only. Consecutive messages of one author form a
    /// group: no gap, small corners on the group side, userpic and name once.
    ///
    /// Bubbles get an explicit width from the measured text: GPUI 0.2.2 otherwise
    /// measures wrapped text at one width and paints it at another.
    fn render_message(&self, i: usize, window: &Window) -> AnyElement {
        let Some(item) = self.messages.get(i) else {
            return div().into_any_element();
        };
        let p = self.palette();
        let chat = self
            .selected
            .and_then(|peer| self.dialogs.iter().find(|d| d.peer == peer));
        let is_group = chat.is_some_and(|d| d.is_group);
        let chat_width = f32::from(window.viewport_size().width) - DIALOGS_WIDTH;
        let wide = chat_width >= CHAT_WIDE_WIDTH;
        let right_side = item.out && !wide;
        let with_userpic = if wide { true } else { is_group && !item.out };

        let prev_attached = i > 0 && Self::attached(&self.messages[i - 1], item);
        let next_attached = self
            .messages
            .get(i + 1)
            .is_some_and(|n| Self::attached(item, n));

        let (bg, fg, date_fg) = if item.out {
            (p.msg_out_bg, p.history_text_out_fg, p.msg_out_date_fg)
        } else {
            (p.msg_in_bg, p.history_text_in_fg, p.msg_in_date_fg)
        };
        let reserved = MSG_SIDE + MSG_MARGIN_FAR + if with_userpic { MSG_PHOTO_SKIP } else { 0. };
        let max_inner = MSG_MAX_WIDTH.min(chat_width - reserved).max(120.) - 2. * MSG_PADDING_X;
        let sender = item.sender.as_ref().filter(|_| !prev_attached);
        let text_w = text_width(window, &item.text, FONT_SIZE, FontWeight::NORMAL);
        let time_w = text_width(window, &item.time, DATE_FONT_SIZE, FontWeight::NORMAL);
        let name_w = sender.map_or(0., |(name, _)| {
            text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD)
        });
        // The time sits on the last line when it fits there (single-line text), as in
        // tdesktop; otherwise on its own line.
        let time_gap = 8.;
        let inline_time = !item.text.contains('\n') && text_w + time_gap + time_w <= max_inner;
        let natural = if inline_time {
            text_w + time_gap + time_w
        } else {
            text_w.max(time_w)
        }
        .max(name_w);
        // +1 px: sub-pixel rounding must not push the last word to a new line.
        let inner = (natural + 1.).min(max_inner).ceil();

        let small = px(MSG_RADIUS_SMALL);
        let large = px(MSG_RADIUS_LARGE);
        // The group side is the one with the userpic (left) or, for outgoing messages in
        // a narrow chat, the right side.
        let (tl, tr, bl, br) = if right_side {
            (
                large,
                if prev_attached { small } else { large },
                large,
                if next_attached { small } else { large },
            )
        } else {
            (
                if prev_attached { small } else { large },
                large,
                if next_attached { small } else { large },
                large,
            )
        };

        let time = div()
            .flex_none()
            .text_size(px(DATE_FONT_SIZE))
            .text_color(c(date_fg))
            .child(item.time.clone());
        let body = if inline_time {
            div()
                .w(px(inner))
                .flex()
                .items_end()
                .justify_between()
                .gap(px(time_gap))
                .child(div().child(item.text.clone()))
                .child(time)
        } else {
            div()
                .w(px(inner))
                .flex()
                .flex_col()
                .child(div().w(px(inner)).child(item.text.clone()))
                .child(div().w(px(inner)).flex().justify_end().child(time))
        };
        let bubble = div()
            .flex_none()
            .px(px(MSG_PADDING_X))
            .py(px(MSG_PADDING_Y))
            .rounded_tl(tl)
            .rounded_tr(tr)
            .rounded_bl(bl)
            .rounded_br(br)
            .bg(c(bg))
            .text_color(c(fg))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(sender.map(|(name, color)| {
                div()
                    .w(px(inner))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_color(p, *color)))
                    .child(one_line(name.clone()))
            }))
            .child(body);

        let avatar_slot = with_userpic.then(|| {
            let Author {
                initials, color, ..
            } = &item.author;
            div()
                .flex_none()
                .w(px(MSG_PHOTO_SKIP))
                .flex()
                .items_end()
                .when(!next_attached, |d| {
                    d.child(userpic(p, MSG_PHOTO_SIZE, initials, *color, false))
                })
        });
        div()
            .w_full()
            .flex()
            .items_end()
            .pt(px(if prev_attached {
                MSG_MARGIN_TOP_ATTACHED + 2.
            } else {
                MSG_MARGIN_TOP
            }))
            .px(px(MSG_SIDE))
            .when(right_side, |d| d.justify_end())
            .children(avatar_slot)
            .child(bubble)
            .into_any_element()
    }

    fn render_main_menu(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette();
        let toggle = {
            let (border, fill, knob_left) = if self.dark {
                (p.window_bg_active, p.window_bg_active, px(14.))
            } else {
                (p.menu_icon_fg, p.window_bg, px(0.))
            };
            div()
                .relative()
                .flex_none()
                .w(px(30.))
                .h(px(16.))
                .rounded(px(8.))
                .border_2()
                .border_color(c(border))
                .bg(c(fill))
                .child(
                    div()
                        .absolute()
                        .top(px(-2.))
                        .left(knob_left - px(2.))
                        .size(px(16.))
                        .rounded_full()
                        .border_2()
                        .border_color(c(border))
                        .bg(c(p.window_bg)),
                )
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
                        div()
                            .id("menu-night-mode")
                            .h(px(44.))
                            .px(px(24.))
                            .flex()
                            .items_center()
                            .gap(px(20.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c(p.menu_bg_over)))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                            .child(icon(Icon::NightMode, p.menu_icon_fg, window))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(c(p.window_bold_fg))
                                    .child("Night Mode"),
                            )
                            .child(toggle),
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

    /// The overlay scrollbar of the message list (`historyScroll`): shown while scrolling
    /// and on hover, measured over the loaded messages.
    fn render_scrollbar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let p = self.palette();
        let (viewport, max, offset) = self.scroll_metrics();
        if max <= 1. || viewport <= 0. {
            return None;
        }
        let recently = self
            .last_scroll
            .is_some_and(|t| t.elapsed() < SCROLL_HIDE_AFTER);
        let visible = recently || self.scroll_hovered || self.scroll_drag.is_some();
        let track = viewport - 2. * SCROLL_DELTA;
        let bar = (track * viewport / (viewport + max)).max(SCROLL_MIN_BAR);
        let bar_top = SCROLL_DELTA + (track - bar) * (offset / max);
        let over = self.scroll_hovered || self.scroll_drag.is_some();
        Some(
            div()
                .id("history-scroll")
                .absolute()
                .top_0()
                .right_0()
                .w(px(SCROLL_WIDTH))
                .h_full()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.scroll_hovered = *hovered;
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        let (_, _, offset) = this.scroll_metrics();
                        this.scroll_drag = Some(ScrollDrag {
                            start_mouse_y: e.position.y,
                            start_offset: px(offset),
                        });
                        this.messages_state.scrollbar_drag_started();
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
                            .w(px(SCROLL_WIDTH - 2. * SCROLL_DELTA))
                            .h(px(track))
                            .rounded(px(3.))
                            .when(over, |d| d.bg(c(p.history_scroll_bg_over))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(bar_top))
                            .left(px(SCROLL_DELTA))
                            .w(px(SCROLL_WIDTH - 2. * SCROLL_DELTA))
                            .h(px(bar))
                            .rounded(px(3.))
                            .bg(c(if over {
                                p.history_scroll_bar_bg_over
                            } else {
                                p.history_scroll_bar_bg
                            })),
                    )
                }),
        )
    }

    fn drag_scrollbar(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &self.scroll_drag else {
            return;
        };
        let (viewport, max, _) = self.scroll_metrics();
        let track = viewport - 2. * SCROLL_DELTA;
        let bar = (track * viewport / (viewport + max)).max(SCROLL_MIN_BAR);
        let movable = (track - bar).max(1.);
        let delta = f32::from(mouse_y - drag.start_mouse_y) * max / movable;
        let target = (f32::from(drag.start_offset) + delta).clamp(0., max);
        self.messages_state
            .set_offset_from_scrollbar(point(px(0.), px(-target)));
        self.stick_to_bottom = target >= max - 1.;
        self.last_scroll = Some(Instant::now());
        cx.notify();
    }
}

impl Focusable for MainView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_list_frame_logged && !self.dialogs.is_empty() {
            self.first_list_frame_logged = true;
            tracing::info!(
                elapsed = ?crate::STARTED.elapsed(),
                chats = self.dialogs.len(),
                "first frame with the chat list"
            );
        }
        let p = self.palette();
        let border = c(p.shadow_fg);

        let top_bar = div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(8.))
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
            .child(
                div()
                    .flex_1()
                    .text_color(c(p.window_sub_text_fg))
                    .child(self.status.clone()),
            );

        let dialogs = uniform_list(
            "dialogs",
            self.dialogs.len(),
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                range
                    .filter_map(|i| this.dialogs.get(i).cloned())
                    .map(|item| this.render_dialog(&item, window, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1();

        let left = div()
            .w(px(DIALOGS_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(p.dialogs_bg))
            .border_r_1()
            .border_color(border)
            .child(top_bar)
            .child(dialogs);

        let right = match self
            .selected
            .and_then(|peer| self.dialogs.iter().find(|d| d.peer == peer))
        {
            None => div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(c(p.window_bg_over))
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
                let header = div()
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
                            .child(chat.name.clone()),
                    )
                    .when(chat.is_channel, |d| {
                        d.child(div().text_color(c(p.window_sub_text_fg)).child("channel"))
                    });
                let view = cx.entity();
                let messages = list(self.messages_state.clone(), move |i, window, cx| {
                    view.read(cx).render_message(i, window)
                })
                .size_full()
                .pb(px(8.));
                let (_, max, offset) = self.scroll_metrics();
                let show_to_down = max - offset > TO_DOWN_SHOWN_AFTER;
                let to_down = show_to_down.then(|| {
                    div()
                        .id("history-to-down")
                        .absolute()
                        .right(px(TO_DOWN_RIGHT))
                        .bottom(px(TO_DOWN_BOTTOM))
                        .w(px(TO_DOWN_WIDTH))
                        .h(px(TO_DOWN_HEIGHT))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.scroll_to_bottom(cx)))
                        .child(div().absolute().top(px(10.)).left_0().child(icon(
                            Icon::DownShadow,
                            p.history_to_down_shadow,
                            window,
                        )))
                        .child(div().absolute().top(px(10.)).left_0().child(icon(
                            Icon::DownCircle,
                            p.history_to_down_bg,
                            window,
                        )))
                        .child(div().absolute().top(px(10.)).left_0().child(icon(
                            Icon::DownArrow,
                            p.history_to_down_fg,
                            window,
                        )))
                });
                let scrollbar = self.render_scrollbar(cx);
                let feed = div()
                    .id("history")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(c(p.window_bg_over))
                    .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                        if this.scroll_drag.is_some() {
                            this.drag_scrollbar(e.position.y, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.scroll_drag.take().is_some() {
                                this.messages_state.scrollbar_drag_ended();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.scroll_drag.take().is_some() {
                                this.messages_state.scrollbar_drag_ended();
                                cx.notify();
                            }
                        }),
                    )
                    .child(messages)
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
                            .py(px((46. - LINE_HEIGHT) / 2.))
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
            .text_size(px(FONT_SIZE))
            .bg(c(p.window_bg))
            .text_color(c(p.window_fg))
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
                    .when(self.main_menu_open, |d| {
                        d.child(self.render_main_menu(window, cx))
                    }),
            )
    }
}
