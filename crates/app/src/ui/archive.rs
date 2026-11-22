//! The archive (`Data::Folder` with `kId`): its row over the chat list
//! (`Dialogs::Ui::PaintRow` with a folder), its menu (`Filler::fillArchiveActions`),
//! its place in the main menu (`MainMenu::setupArchive`, the account's
//! `archiveInMainMenu`) and its own list under a top bar, slid in and out like
//! tdesktop's `Dialogs::Widget::changeOpenedFolder` (`Window::SlideAnimation`).

use std::time::Instant;

use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, StatefulInteractiveElement, Styled, Window, div,
};

use super::{
    Area, ColumnSlide, DIALOG_NAME_LEFT, DIALOG_NAME_TOP, DIALOG_PADDING_X, DIALOG_PADDING_Y,
    DIALOG_PHOTO, DIALOG_ROW_HEIGHT, DIALOG_TEXT_TOP, FONT_SIZE, MainView, UNREAD_FONT_SIZE,
    UNREAD_HEIGHT, UNREAD_PADDING, c, text_width,
};
use crate::bridge::{ArchiveInfo, Command, DialogItem};
use crate::icons::{Icon, icon};
use crate::one_line::one_line;
use crate::popup_menu::{Entry, MenuAction, PopupMenu};
use crate::scale::{self, lg, px};

/// `historySavedFont`: the top bar's "Archived chats".
const TITLE_FONT: f32 = 14.;
/// The archive's menu "Move to main menu" toast (`kArchivedToastDuration`).
const TO_MENU_TOAST_MS: f32 = 5000.;

/// The chat list sliding while the archive opens or closes: what was shown before,
/// drawn as it was (tdesktop slides a grab of it).
pub(super) struct ArchiveSlide {
    pub start: Instant,
    /// The archive comes in from the right (`FromRight`), or leaves.
    pub opening: bool,
    pub old: Vec<DialogItem>,
    pub old_offset: f32,
    /// The archive's row was over the old list.
    pub old_row: bool,
}

/// Where the archive's menu was opened from: `Section::ContextMenu` in the chat
/// list or the main menu, `Section::ChatsList` from the archive's own top bar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchiveMenu {
    Row,
    MainMenu,
    TopBar,
}

/// The archive row's second line (`ComposeFolderListEntryText`): the names, unread
/// chats' names marked, "… and N more chats".
fn entry_text(info: &ArchiveInfo) -> (String, Vec<std::ops::Range<usize>>) {
    let mut text = String::new();
    let mut marked = Vec::new();
    for (i, (name, unread)) in info.names.iter().enumerate() {
        if i > 0 {
            text.push_str(", ");
        }
        let start = text.len();
        text.push_str(name);
        if *unread {
            marked.push(start..text.len());
        }
    }
    if info.more > 0 && !info.names.is_empty() {
        let chats = if info.more == 1 { "chat" } else { "chats" };
        text = format!("{text} and {} more {chats}", info.more);
    }
    (text, marked)
}

impl MainView {
    /// The archive's row is over "All chats" while it has chats and is not in the
    /// main menu (`fixedOnTopIndex`).
    pub(super) fn archive_row_shown(&self) -> bool {
        !self.archive_open
            && self.active_folder == 0
            && self.archive.as_ref().is_some_and(|a| !a.in_menu)
    }

    /// Rows over the chats in the list (the archive's).
    pub(super) fn list_skip(&self) -> usize {
        usize::from(self.archive_row_shown())
    }

    /// The list the window shows: the open folder's or the archive's.
    pub(super) fn shown_list_id(&self) -> i32 {
        if self.archive_open {
            crate::bridge::ARCHIVE_LIST
        } else {
            self.active_folder
        }
    }

    /// The archive's row: its userpic, "Archived chats", the latest chats' names and
    /// a grey badge with the unread chats. A click opens it, the right button its
    /// menu.
    pub(super) fn render_archive_row(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = &self.archive else {
            return div().h(px(DIALOG_ROW_HEIGHT)).into_any_element();
        };
        let p = self.palette();
        let narrow = self.narrow(window);
        let text_font = scale::font(FONT_SIZE);
        let unread_font = scale::font(UNREAD_FONT_SIZE);
        let badge_text = info.unread.to_string();
        let badge_width = (text_width(window, &badge_text, UNREAD_FONT_SIZE, FontWeight::BOLD)
            .ceil()
            + 2. * lg(UNREAD_PADDING))
        .max(lg(UNREAD_HEIGHT))
        .round();
        let badge = (info.unread > 0).then(|| self.count_badge(&badge_text, true, window));
        // `PaintWideCounter`: the badge's top on the text line's baseline.
        let badge_top = {
            let top = scale::device(DIALOG_TEXT_TOP) + text_font.ascent * scale::get()
                - unread_font.ascent * scale::get()
                - ((scale::device(UNREAD_HEIGHT) - unread_font.height * scale::get()) / 2.).floor();
            gpui::px(top.round() / scale::get())
        };
        let userpic = div()
            .absolute()
            .left(px(DIALOG_PADDING_X))
            .top(px(DIALOG_PADDING_Y))
            .size(px(DIALOG_PHOTO))
            .rounded_full()
            .bg(c(p.history_peer_archive_userpic_bg))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(
                Icon::ArchiveUserpic,
                p.history_peer_userpic_fg,
                window,
            ));
        let mut row = div()
            .id("archive-row")
            .relative()
            .h(px(DIALOG_ROW_HEIGHT))
            .w_full()
            .bg(c(p.dialogs_bg))
            .hover(|s| s.bg(c(p.dialogs_bg_over)))
            .cursor_pointer()
            .child(userpic)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                    this.open_archive_menu(e.position, ArchiveMenu::Row);
                    cx.notify();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| this.open_archive(cx)));
        if narrow {
            // Userpics only: the counter on the userpic's corner.
            return row
                .children(badge.map(|b| {
                    div()
                        .absolute()
                        .top(px(DIALOG_PADDING_Y + DIALOG_PHOTO - UNREAD_HEIGHT))
                        .left(gpui::px(
                            lg(DIALOG_PADDING_X) + lg(DIALOG_PHOTO) - badge_width,
                        ))
                        .child(b)
                }))
                .into_any_element();
        }
        let (text, marked) = entry_text(info);
        let text_right = lg(DIALOG_PADDING_X)
            + if badge.is_some() {
                badge_width + lg(UNREAD_PADDING)
            } else {
                0.
            };
        row = row
            .child(
                div()
                    .absolute()
                    .left(px(DIALOG_NAME_LEFT))
                    .right(px(DIALOG_PADDING_X))
                    .top(px(DIALOG_NAME_TOP))
                    .h(text_font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.dialogs_name_fg))
                    .child(one_line("Archived chats")),
            )
            .child(
                div()
                    .absolute()
                    .left(px(DIALOG_NAME_LEFT))
                    .right(gpui::px(text_right))
                    .top(px(DIALOG_TEXT_TOP))
                    .h(text_font.line_height())
                    .text_color(c(p.dialogs_text_fg))
                    .child(one_line(text).marked(marked, p.dialogs_archive_fg)),
            )
            .children(badge.map(|b| {
                div()
                    .absolute()
                    .right(px(DIALOG_PADDING_X))
                    .top(badge_top)
                    .child(b)
            }));
        row.into_any_element()
    }

    /// The top bar of the open archive (`HistoryView::TopBarWidget` with the
    /// folder): the back arrow, "Archived chats", the "⋮" menu.
    pub(super) fn render_archive_bar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette();
        let font = scale::font(TITLE_FONT);
        let colors = (p.menu_icon_fg, p.menu_icon_fg_over);
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
            .flex()
            .bg(c(p.window_bg))
            .child(
                div()
                    .id("archive-back")
                    .flex_none()
                    .w(px(super::BACK_WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .relative()
                    .pl(px(super::BACK_ICON_LEFT))
                    .cursor_pointer()
                    .child(icon(Icon::Back, p.box_title_close_fg, window))
                    // `TopBarWidget::updateUnreadBadge`: the unread messages outside
                    // the archive (`unreadBadgeIgnoreOne`).
                    .children(unread.map(|(count, muted)| {
                        div()
                            .absolute()
                            .right_0()
                            .top(px(super::BACK_UNREAD_TOP))
                            .child(self.count_badge(
                                &super::main_menu::short_count(count),
                                muted,
                                window,
                            ))
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.close_archive(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(font.size)
                    .line_height(font.line_height())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(p.dialogs_name_fg))
                    .child(one_line("Archived chats")),
            )
            .child(self.icon_button(
                "archive-menu",
                Icon::MenuDots,
                (super::chrome::TOP_BAR_MENU, super::TOP_BAR_HEIGHT),
                Some(super::chrome::TOP_BAR_MENU_ICON),
                (
                    (0., super::chrome::TOP_BAR_RIPPLE_TOP),
                    super::chrome::TOP_BAR_BUTTON,
                ),
                colors,
                window,
                cx,
                |this, window, _| {
                    // Its top right at the button's bottom right.
                    let x = this.dialogs_right(window);
                    let at = gpui::point(
                        gpui::px(x),
                        gpui::px(lg(super::TITLE_HEIGHT) + lg(super::TOP_BAR_HEIGHT) - lg(8.)),
                    );
                    this.open_archive_menu(at, ArchiveMenu::TopBar);
                },
            ))
            // `PlainShadow` under it.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(gpui::px(lg(1.)))
                    .bg(c(p.shadow_fg)),
            )
    }

    /// The right edge of the chat list in the window.
    fn dialogs_right(&self, window: &Window) -> f32 {
        self.filters_width() + self.dialogs_width(window)
    }

    /// The archive's menu at `at`.
    pub(super) fn open_archive_menu(&mut self, at: Point<Pixels>, from: ArchiveMenu) {
        let Some(info) = &self.archive else {
            return;
        };
        let in_menu = info.in_menu;
        let mut entries = Vec::new();
        if from == ArchiveMenu::TopBar {
            entries.push(Entry::action(
                "Search",
                Icon::MenuSearch,
                MenuAction::ArchiveStub,
            ));
        } else {
            entries.push(Entry::action(
                "Open in new window",
                Icon::MenuNewWindow,
                MenuAction::ArchiveStub,
            ));
            if !in_menu {
                entries.push(Entry::action(
                    "Collapse",
                    Icon::MenuCollapse,
                    MenuAction::ArchiveStub,
                ));
            }
            entries.push(if in_menu {
                Entry::action(
                    "Move to chat list",
                    Icon::MenuFromMainMenu,
                    MenuAction::ArchiveInMenu(false),
                )
            } else {
                Entry::action(
                    "Move to main menu",
                    Icon::MenuToMainMenu,
                    MenuAction::ArchiveInMenu(true),
                )
            });
            // `MarkAsReadMenu::AddChatListAction`: while something is unread.
            if info.unread > 0 {
                entries.push(Entry::action(
                    "Mark as read",
                    Icon::MenuMarkRead,
                    MenuAction::ArchiveStub,
                ));
            }
        }
        entries.push(Entry::WideSeparator);
        entries.push(Entry::action(
            "Archive settings",
            Icon::MenuManage,
            MenuAction::ArchiveStub,
        ));
        entries.push(Entry::action(
            "How does it work?",
            Icon::MenuFaq,
            MenuAction::ArchiveStub,
        ));
        let menu = PopupMenu::new(entries, true, at);
        let menu = if from == ArchiveMenu::TopBar {
            menu.top_right_origin()
        } else {
            menu
        };
        self.open_popup(menu);
    }

    /// "Move to main menu" / "Move to chat list": the account's setting, the main
    /// menu and layers hidden (`hideSettingsAndLayer`), a toast when it moves there.
    pub(super) fn set_archive_in_menu(&mut self, in_menu: bool) {
        if let Some(info) = &mut self.archive {
            info.in_menu = in_menu;
        }
        let _ = self.commands.send(Command::ArchiveInMenu(in_menu));
        if in_menu {
            self.show_long_toast(
                "Archive moved to the main menu!\nRight click the archive button to return the Archive to your chat list.",
                TO_MENU_TOAST_MS,
            );
        }
        self.layers.hide_all();
    }

    /// Opens the archive: its list slides in from the right over the chats.
    pub(super) fn open_archive(&mut self, cx: &mut Context<Self>) {
        if self.archive_open {
            return;
        }
        let (_, _, offset) = self.scroll_metrics(Area::Dialogs);
        self.archive_slide = Some(ArchiveSlide {
            start: Instant::now(),
            opening: true,
            old: self.dialogs.clone(),
            old_offset: offset,
            old_row: self.archive_row_shown(),
        });
        self.main_list_offset = offset;
        // The open chat stays open (`changeOpenedFolder` leaves the chat alone): its
        // header data is kept aside while the archive's list is shown.
        if let Some(peer) = self.selected
            && let Some(item) = self.dialogs.iter().find(|d| d.peer == peer)
        {
            self.extra_chats.insert(peer, item.clone());
        }
        self.archive_open = true;
        self.dialogs = self
            .folder_lists
            .get(&crate::bridge::ARCHIVE_LIST)
            .cloned()
            .unwrap_or_default();
        self.more_dialogs_requested = 0;
        self.set_scroll(Area::Dialogs, 0.);
        self.layers.hide_all();
        let _ = self.commands.send(Command::OpenArchive);
        cx.notify();
    }

    /// Back to the chats: the archive slides out to the right.
    pub(super) fn close_archive(&mut self, cx: &mut Context<Self>) {
        if !self.archive_open {
            return;
        }
        let (_, _, offset) = self.scroll_metrics(Area::Dialogs);
        self.archive_slide = Some(ArchiveSlide {
            start: Instant::now(),
            opening: false,
            old: self.dialogs.clone(),
            old_offset: offset,
            old_row: false,
        });
        // An archived chat open stays open: its header data is kept aside.
        if let Some(peer) = self.selected
            && let Some(item) = self.dialogs.iter().find(|d| d.peer == peer)
        {
            self.extra_chats.insert(peer, item.clone());
        }
        self.archive_open = false;
        self.dialogs = self
            .folder_lists
            .get(&self.active_folder)
            .cloned()
            .unwrap_or_default();
        self.more_dialogs_requested = 0;
        let back = self.main_list_offset;
        self.set_scroll(Area::Dialogs, back);
        let _ = self.commands.send(Command::CloseArchive);
        cx.notify();
    }

    /// The list that was shown, as it was: its top bar and the rows in view.
    fn render_list_snapshot(
        &self,
        slide: &ArchiveSlide,
        dialogs_width: f32,
        narrow: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette();
        let header = if slide.opening {
            self.render_dialogs_header(dialogs_width, narrow, window, cx)
        } else {
            self.render_archive_bar(window, cx)
        };
        let height = lg(DIALOG_ROW_HEIGHT);
        let skip = usize::from(slide.old_row);
        let first = (slide.old_offset / height).floor().max(0.) as usize;
        let rows_in_view = (f32::from(window.viewport_size().height) / height).ceil() as usize + 1;
        let mut rows = div().relative().flex_1().min_h(px(0.)).overflow_hidden();
        for i in first..first + rows_in_view {
            let top = i as f32 * height - slide.old_offset;
            let row = if i < skip {
                self.render_archive_row(window, cx)
            } else if let Some(item) = slide.old.get(i - skip) {
                self.render_dialog(item, true, window, cx)
            } else {
                break;
            };
            rows = rows.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(gpui::px(top))
                    .h(gpui::px(height))
                    .child(row),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.dialogs_bg))
            .child(header)
            .child(rows)
    }

    /// The chat list column's content, sliding while the archive opens or closes.
    pub(super) fn archive_slide_view(
        &mut self,
        live: Div,
        dialogs_width: f32,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let done = self
            .archive_slide
            .as_ref()
            .is_none_or(|s| s.start.elapsed() >= super::SLIDE_DURATION);
        if done {
            self.archive_slide = None;
            return live.into_any_element();
        }
        let Some(slide) = self.archive_slide.take() else {
            return live.into_any_element();
        };
        let snapshot = self.render_list_snapshot(&slide, dialogs_width, narrow, window, cx);
        let (under, over) = if slide.opening {
            (snapshot, live)
        } else {
            (live, snapshot)
        };
        let view = self.column_slide_view(
            under,
            over,
            ColumnSlide {
                start: slide.start,
                to_chat: slide.opening,
            },
            dialogs_width,
            window,
        );
        self.archive_slide = Some(slide);
        view.into_any_element()
    }

    /// The main menu's archive row was clicked: the archive opens, the menu closes.
    pub(super) fn archive_from_main_menu(&mut self, cx: &mut Context<Self>) {
        self.layers.hide_all();
        if self.archive_open {
            return;
        }
        self.open_archive(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_text_like_tdesktop() {
        let info = ArchiveInfo {
            in_menu: false,
            names: vec![("Ann".into(), false), ("Bob".into(), true)],
            more: 2,
            unread: 1,
        };
        let (text, marked) = entry_text(&info);
        assert_eq!(text, "Ann, Bob and 2 more chats");
        assert_eq!(marked, vec![5..8]);
    }
}
