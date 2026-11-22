//! Threads and forums: a post's comments and a forum topic shown as a thread of
//! the chat (tdesktop's `HistoryView::ChatWidget` with `Section::Replies`), and a
//! forum's topics list beside the chat list narrowed to userpics (the dialogs
//! widget's child list, `Dialogs::InnerWidget` of a forum).

use std::time::Instant;

use gpui::{
    AnyElement, Context, Div, Focusable, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use tdr_data::tdr_storage::PeerKey;

use super::{
    DIALOG_DATE_SKIP, FONT_SIZE, MainView, Slide, UNREAD_FONT_SIZE, UNREAD_HEIGHT, UNREAD_PADDING,
    c, text_width,
};
use crate::anim::ease_out_circ;
use crate::bridge::{Command, TopicItem};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::scale::{self, lg, px};

/// `forumTopicRow`: height, padding, the icon's size and place, the name and the
/// text.
const TOPIC_ROW_HEIGHT: f32 = 54.;
const TOPIC_PADDING_RIGHT: f32 = 10.;
const TOPIC_ICON: f32 = 21.;
const TOPIC_ICON_POSITION: (f32, f32) = (10., 7.);
const TOPIC_NAME_LEFT: f32 = 39.;
const TOPIC_NAME_TOP: f32 = 7.;
const TOPIC_TEXT_TOP: f32 = 29.;
/// The chat list beside an open forum: userpics only (`defaultDialogRow`'s photo
/// and paddings).
pub(super) const FORUM_STRIP_WIDTH: f32 = 66.;
/// `historyCommentsUserpics`: size, how much the next one overlaps, the ring.
pub(super) const COMMENTS_USERPIC: f32 = 25.;
pub(super) const COMMENTS_USERPIC_SHIFT: f32 = 6.;
pub(super) const COMMENTS_USERPIC_STROKE: f32 = 2.;

/// What a thread of the open chat is.
#[derive(Clone)]
pub(super) enum ThreadKind {
    /// A post's comments: "Discussion", "12 comments".
    Comments,
    /// A forum topic: its title, "5 messages".
    Topic { title: String },
}

/// A thread shown instead of the chat's own history.
#[derive(Clone)]
pub(super) struct ThreadView {
    /// The root in the chat (`None` until the comments' root is known).
    pub root: Option<i32>,
    pub kind: ThreadKind,
    /// Messages in it (the server's count).
    pub count: Option<i32>,
}

/// An open forum: its topics beside the narrowed chat list, sliding in.
pub(super) struct ForumView {
    pub peer: PeerKey,
    pub topics: Option<Vec<TopicItem>>,
    pub start: Instant,
    /// The open topic (highlighted).
    pub active: Option<i32>,
}

impl MainView {
    /// The open thread's header: its title and status line (`TopBarWidget::paint`
    /// for `Section::Replies`).
    pub(super) fn thread_header(&self) -> Option<(String, String)> {
        let thread = self.thread.as_ref()?;
        Some(match &thread.kind {
            ThreadKind::Comments => (
                "Discussion".into(),
                match thread.count {
                    Some(n) if n > 0 => crate::present::count_text(n, "comment", "comments"),
                    _ => "Comments".into(),
                },
            ),
            ThreadKind::Topic { title } => (
                title.clone(),
                match thread.count {
                    // `lng_forum_messages`: the topic's first message is not counted.
                    Some(n) if n > 1 => crate::present::count_text(n - 1, "message", "messages"),
                    Some(_) => "No messages".into(),
                    None => "Loading...".into(),
                },
            ),
        })
    }

    /// The comments button: the discussion slides in over the channel
    /// (`showRepliesForMessage`), Back returns.
    pub(super) fn open_comments(
        &mut self,
        channel: PeerKey,
        post: i32,
        discussion: Option<PeerKey>,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = discussion else {
            return;
        };
        self.save_draft(cx);
        if let Some(pane) = self.take_current_pane(cx) {
            self.back_stack.push(pane.clone());
            self.slide = Some(Slide {
                pane,
                forward: true,
                start: Instant::now(),
            });
        }
        self.reset_for_thread(group);
        self.thread = Some(ThreadView {
            root: None,
            kind: ThreadKind::Comments,
            count: None,
        });
        let _ = self.commands.send(Command::OpenComments { channel, post });
        cx.notify();
    }

    /// A topic of the open forum: its messages in the chat area (`showTopic`).
    pub(super) fn open_topic(
        &mut self,
        peer: PeerKey,
        topic: &TopicItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for pane in std::mem::take(&mut self.back_stack) {
            self.remember(pane);
        }
        self.slide = None;
        self.save_draft(cx);
        self.leave_current(cx);
        self.reset_for_thread(peer);
        self.thread = Some(ThreadView {
            root: Some(topic.id),
            kind: ThreadKind::Topic {
                title: topic.title.clone(),
            },
            count: None,
        });
        if let Some(forum) = &mut self.forum {
            forum.active = Some(topic.id);
        }
        let _ = self.commands.send(Command::OpenThread {
            peer,
            root: topic.id,
        });
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    /// The chat area for a thread of `peer`: empty until its messages come.
    fn reset_for_thread(&mut self, peer: PeerKey) {
        self.selected = Some(peer);
        self.messages.clear();
        self.messages_state.reset(0);
        self.history_complete = false;
        self.loading_older = false;
        self.loading_newer = false;
        self.newer_available = false;
        self.stick_to_bottom = true;
        self.unread_target = None;
        self.unread_bar = None;
        self.pending_focus = None;
        self.highlight = None;
    }

    /// A forum in the chat list: its topics open beside it (the chat list
    /// narrows to userpics), the chat area stays.
    pub(super) fn open_forum(&mut self, peer: PeerKey, cx: &mut Context<Self>) {
        if self.forum.as_ref().is_some_and(|f| f.peer == peer) {
            return;
        }
        self.forum = Some(ForumView {
            peer,
            topics: None,
            start: Instant::now(),
            active: None,
        });
        let _ = self.commands.send(Command::OpenForum(peer));
        cx.notify();
    }

    pub(super) fn close_forum(&mut self, cx: &mut Context<Self>) {
        self.forum = None;
        cx.notify();
    }

    /// The narrowed chat list's top: the back arrow closing the forum, with the
    /// unread count of the other chats.
    pub(super) fn render_forum_strip_header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let unread = self
            .accounts
            .read(cx)
            .active()
            .map(|s| s.state)
            .filter(|s| s.unread > 0)
            .map(|s| (s.unread, s.unread_muted));
        div()
            .flex_none()
            .relative()
            .h(px(super::TOP_BAR_HEIGHT))
            .w_full()
            .bg(c(p.dialogs_bg))
            .child(
                div()
                    .id("forum-strip-back")
                    .relative()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon(Icon::Back, p.box_title_close_fg, window))
                    .children(unread.map(|(count, muted)| {
                        div()
                            .absolute()
                            .right(px(4.))
                            .top(px(super::BACK_UNREAD_TOP))
                            .child(self.count_badge(
                                &super::main_menu::short_count(count),
                                muted,
                                window,
                            ))
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.close_forum(cx))),
            )
    }

    /// The forum's column: its top bar (back, title, members) and its topics,
    /// sliding in from the right (`_childListShown`).
    pub(super) fn render_topics_column(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let Some(forum) = &self.forum else {
            return div().into_any_element();
        };
        let peer = forum.peer;
        let t = (forum.start.elapsed().as_secs_f32() / super::SLIDE_DURATION.as_secs_f32()).min(1.);
        if t < 1. {
            window.request_animation_frame();
        }
        let shift = width * (1. - ease_out_circ(t));
        let chat = self.chat_item(peer).cloned();
        let title = chat.as_ref().map(|c| c.name.clone()).unwrap_or_default();
        let status = self
            .members
            .get(&peer)
            .cloned()
            .or_else(|| chat.as_ref().map(|c| c.status.clone()))
            .unwrap_or_default();
        let active = forum.active.filter(|_| self.selected == Some(peer));
        let rows: Vec<AnyElement> = match &forum.topics {
            Some(topics) => topics
                .iter()
                .map(|t| self.render_topic_row(peer, t, active == Some(t.id), window, cx))
                .collect(),
            None => Vec::new(),
        };
        let bar = div()
            .flex_none()
            .relative()
            .h(px(super::TOP_BAR_HEIGHT))
            .w_full()
            .flex()
            .bg(c(p.window_bg))
            .child(
                div()
                    .id("forum-back")
                    .flex_none()
                    .w(px(super::BACK_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(px(super::BACK_ICON_LEFT))
                    .cursor_pointer()
                    .child(icon(Icon::Back, p.box_title_close_fg, window))
                    .on_click(cx.listener(|this, _, _, cx| this.close_forum(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .pr(px(16.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(p.dialogs_name_fg))
                            .child(one_line(title)),
                    )
                    .child(
                        div()
                            .text_color(c(p.window_sub_text_fg))
                            .child(one_line(status)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(gpui::px(lg(1.)))
                    .bg(c(p.shadow_fg)),
            );
        div()
            .relative()
            .h_full()
            .w(gpui::px(width))
            .flex_none()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::px(shift))
                    .w(gpui::px(width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(c(p.dialogs_bg))
                    .border_l_1()
                    .border_color(c(p.shadow_fg))
                    .child(bar)
                    .child(
                        div()
                            .id("forum-topics")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .into_any_element()
    }

    /// A topic's row (`Dialogs::Ui::PaintRow` with `forumTopicRow`): its icon, its
    /// title and time, the last message's sender and text, the unread count.
    fn render_topic_row(
        &self,
        peer: PeerKey,
        topic: &TopicItem,
        active: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let (bg, name_fg, text_fg, date_fg, service_fg) = if active {
            (
                p.dialogs_bg_active,
                p.dialogs_name_fg_active,
                p.dialogs_text_fg_active,
                p.dialogs_date_fg_active,
                p.dialogs_text_fg_service_active,
            )
        } else {
            (
                p.dialogs_bg,
                p.dialogs_name_fg,
                p.dialogs_text_fg,
                p.dialogs_date_fg,
                p.dialogs_text_fg_service,
            )
        };
        let text_font = scale::font(FONT_SIZE);
        let date_w = text_width(
            window,
            &topic.time,
            super::DATE_FONT_SIZE,
            FontWeight::NORMAL,
        )
        .ceil();
        let badge_text = topic.unread.to_string();
        let badge_w = (text_width(window, &badge_text, UNREAD_FONT_SIZE, FontWeight::BOLD).ceil()
            + 2. * lg(UNREAD_PADDING))
        .max(lg(UNREAD_HEIGHT))
        .round();
        let badge = (topic.unread > 0).then(|| self.count_badge(&badge_text, topic.muted, window));
        let text_right = lg(TOPIC_PADDING_RIGHT)
            + if badge.is_some() {
                badge_w + lg(UNREAD_PADDING)
            } else if topic.pinned {
                crate::icons::icon_width(Icon::DialogsPinned, window) + lg(UNREAD_PADDING)
            } else {
                0.
            };
        let last = &topic.last;
        let last_line = div()
            .flex()
            .children(last.prefix.as_ref().map(|name| {
                div()
                    .flex_none()
                    .text_color(c(service_fg))
                    .child(format!("{name}:\u{a0}"))
            }))
            .child(match &last.accent {
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
            });
        let item = topic.clone();
        div()
            .id(SharedString::from(format!("topic-{}", topic.id)))
            .relative()
            .h(px(TOPIC_ROW_HEIGHT))
            .w_full()
            .bg(c(bg))
            .when(!active, |d| d.hover(|s| s.bg(c(p.dialogs_bg_over))))
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .left(px(TOPIC_ICON_POSITION.0))
                    .top(px(TOPIC_ICON_POSITION.1))
                    .child(crate::icons::topic_icon(
                        topic.icon_color,
                        topic.general,
                        &topic.title,
                        TOPIC_ICON,
                        if active {
                            p.dialogs_text_fg_active
                        } else {
                            p.dialogs_text_fg
                        },
                        window,
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(px(TOPIC_NAME_LEFT))
                    .right(gpui::px(
                        lg(TOPIC_PADDING_RIGHT) + date_w + lg(DIALOG_DATE_SKIP),
                    ))
                    .top(px(TOPIC_NAME_TOP))
                    .h(text_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(name_fg))
                    .child(one_line(topic.title.clone())),
            )
            .child(
                div()
                    .absolute()
                    .right(px(TOPIC_PADDING_RIGHT))
                    .top(px(TOPIC_NAME_TOP))
                    .text_size(scale::font(super::DATE_FONT_SIZE).size)
                    .text_color(c(date_fg))
                    .child(topic.time.clone()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(TOPIC_NAME_LEFT))
                    .right(gpui::px(text_right))
                    .top(px(TOPIC_TEXT_TOP))
                    .h(text_font.line_height())
                    .child(last_line),
            )
            .children(badge.map(|b| {
                div()
                    .absolute()
                    .right(px(TOPIC_PADDING_RIGHT))
                    .top(px(TOPIC_TEXT_TOP))
                    .child(b)
            }))
            .when(topic.pinned && topic.unread == 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(TOPIC_PADDING_RIGHT))
                        .top(px(TOPIC_TEXT_TOP))
                        .child(icon(Icon::DialogsPinned, p.dialogs_unread_bg_muted, window)),
                )
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_topic(peer, &item, window, cx)),
            )
            .into_any_element()
    }

    /// The commenters' userpics on the comments button (`GenerateUserpicsInRow`):
    /// each over the previous one, ringed in the bubble's color.
    pub(super) fn render_commenters(
        &self,
        authors: &[crate::bridge::Author],
        ring: tdr_theme::Color,
    ) -> Div {
        let size = COMMENTS_USERPIC;
        let mut row = div().relative().flex_none().h(px(size)).w(gpui::px(
            lg(size)
                + (authors.len().saturating_sub(1)) as f32
                    * (lg(size) - lg(COMMENTS_USERPIC_SHIFT)),
        ));
        for (i, a) in authors.iter().enumerate() {
            let left = i as f32 * (lg(size) - lg(COMMENTS_USERPIC_SHIFT));
            let stroke = lg(COMMENTS_USERPIC_STROKE);
            row = row.child(
                div()
                    .absolute()
                    .top(gpui::px(-stroke))
                    .left(gpui::px(left - stroke))
                    .size(gpui::px(lg(size) + 2. * stroke))
                    .rounded_full()
                    .bg(c(ring))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.userpic(size, a.key, a.photo, &a.initials, a.color, false)),
            );
        }
        row
    }
}

/// The width of the commenters' userpics row.
pub(super) fn commenters_width(count: usize) -> f32 {
    if count == 0 {
        return 0.;
    }
    lg(COMMENTS_USERPIC) + (count - 1) as f32 * (lg(COMMENTS_USERPIC) - lg(COMMENTS_USERPIC_SHIFT))
}
