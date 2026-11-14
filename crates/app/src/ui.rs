//! The main window: chat list on the left, the open chat on the right.
//!
//! Sizes follow tdesktop's styles (`dialogs.style`, `ui/chat/chat.style`), colors come
//! from its palette (`tdr-theme`).

use gpui::{
    AnyElement, Context, Entity, FocusHandle, Focusable, FontWeight, ListAlignment, ListState,
    Rgba, SharedString, Window, div, list, prelude::*, px, rgba, uniform_list,
};
use tdr_data::tdr_storage::PeerKey;
use tdr_theme::{Color, Palette};
use tokio::sync::mpsc;

use crate::bridge::{Command, DialogItem, MessageItem, Update};
use crate::text_input::{Submit, TextInput};

// dialogs.style: defaultDialogRow
const DIALOG_ROW_HEIGHT: f32 = 62.;
const DIALOG_PHOTO: f32 = 46.;
const DIALOG_PADDING_X: f32 = 10.;
const DIALOG_PADDING_Y: f32 = 8.;
const DIALOGS_WIDTH: f32 = 320.;
const TOP_BAR_HEIGHT: f32 = 54.;
// chat.style
const MSG_MAX_WIDTH: f32 = 430.;
const MSG_PADDING_X: f32 = 11.;
const MSG_PADDING_Y: f32 = 8.;
const MSG_RADIUS: f32 = 16.;
const MSG_MARGIN_TOP: f32 = 6.;
const MSG_SIDE: f32 = 16.;
// basic.style: fsize
const FONT_SIZE: f32 = 13.;
const FONT_FAMILY: &str = "Open Sans";

fn c(color: Color) -> Rgba {
    rgba(color.rgba())
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

pub struct MainView {
    dark: bool,
    dialogs: Vec<DialogItem>,
    selected: Option<PeerKey>,
    messages: Vec<MessageItem>,
    messages_state: ListState,
    input: Entity<TextInput>,
    status: String,
    fatal: Option<String>,
    first_list_frame_logged: bool,
    /// Demo/screenshot helper: open the chat at this list index once the list arrives.
    pub open_on_start: Option<usize>,
    commands: mpsc::UnboundedSender<Command>,
    focus: FocusHandle,
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

        Self {
            dark,
            dialogs: Vec::new(),
            selected: None,
            messages: Vec::new(),
            messages_state: ListState::new(0, ListAlignment::Bottom, px(400.)),
            input,
            status: String::new(),
            fatal: None,
            open_on_start: None,
            first_list_frame_logged: false,
            commands,
            focus: cx.focus_handle(),
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
            Update::History { peer, messages } => {
                if self.selected == Some(peer) {
                    let stick_to_bottom = true;
                    self.messages = messages;
                    self.messages_state.reset(self.messages.len());
                    if stick_to_bottom && !self.messages.is_empty() {
                        self.messages_state
                            .scroll_to_reveal_item(self.messages.len() - 1);
                    }
                }
            }
            Update::Status(s) => self.status = s,
            Update::Fatal(e) => self.fatal = Some(e),
        }
        cx.notify();
    }

    fn open(&mut self, peer: PeerKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected == Some(peer) {
            return;
        }
        self.selected = Some(peer);
        self.messages.clear();
        self.messages_state.reset(0);
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
                                    .child(div().w_full().child(one_line(item.name.clone()))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
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
                                    .child(div().w_full().child(one_line(item.last.clone()))),
                            )
                            .children(badge)
                            .when(item.unread == 0 && item.pinned, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(12.))
                                        .text_color(c(date_fg))
                                        .child("📌"),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_message(&self, item: &MessageItem) -> AnyElement {
        let p = self.palette();
        let (bg, fg, date_fg) = if item.out {
            (p.msg_out_bg, p.history_text_out_fg, p.msg_out_date_fg)
        } else {
            (p.msg_in_bg, p.history_text_in_fg, p.msg_in_date_fg)
        };
        let bubble = div()
            .max_w(px(MSG_MAX_WIDTH))
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
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_color(p, *color)))
                    .child(name.clone())
            }))
            .child(div().child(item.text.clone()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .text_size(px(12.))
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(
                        div()
                            .text_color(c(p.window_sub_text_fg))
                            .child(if chat.is_channel { "channel" } else { "" }),
                    );
                let view = cx.entity();
                let messages = list(self.messages_state.clone(), move |i, _window, cx| {
                    let view = view.read(cx);
                    match view.messages.get(i) {
                        Some(m) => view.render_message(m),
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
            .font_family(FONT_FAMILY)
            .text_size(px(FONT_SIZE))
            .bg(c(p.window_bg))
            .text_color(c(p.window_fg))
            .track_focus(&self.focus)
            .child(left)
            .child(right)
    }
}
