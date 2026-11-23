//! An emoji set's box (`StickerSetBox` for emoji): the set's title (its mentions
//! as links), the "⋮" menu (Share Emoji, Copy Link), the close button, its emoji
//! eight per row, and the bottom button: "Add Emoji N", "Remove Emoji N" (asks
//! first, `MakeConfirmRemoveSetBox`), "Unlock Animated Emoji" for a Premium set
//! without Premium. Opened from a set's title in the emoji panel, by
//! `t.me/addemoji/<name>` links and from a message's menu.

use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, prelude::FluentBuilder,
};
use tdr_data::emoji_sets::SetRef;

use super::layers::{BOX_RADIUS, LayerBox, TITLE_FONT, box_shadow};
use super::{MainView, c};
use crate::bridge::{Command, EmojiSet};
use crate::icons::{Icon, icon};
use crate::popup_menu::{Entry, MenuAction, PopupMenu};
use crate::scale::{self, lg, px};

// layers.style boxWideWidth; chat_helpers.style emojiSetPadding, emojiSetSize,
// emojiSetMaxHeight; `kEmojiPerRow`.
const BOX_WIDE_WIDTH: f32 = 364.;
const PADDING: f32 = 12.;
const CELL: (f32, f32) = (42., 39.);
const MAX_HEIGHT: f32 = 197.;
const PER_ROW: usize = 8;
const TITLE_HEIGHT: f32 = 56.;
const TITLE_LEFT: f32 = 24.;
const BUTTON_HEIGHT: f32 = 40.;
const BUTTON_PADDING: f32 = 22.;

/// The open set box.
pub(super) struct SetBox {
    pub set: SetRef,
    /// `None` while loading, `Some(None)` when not found.
    pub loaded: Option<Option<EmojiSet>>,
    /// The title's link under the mouse (its word's index).
    pub link_over: Option<usize>,
    /// The emoji under the mouse.
    pub over: Option<usize>,
}

/// A link in a set's title: a mention (`TextParseMentions`) or an address, with
/// the punctuation after it left out. The word's link part and where it goes.
fn title_link(word: &str) -> Option<(usize, String)> {
    if let Some(name) = word.strip_prefix('@') {
        let len = name
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(name.len());
        return (len >= 4).then(|| (len + 1, format!("https://t.me/{}", &name[..len])));
    }
    let lower = word.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("t.me/") {
        let len = word
            .trim_end_matches(['.', ',', ')', '!', '?', ';', ':'])
            .len();
        return Some((len, word[..len].to_string()));
    }
    None
}

impl MainView {
    pub(super) fn open_emoji_set_box(&mut self, set: SetRef) {
        let _ = self.commands.send(Command::LoadEmojiSet(set.clone()));
        self.emoji_set_box = Some(SetBox {
            set,
            loaded: None,
            link_over: None,
            over: None,
        });
        self.layers.show_layer(LayerBox::EmojiSet);
    }

    pub(super) fn emoji_set_loaded(&mut self, set: SetRef, found: Option<EmojiSet>) {
        if let Some(b) = &mut self.emoji_set_box
            && (b.set == set
                || found.as_ref().is_some_and(|f| match &b.set {
                    SetRef::ShortName(n) => n.eq_ignore_ascii_case(&f.info.short_name),
                    SetRef::Id(id, _) => *id == f.info.id,
                }))
        {
            if let Some(f) = &found {
                for doc in &f.emoji {
                    crate::custom_emoji::remember(doc);
                }
            }
            b.loaded = Some(found);
        }
    }

    fn installed_set(&self, id: i64) -> bool {
        self.emoji_panel.has_installed(id)
    }

    /// `MakeConfirmRemoveSetBox`.
    pub(super) fn render_remove_emoji_set(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((set, title)) = self.emoji_set_removing.clone() else {
            return div().into_any_element();
        };
        let p = self.palette();
        let font = scale::font(14.);
        let body = div()
            .pt(px(10. + 14.))
            .pl(px(24.))
            .pr(px(24.))
            .pb(px(8.))
            .text_size(font.size)
            .line_height(px(22.))
            .text_color(c(p.box_text_fg))
            .child(format!("Remove \"{title}\"?"));
        let confirm = self.box_button(
            "emoji-set-remove-confirm",
            "Remove",
            true,
            window,
            cx,
            move |this, _, _| {
                this.layers.hide_all();
                let _ = this.commands.send(Command::ToggleEmojiSet {
                    set: set.clone(),
                    install: false,
                });
            },
        );
        let cancel = self.box_button(
            "emoji-set-remove-cancel",
            "Cancel",
            false,
            window,
            cx,
            |this, _, _| this.layers.hide_layer(),
        );
        self.box_frame(320., body, vec![confirm, cancel])
            .into_any_element()
    }

    fn emoji_set_menu(&mut self, at: gpui::Point<gpui::Pixels>) {
        let Some(Some(set)) = self.emoji_set_box.as_ref().and_then(|b| b.loaded.clone()) else {
            return;
        };
        let link = format!("https://t.me/addemoji/{}", set.info.short_name);
        let entries = vec![
            Entry::action("Share Emoji", Icon::MenuForward, MenuAction::None),
            Entry::action(
                "Copy Link",
                Icon::MenuCopy,
                MenuAction::Copy(link, Some("Emoji pack link copied to clipboard.")),
            ),
        ];
        self.open_popup(PopupMenu::new(entries, true, at).top_right_origin());
    }

    pub(super) fn render_emoji_set_box(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette();
        let state = self.emoji_set_box.as_ref();
        let loaded = state.and_then(|b| b.loaded.clone());
        let title_font = scale::font(TITLE_FONT);
        let (title, set) = match &loaded {
            None => ("Loading...".to_string(), None),
            Some(None) => (
                "Sorry, this emoji set doesn't seem to exist.".to_string(),
                None,
            ),
            Some(Some(set)) => (set.info.title.clone(), Some(set.clone())),
        };
        // The title's mentions and links open (`TextParseMentions`), underlined
        // under the mouse.
        let link_over = self.emoji_set_box.as_ref().and_then(|b| b.link_over);
        let mut title_row = div().flex().flex_wrap().min_w(px(0.));
        for (k, word) in title.split(' ').enumerate() {
            let mut cell = div().flex().when(k > 0, |d| d.ml(gpui::px(lg(4.))));
            match title_link(word) {
                Some((len, url)) => {
                    let over = link_over == Some(k);
                    cell = cell
                        .child(
                            div()
                                .id(("emoji-set-title-link", k))
                                .cursor_pointer()
                                .text_color(c(p.window_active_text_fg))
                                .when(over, |d| d.underline())
                                .child(word[..len].to_string())
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if let Some(b) = &mut this.emoji_set_box {
                                        b.link_over = if *hovered {
                                            Some(k)
                                        } else {
                                            b.link_over.filter(|o| *o != k)
                                        };
                                    }
                                    cx.notify();
                                }))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.layers.hide_layer();
                                    this.open_url(&url, window, cx);
                                    cx.notify();
                                })),
                        )
                        .when(len < word.len(), |d| {
                            d.child(
                                div()
                                    .text_color(c(p.box_title_fg))
                                    .child(word[len..].to_string()),
                            )
                        });
                }
                None => {
                    cell = cell.text_color(c(p.box_title_fg)).child(word.to_string());
                }
            }
            title_row = title_row.child(cell);
        }
        let header = div()
            .flex_none()
            .relative()
            .h(px(TITLE_HEIGHT))
            .flex()
            .items_center()
            .pl(px(TITLE_LEFT))
            .pr(px(2. * 40.))
            .text_size(title_font.size)
            .line_height(title_font.line_height())
            .font_weight(FontWeight::SEMIBOLD)
            .child(title_row)
            .child(
                div()
                    .id("emoji-set-menu")
                    .absolute()
                    .right(px(44.))
                    .top(px(10.))
                    .size(px(36.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.window_bg_over)))
                    .when(set.is_some(), |d| {
                        d.child(icon(Icon::MenuDots, p.box_title_close_fg, window))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.emoji_set_menu(e.position);
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("emoji-set-close")
                    .absolute()
                    .right(px(8.))
                    .top(px(10.))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(icon(Icon::InfoClose, p.box_title_close_fg, window))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layers.hide_layer();
                        cx.notify();
                    })),
            );

        // The emoji.
        let ids: Vec<i64> = set
            .as_ref()
            .map(|s| s.emoji.iter().map(|d| d.id).collect())
            .unwrap_or_default();
        let rows = ids.len().div_ceil(PER_ROW);
        let grid_h = rows as f32 * lg(CELL.1);
        let text_fg = p.window_fg;
        // `PathShiftGradient(windowBgRipple, windowBgOver)` over loading emoji.
        let (shimmer_bg, shimmer_fg): (gpui::Rgba, gpui::Rgba) =
            (c(p.window_bg_ripple), c(p.window_bg_over));
        let side = crate::custom_emoji::size_large();
        let grid = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let clip = window.content_mask().bounds;
                let s = side as f32 / scale::get();
                for (k, id) in ids.iter().enumerate() {
                    let x = lg(PADDING) + (k % PER_ROW) as f32 * lg(CELL.0);
                    let y = (k / PER_ROW) as f32 * lg(CELL.1);
                    let at = point(
                        bounds.origin.x + gpui::px(x + ((lg(CELL.0) - s) / 2.).floor()),
                        bounds.origin.y + gpui::px(y + ((lg(CELL.1) - s) / 2.).floor()),
                    );
                    if at.y > clip.bottom() || at.y + gpui::px(s) < clip.top() {
                        continue;
                    }
                    crate::custom_emoji::paint_ex(
                        *id,
                        at,
                        side,
                        c(text_fg).into(),
                        crate::custom_emoji::Loading::Shimmer {
                            left: f32::from(bounds.origin.x),
                            width: f32::from(bounds.size.width),
                            bg: shimmer_bg,
                            fg: shimmer_fg,
                        },
                        window,
                    );
                }
            },
        )
        .w_full()
        .h(gpui::px(grid_h));
        // Each emoji is chosen into the field (`processChosenSticker`), on the
        // rounded `emojiPanHover` under the mouse; the box stays.
        let over = self.emoji_set_box.as_ref().and_then(|b| b.over);
        let count = set.as_ref().map_or(0, |s| s.emoji.len());
        let mut cells = div().relative().w_full().h(gpui::px(grid_h));
        for k in 0..count {
            let id = set.as_ref().map_or(0, |s| s.emoji[k].id);
            let x = lg(PADDING) + (k % PER_ROW) as f32 * lg(CELL.0);
            let y = (k / PER_ROW) as f32 * lg(CELL.1);
            cells = cells.child(
                div()
                    .id(("emoji-set-cell", k))
                    .absolute()
                    .left(gpui::px(x))
                    .top(gpui::px(y))
                    .w(gpui::px(lg(CELL.0)))
                    .h(gpui::px(lg(CELL.1)))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(over == Some(k), |d| d.bg(c(p.window_bg_over)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let Some(b) = &mut this.emoji_set_box {
                            b.over = if *hovered {
                                Some(k)
                            } else {
                                b.over.filter(|o| *o != k)
                            };
                        }
                        if *hovered {
                            this.preview_hover(id, cx);
                        }
                        cx.notify();
                    }))
                    // Held: the large preview (`showPreview`); a click chooses.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.preview_press(id, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if !this.preview_release(cx) {
                                this.choose_custom(id, window, cx);
                            }
                            cx.notify();
                        }),
                    ),
            );
        }
        let list = div()
            .id("emoji-set-list")
            .flex_none()
            .h(gpui::px(grid_h.clamp(lg(CELL.1), lg(MAX_HEIGHT))))
            .overflow_y_scroll()
            .child(cells.child(div().absolute().left_0().top_0().size_full().child(grid)));

        // The bottom button.
        let button = set.as_ref().map(|set| {
            let installed = self.installed_set(set.info.id);
            let premium_set = set.emoji.iter().any(|d| !d.free);
            let premium = self
                .accounts
                .read(cx)
                .active()
                .and_then(|s| s.me.as_ref())
                .is_some_and(|m| m.premium);
            let count = set.emoji.len();
            let r = SetRef::Id(set.info.id, set.info.access_hash);
            let title = set.info.title.clone();
            let (label, badge, fg, bg) = if !installed && premium_set && !premium {
                (
                    "Unlock Animated Emoji".to_string(),
                    None,
                    p.premium_button_fg,
                    None,
                )
            } else if !installed {
                (
                    "Add emoji".to_string(),
                    Some(count),
                    p.active_button_fg,
                    Some(p.active_button_bg),
                )
            } else {
                (
                    "Remove Emoji".to_string(),
                    Some(count),
                    p.attention_button_fg,
                    None,
                )
            };
            let premium_bg = (p.premium_button_bg1, p.premium_button_bg3);
            let mut b = div()
                .id("emoji-set-button")
                .mx(px(BUTTON_PADDING))
                .mb(px(16.))
                .h(px(BUTTON_HEIGHT))
                .rounded(px(BUTTON_HEIGHT / 2.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(fg))
                .child(label);
            b = match bg {
                Some(bg) => b.bg(c(bg)),
                None if !installed => b.bg(gpui::linear_gradient(
                    90.,
                    gpui::linear_color_stop(c(premium_bg.0), 0.),
                    gpui::linear_color_stop(c(premium_bg.1), 1.),
                )),
                None => b.hover(|d| d.bg(c(p.attention_button_bg_over))),
            };
            if let Some(n) = badge {
                let (badge_bg, badge_fg) = if installed {
                    (p.attention_button_fg, p.window_bg)
                } else {
                    (p.active_button_fg, p.active_button_bg)
                };
                b = b.child(
                    div()
                        .ml(px(6.))
                        .px(px(5.))
                        .h(px(18.))
                        .rounded(px(9.))
                        .bg(c(badge_bg))
                        .text_color(c(badge_fg))
                        .text_size(scale::font(12.).size)
                        .flex()
                        .items_center()
                        .child(n.to_string()),
                );
            }
            b.on_click(cx.listener(move |this, _, _, cx| {
                if installed {
                    this.emoji_set_removing = Some((r.clone(), title.clone()));
                    this.layers.show_layer(LayerBox::RemoveEmojiSet);
                } else if premium_set && !premium {
                    this.show_toast("Subscribe to Telegram Premium to unlock this emoji.", None);
                } else {
                    let _ = this.commands.send(Command::ToggleEmojiSet {
                        set: r.clone(),
                        install: true,
                    });
                    this.layers.hide_layer();
                }
                cx.notify();
            }))
        });

        div()
            .id("emoji-set-box")
            .occlude()
            .w(px(BOX_WIDE_WIDTH))
            .flex()
            .flex_col()
            .bg(c(p.box_bg))
            .rounded(px(BOX_RADIUS))
            .shadow(box_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(list)
            .child(div().h(px(16.)))
            .children(button)
            .into_any_element()
    }
}
