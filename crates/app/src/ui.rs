//! The main window: custom title bar, chat list on the left, the open chat on the right.
//!
//! Sizes follow tdesktop's styles (`dialogs.style`, `ui/chat/chat.style`,
//! `chat_helpers.style`, `window/window.style`, `lib_ui/ui/widgets/widgets.style`),
//! colors come from its palette (`tdr-theme`), icons from its resources (`icons`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListOffset,
    ListState, MouseButton, PathBuilder, Pixels, RenderImage, Rgba, SharedString, TextRun,
    UniformListScrollHandle, Window, WindowControlArea, canvas, div, font, img, linear_color_stop,
    linear_gradient, list, point, prelude::*, px, rgba, uniform_list,
};
use tdr_data::describe::PeerKind as ChatKind;
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Command, DialogItem, MessageItem, Update};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::present;
use crate::text_input::{LINE_HEIGHT, Submit, TextInput};

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
const DIALOGS_WIDTH: f32 = 320.;
const TOP_BAR_HEIGHT: f32 = 54.;
// dialogs.style: dialogsChatTypeSkip, dialogsUnread*
const CHAT_TYPE_SKIP: f32 = 3.;
const UNREAD_HEIGHT: f32 = 19.;
const UNREAD_PADDING: f32 = 5.;
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
/// Room after the time for the ✓ / ✓✓ of outgoing messages (`historySendStateSpace`
/// minus the gap already there).
const SEND_STATE_SPACE: f32 = 18.;
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

    /// The chat background (under messages and the empty-chat placeholder): the night
    /// theme's solid color, or the default wallpaper's gradient in the day theme.
    fn with_chat_background(&self, d: gpui::Div) -> gpui::Div {
        if self.dark {
            d.bg(c(tdr_theme::NIGHT_CHAT_BACKGROUND))
        } else {
            d.relative().child(crate::wallpaper::default_gradient())
        }
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
            Update::Dialogs(list) => {
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

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.stick_to_bottom = true;
        if !self.messages.is_empty() {
            self.messages_state
                .scroll_to_reveal_item(self.messages.len() - 1);
            self.mark_read(self.messages.len());
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
    fn render_dialog(
        &self,
        item: &DialogItem,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let peer = item.peer;
        let draft = (!active).then(|| self.draft_of(peer)).flatten();
        let has_draft = draft.is_some();
        // Unread badge: grey for muted chats (`UnreadBadgeStyle::muted`).
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
                .flex_none()
                .min_w(px(UNREAD_HEIGHT))
                .h(px(UNREAD_HEIGHT))
                .px(px(UNREAD_PADDING))
                .rounded(px(UNREAD_HEIGHT / 2.))
                .bg(c(badge_bg))
                .text_color(c(badge_fg))
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
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
                    .min_w(px(0.))
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
        // Group / channel / bot icon before the name (`ChatTypeIcon`).
        let type_icon = match item.kind {
            ChatKind::Group => Some((Icon::ChatGroup, 1.)),
            ChatKind::Channel => Some((Icon::ChatChannel, 3.)),
            ChatKind::Bot => Some((Icon::ChatBot, 1.)),
            ChatKind::User => None,
        }
        .filter(|_| !item.is_saved)
        .map(|(kind, left)| {
            div()
                .flex_none()
                .pl(px(left))
                .pr(px(CHAT_TYPE_SKIP))
                .child(icon(kind, icon_fg, window))
        });
        // ✓ / ✓✓ of our last message, left of the time.
        let send_state = item.last_read.filter(|_| !has_draft).map(|read| {
            let kind = if read {
                Icon::DialogsReceived
            } else {
                Icon::DialogsSent
            };
            div().flex_none().child(icon(kind, sent_fg, window))
        });
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
            .child(self.userpic(
                DIALOG_PHOTO,
                Some(peer),
                item.photo,
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
                            .gap(px(5.))
                            .children(type_icon)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(name_fg))
                                    .child(one_line(item.name.clone())),
                            )
                            .children(send_state)
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
                                let pin_fg = if active {
                                    p.dialogs_unread_bg_muted_active
                                } else {
                                    p.dialogs_unread_bg_muted
                                };
                                d.child(icon(Icon::DialogsPinned, pin_fg, window))
                            }),
                    ),
            )
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
        let chat_width = f32::from(window.viewport_size().width) - DIALOGS_WIDTH;
        let day = present::day_of(item.date);
        let new_day = i == 0 || present::day_of(self.messages[i - 1].date) != day;
        let today = chrono::Local::now().date_naive();
        let divider = new_day.then_some(day).flatten().map(|day| {
            let text = present::day_divider(day, today);
            let w = text_width(window, &text, FONT_SIZE, FontWeight::SEMIBOLD).ceil() + 1.;
            div()
                .w_full()
                .flex()
                .justify_center()
                .pt(px(SERVICE_MARGIN_TOP))
                .pb(px(SERVICE_MARGIN_BOTTOM))
                .child(service_pill(p, text, w))
        });
        let unread_bar = (self.unread_bar == Some(item.id)).then(|| {
            div()
                .w_full()
                .mt(px(UNREAD_BAR_MARGIN))
                .h(px(UNREAD_BAR_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .bg(c(p.history_unread_bar_bg))
                .border_t_1()
                .border_b_1()
                .border_color(c(p.history_unread_bar_border))
                .text_color(c(p.history_unread_bar_fg))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Unread messages")
        });
        let body = match &item.service {
            Some(text) => {
                let max =
                    SERVICE_MAX_WIDTH.min(chat_width - 2. * MSG_SIDE - 2. * SERVICE_PADDING_X);
                let w = (text_width(window, text, FONT_SIZE, FontWeight::SEMIBOLD) + 1.)
                    .min(max)
                    .ceil();
                div()
                    .w_full()
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
        let max_inner = MSG_MAX_WIDTH.min(chat_width - reserved).max(120.) - 2. * MSG_PADDING_X;
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
        let text_w = text_width(window, &text, FONT_SIZE, FontWeight::NORMAL);
        let send_state_w = if item.read.is_some() {
            SEND_STATE_SPACE
        } else {
            0.
        };
        let time_w =
            text_width(window, &time_text, DATE_FONT_SIZE, FontWeight::NORMAL) + send_state_w;
        let name_w = sender.map_or(0., |(name, _)| {
            text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD)
        });
        let media_w =
            media_line.map_or(0., |m| text_width(window, m, FONT_SIZE, FontWeight::NORMAL));
        // The time sits on the last line when it fits there (single-line text), as in
        // tdesktop; otherwise on its own line.
        let time_gap = 8.;
        let inline_time = !text.contains('\n') && text_w + time_gap + time_w <= max_inner;
        let natural = if inline_time {
            text_w + time_gap + time_w
        } else {
            text_w.max(time_w)
        }
        .max(name_w)
        .max(media_w);
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
            .flex()
            .items_center()
            .text_size(px(DATE_FONT_SIZE))
            .text_color(c(date_fg))
            .child(time_text)
            .children(item.read.map(|read| {
                let kind = if read {
                    Icon::HistoryReceived
                } else {
                    Icon::HistorySent
                };
                div()
                    .flex_none()
                    .pl(px(3.))
                    .child(icon(kind, p.history_out_icon_fg, window))
            }));
        // Captions stand in for media: in the date color when they are the whole text.
        let text_color = if item.text.is_empty() && media.is_some() {
            date_fg
        } else {
            fg
        };
        let body = if inline_time {
            div()
                .w(px(inner))
                .flex()
                .items_end()
                .justify_between()
                .gap(px(time_gap))
                .child(div().text_color(c(text_color)).child(text))
                .child(time)
        } else {
            div()
                .w(px(inner))
                .flex()
                .flex_col()
                .child(div().w(px(inner)).text_color(c(text_color)).child(text))
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
            .children(media_line.map(|m| {
                div()
                    .w(px(inner))
                    .text_color(c(date_fg))
                    .child(one_line(m.clone()))
            }))
            .child(body);

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
                .w(px(SCROLL_WIDTH))
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
                            .w(px(SCROLL_WIDTH - 2. * SCROLL_DELTA))
                            .h(px(track))
                            .rounded(px(3.))
                            .when(over, |d| d.bg(c(track_bg))),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(bar_top))
                            .left(px(SCROLL_DELTA))
                            .w(px(SCROLL_WIDTH - 2. * SCROLL_DELTA))
                            .h(px(bar))
                            .rounded(px(3.))
                            .bg(c(if over { bar_bg_over } else { bar_bg })),
                    )
                }),
        )
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
        .track_scroll(self.dialogs_scroll.clone())
        .size_full();

        let dialogs_area = div()
            .id("dialogs-area")
            .relative()
            .flex_1()
            .min_h(px(0.))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.scrolled(Area::Dialogs, cx)))
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                this.drag_scrollbar(Area::Dialogs, e.position.y, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_drag(Area::Dialogs, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_drag(Area::Dialogs, cx)),
            )
            .child(dialogs)
            .children(self.render_scrollbar(Area::Dialogs, cx));

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
            .child(dialogs_area);

        let right = match self
            .selected
            .and_then(|peer| self.dialogs.iter().find(|d| d.peer == peer))
        {
            None => self
                .with_chat_background(div())
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
                let floating_date = self.floating_date().map(|text| {
                    let w = text_width(window, &text, FONT_SIZE, FontWeight::SEMIBOLD).ceil() + 1.;
                    div()
                        .absolute()
                        .top(px(SERVICE_MARGIN_TOP))
                        .left_0()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(service_pill(p, text, w))
                });
                let scrollbar = self.render_scrollbar(Area::History, cx);
                let feed = self
                    .with_chat_background(div())
                    .id("history")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, cx| {
                        this.drag_scrollbar(Area::History, e.position.y, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_drag(Area::History, cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_drag(Area::History, cx)),
                    )
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
                    .when(self.main_menu_open, |d| {
                        d.child(self.render_main_menu(window, cx))
                    }),
            )
    }
}
