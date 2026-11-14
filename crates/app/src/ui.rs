//! The main window: custom title bar, chat list on the left, the open chat on the right.
//!
//! Sizes follow tdesktop's styles (`dialogs.style`, `ui/chat/chat.style`,
//! `lib_ui/ui/widgets/widgets.style`), colors come from its palette (`tdr-theme`).

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListOffset,
    ListState, Pixels, Rgba, SharedString, TextRun, Window, WindowControlArea, div, font, list,
    prelude::*, px, rgba, uniform_list,
};
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Command, DialogItem, MessageItem, Update};
use crate::text_input::{Submit, TextInput};

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
// chat.style: msgMaxWidth, msgPadding, bubbleRadiusLarge, msgMargin
const MSG_MAX_WIDTH: f32 = 430.;
const MSG_PADDING_X: f32 = 11.;
const MSG_PADDING_Y: f32 = 8.;
const MSG_RADIUS: f32 = 16.;
const MSG_MARGIN_TOP: f32 = 6.;
const MSG_SIDE: f32 = 16.;
const MSG_MARGIN_FAR: f32 = 56.;
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
fn one_line(text: String) -> gpui::Div {
    div()
        .w_full()
        .overflow_hidden()
        .line_clamp(1)
        .text_ellipsis()
        .child(text)
}

/// Width of the longest line of `text` when laid out without wrapping.
fn text_width(window: &Window, text: &str, size: f32, weight: FontWeight) -> Pixels {
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
            window
                .text_system()
                .layout_line(line, px(size), &[run], None)
                .width
        })
        .fold(px(0.), |a, b| a.max(b))
}

/// tdesktop's userpic colors in `color_index` order (red, orange, violet, green, cyan,
/// blue, pink).
fn userpic_color(p: &Palette, index: u8) -> Color {
    [
        p.history_peer1_userpic_bg,
        p.history_peer8_userpic_bg,
        p.history_peer5_userpic_bg,
        p.history_peer2_userpic_bg,
        p.history_peer7_userpic_bg,
        p.history_peer4_userpic_bg,
        p.history_peer6_userpic_bg,
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
                let _ = view.update(cx, |v, _| {
                    v.stick_to_bottom = at_bottom;
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

    /// Replaces the shown messages, telling the list exactly what changed so that the
    /// scroll position survives: older messages prepended, new ones appended, edits in
    /// place. (A full reset would jump to the bottom.)
    fn set_messages(&mut self, new: Vec<MessageItem>) {
        self.update_list(new);
        if self.stick_to_bottom && !self.messages.is_empty() {
            self.messages_state
                .scroll_to_reveal_item(self.messages.len() - 1);
        }
    }

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
                    .filter(|&i| new[prepended + i].text != old[i].text)
                    .collect();
                let appended = new.len() - prepended - old.len();
                let old_len = old.len();
                self.messages = new;
                if prepended > 0 {
                    self.messages_state.splice(0..0, prepended);
                }
                for i in changed {
                    let at = prepended + i;
                    self.messages_state.splice(at..at + 1, 1);
                }
                if appended > 0 {
                    let at = prepended + old_len;
                    self.messages_state.splice(at..at, appended);
                }
            }
            None => {
                // Deletions or a different chat: rebuild, keeping the position if the
                // user had scrolled up.
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

    fn open(&mut self, peer: PeerKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(peer) {
            return;
        }
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
        // Message colors changed: re-measure nothing, just repaint.
        cx.notify();
    }

    /// The custom title bar of tdesktop on Windows: no text, caption buttons on the right.
    fn render_title_bar(&self, window: &Window) -> impl IntoElement {
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
                div()
                    .id(id)
                    .w(px(TITLE_BUTTON_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(self.icon_font.clone())
                    .text_size(px(10.))
                    .text_color(c(fg))
                    .hover(move |s| s.bg(c(bg_over)).text_color(c(fg_over)))
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

    fn render_dialog(&self, item: &DialogItem, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette();
        let active = self.selected == Some(item.peer);
        let (bg, name_fg, text_fg, date_fg) = if active {
            (
                p.dialogs_bg_active,
                p.dialogs_name_fg_active,
                p.dialogs_text_fg_active,
                p.dialogs_date_fg_active,
            )
        } else {
            (
                p.dialogs_bg,
                p.dialogs_name_fg,
                p.dialogs_text_fg,
                p.dialogs_date_fg,
            )
        };
        let peer = item.peer;
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
            .child(
                div()
                    .flex_none()
                    .size(px(DIALOG_PHOTO))
                    .rounded_full()
                    .bg(c(userpic_color(p, item.color)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c(p.history_peer_userpic_fg))
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(item.initials.clone()),
            )
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
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_color(c(text_fg))
                                    .child(one_line(item.last.clone())),
                            )
                            .children(badge)
                            .when(item.unread == 0 && item.pinned, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(DATE_FONT_SIZE))
                                        .text_color(c(date_fg))
                                        .child("📌"),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    /// A message bubble with an explicit width: its natural text width, capped. GPUI 0.2.2
    /// otherwise measures wrapped text at one width and paints it at another, so long
    /// messages overlap the next ones.
    fn render_message(&self, item: &MessageItem, window: &Window) -> AnyElement {
        let p = self.palette();
        let (bg, fg, date_fg) = if item.out {
            (p.msg_out_bg, p.history_text_out_fg, p.msg_out_date_fg)
        } else {
            (p.msg_in_bg, p.history_text_in_fg, p.msg_in_date_fg)
        };
        let available =
            f32::from(window.viewport_size().width) - DIALOGS_WIDTH - MSG_SIDE - MSG_MARGIN_FAR;
        let max_inner = MSG_MAX_WIDTH.min(available).max(120.) - 2. * MSG_PADDING_X;
        let natural = [
            f32::from(text_width(
                window,
                &item.text,
                FONT_SIZE,
                FontWeight::NORMAL,
            )),
            item.sender.as_ref().map_or(0., |(name, _)| {
                f32::from(text_width(window, name, FONT_SIZE, FontWeight::SEMIBOLD))
            }),
            f32::from(text_width(
                window,
                &item.time,
                DATE_FONT_SIZE,
                FontWeight::NORMAL,
            )),
        ]
        .into_iter()
        .fold(0., f32::max);
        // +1 px: sub-pixel rounding must not push the last word to a new line.
        let inner = (natural + 1.).min(max_inner).ceil();

        let bubble = div()
            .flex_none()
            .px(px(MSG_PADDING_X))
            .py(px(MSG_PADDING_Y))
            .rounded(px(MSG_RADIUS))
            .bg(c(bg))
            .text_color(c(fg))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(item.sender.as_ref().map(|(name, color)| {
                div()
                    .w(px(inner))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_color(p, *color)))
                    .child(one_line(name.clone()))
            }))
            .child(div().w(px(inner)).child(item.text.clone()))
            .child(
                div()
                    .w(px(inner))
                    .flex()
                    .justify_end()
                    .text_size(px(DATE_FONT_SIZE))
                    .text_color(c(date_fg))
                    .child(item.time.clone()),
            );
        div()
            .w_full()
            .flex()
            .pt(px(MSG_MARGIN_TOP))
            .px(px(MSG_SIDE))
            .when(item.out, |d| d.justify_end())
            .child(bubble)
            .into_any_element()
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
            .px(px(16.))
            .gap(px(8.))
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.window_bold_fg))
                    .child(if self.status.is_empty() {
                        "Chats".to_string()
                    } else {
                        self.status.clone()
                    }),
            )
            .child(
                div()
                    .id("theme-toggle")
                    .size(px(32.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .text_color(c(p.menu_icon_fg))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(p.window_bg_over)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
                    .child(if self.dark { "☀" } else { "☾" }),
            );

        let dialogs = uniform_list(
            "dialogs",
            self.dialogs.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|i| this.dialogs.get(i).cloned())
                    .map(|item| this.render_dialog(&item, cx))
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
                    let view = view.read(cx);
                    match view.messages.get(i) {
                        Some(m) => view.render_message(m, window),
                        None => div().into_any_element(),
                    }
                })
                .flex_1()
                .pb(px(8.));
                let composer = div()
                    .flex_none()
                    .min_h(px(TOP_BAR_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .gap(px(10.))
                    .bg(c(p.history_compose_area_bg))
                    .text_color(c(p.history_compose_area_fg))
                    .border_t_1()
                    .border_color(border)
                    .child(div().flex_1().child(self.input.clone()))
                    .child(
                        div()
                            .text_color(c(p.history_send_icon_fg))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Send ↵"),
                    );
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .bg(c(p.window_bg_over))
                            .child(messages),
                    )
                    .child(composer)
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(FONT_FAMILY)
            .text_size(px(FONT_SIZE))
            .bg(c(p.window_bg))
            .text_color(c(p.window_fg))
            .child(self.render_title_bar(window))
            // Focus tracking stays below the title bar: a focusable element swallows mouse
            // downs (prevent_default), and GPUI then skips the native caption-button
            // handling (minimize / maximize / close / drag would do nothing).
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .track_focus(&self.focus)
                    .child(left)
                    .child(right),
            )
    }
}
