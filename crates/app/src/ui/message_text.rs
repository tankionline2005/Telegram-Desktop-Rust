//! Message texts in the chat: their layouts (cached), selection, clicks on links,
//! code and spoilers, copying, and opening links (`HistoryInner`'s text handling,
//! `core/click_handler_types.cpp`, `Core::TryConvertUrlToLocal`).

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, ClipboardItem, Context, Window};
use tdr_data::tdr_storage::PeerKey;

use super::MainView;
use crate::bridge::{Command, MessageItem};
use crate::rich_text::{Hit, Layout, Link, Measured, TextColors, TextEntity, TextEvent, TextState};
use crate::toast::{Toast, ToastIcon};

/// A message of a chat.
pub(super) type TextKey = (PeerKey, i32);

/// Keep at most this many measured texts.
const CACHE_LIMIT: usize = 3000;

pub(super) struct RichEntry {
    text: String,
    entities: Vec<TextEntity>,
    scale: u32,
    measured: Rc<Measured>,
    /// The last layout: width, skip block and the lines.
    layout: Option<((u32, u32, u32), Rc<Layout>)>,
}

/// Selected text in one message.
#[derive(Clone, Debug)]
pub(super) struct TextSelection {
    key: TextKey,
    /// Where the drag started; a double click selects words around it.
    anchor: Range<usize>,
    range: Range<usize>,
    dragging: bool,
    words: bool,
}

/// A press on a link, a spoiler or a code block's header, acted on at release
/// over the same thing.
#[derive(Clone, Debug)]
pub(super) struct TextPress {
    key: TextKey,
    hit: Hit,
}

#[derive(Default)]
pub(super) struct TextUi {
    pub(super) cache: std::cell::RefCell<HashMap<TextKey, RichEntry>>,
    selection: Option<TextSelection>,
    hover: Option<(TextKey, u16)>,
    press: Option<TextPress>,
    /// Messages whose spoilers were clicked open (all of a text at once).
    revealed: HashMap<TextKey, Instant>,
    /// Open this chat at the next frame (a resolved username; needs the window).
    pub(super) pending_open: Option<(PeerKey, Option<i32>)>,
    pub(super) toast: Option<Toast>,
    pub(super) menu: Option<super::menus::OpenMenu>,
    pub(super) code_hint: Option<super::menus::CodeHint>,
    /// The chat the revealed spoilers belong to.
    revealed_peer: Option<PeerKey>,
    /// The press reaction on a link or code.
    ripple: Option<TextRipple>,
    /// Code just copied: its hint stays away until the mouse leaves that line.
    pub(super) code_suppressed: Option<(TextKey, gpui::Bounds<gpui::Pixels>)>,
}

struct TextRipple {
    key: TextKey,
    rects: Vec<gpui::Bounds<gpui::Pixels>>,
    center: gpui::Point<gpui::Pixels>,
    start: Instant,
    released: Option<Instant>,
    mono: bool,
}

/// `defaultRippleAnimation`: spreads in 650 ms, fades in 200 ms once released.
const RIPPLE_SHOW_MS: f32 = 650.;
const RIPPLE_HIDE_MS: f32 = 200.;

fn ordered(a: usize, b: usize) -> Range<usize> {
    a.min(b)..a.max(b)
}

/// A t.me link that tdesktop opens inside (`TryConvertUrlToLocal`).
#[derive(Debug, PartialEq)]
enum LocalLink {
    Username {
        name: String,
        post: Option<i32>,
    },
    /// `t.me/c/<channel>/<post>`: a message of a channel or supergroup by its id.
    Channel {
        id: i64,
        post: Option<i32>,
    },
}

fn is_username(s: &str) -> bool {
    s.len() >= 4
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

fn local_link(url: &str) -> Option<LocalLink> {
    let lower = url.to_ascii_lowercase();
    if let Some(query) = lower.strip_prefix("tg://resolve?") {
        let mut name = None;
        let mut post = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "domain" => name = Some(v.to_string()),
                "post" => post = v.parse().ok(),
                _ => {}
            }
        }
        return name
            .filter(|n| is_username(n))
            .map(|name| LocalLink::Username { name, post });
    }
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !matches!(host, "t.me" | "telegram.me" | "telegram.dog") {
        return None;
    }
    // The original case of the username (resolving ignores it anyway).
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["c", id, post, ..] => Some(LocalLink::Channel {
            id: id.parse().ok()?,
            post: post.parse().ok(),
        }),
        [name, rest @ ..] if is_username(name) && !RESERVED.contains(name) => {
            Some(LocalLink::Username {
                name: name.to_string(),
                post: rest.first().and_then(|p| p.parse().ok()),
            })
        }
        _ => None,
    }
}

/// Paths of t.me that are not usernames.
const RESERVED: &[&str] = &[
    "joinchat",
    "addstickers",
    "addemoji",
    "addtheme",
    "addlist",
    "share",
    "proxy",
    "socks",
    "setlanguage",
    "login",
    "confirmphone",
    "invoice",
    "boost",
    "giftcode",
    "contact",
    "iv",
    "bg",
    "msg",
];

impl MainView {
    /// The measured text of a message (cached until its text or the scale changes).
    pub(super) fn rich_measured(
        &self,
        key: TextKey,
        text: &str,
        entities: &[TextEntity],
        window: &Window,
    ) -> Rc<Measured> {
        let scale = window.scale_factor().to_bits();
        let mut cache = self.text_ui.cache.borrow_mut();
        if let Some(e) = cache.get(&key)
            && e.scale == scale
            && e.text == text
            && e.entities == entities
        {
            return e.measured.clone();
        }
        if cache.len() > CACHE_LIMIT {
            cache.clear();
        }
        let rich = Rc::new(crate::rich_text::prepare(text, entities));
        let measured = Rc::new(Measured::new(rich, window));
        cache.insert(
            key,
            RichEntry {
                text: text.to_string(),
                entities: entities.to_vec(),
                scale,
                measured: measured.clone(),
                layout: None,
            },
        );
        measured
    }

    pub(super) fn rich_layout(
        &self,
        key: TextKey,
        measured: &Rc<Measured>,
        width: f32,
        skip: (f32, f32),
    ) -> Rc<Layout> {
        let size = (width.to_bits(), skip.0.to_bits(), skip.1.to_bits());
        let mut cache = self.text_ui.cache.borrow_mut();
        if let Some(e) = cache.get_mut(&key)
            && Rc::ptr_eq(&e.measured, measured)
        {
            if let Some((s, layout)) = &e.layout
                && *s == size
            {
                return layout.clone();
            }
            let layout = Rc::new(measured.layout(width, skip));
            e.layout = Some((size, layout.clone()));
            return layout;
        }
        Rc::new(measured.layout(width, skip))
    }

    /// `TextPalette` of a message (`inTextPalette` / `outTextPalette`) and its quote
    /// color: the outgoing bar color, or the (original) sender's name color.
    pub(super) fn text_colors(&self, item: &MessageItem, caption_only: bool) -> TextColors {
        let p = self.palette();
        let content_color = item
            .forwarded
            .as_ref()
            .and_then(|f| f.author.as_ref().map(|a| a.color))
            .unwrap_or(item.author.color);
        // `SyntaxHighlightColors`.
        let syntax = [
            p.statistics_chart_line_lightblue,
            p.statistics_chart_line_red,
            p.statistics_chart_line_red,
            p.statistics_chart_line_orange,
            p.statistics_chart_line_red,
            p.statistics_chart_line_blue,
            p.statistics_chart_line_purple,
            p.statistics_chart_line_green,
        ];
        if item.out {
            TextColors {
                fg: if caption_only {
                    p.msg_out_date_fg
                } else {
                    p.history_text_out_fg
                },
                link: p.history_link_out_fg,
                mono: p.msg_out_mono_fg,
                select_bg: p.msg_out_bg_selected,
                select_fg: p.history_text_out_fg_selected,
                select_link: p.history_link_out_fg_selected,
                select_mono: p.msg_out_mono_fg_selected,
                spoiler: p.msg_out_date_fg,
                select_spoiler: p.msg_out_date_fg_selected,
                quote: p.msg_out_reply_bar_color,
                dark: self.dark,
                syntax,
            }
        } else {
            TextColors {
                fg: if caption_only {
                    p.msg_in_date_fg
                } else {
                    p.history_text_in_fg
                },
                link: p.history_link_in_fg,
                mono: p.msg_in_mono_fg,
                select_bg: p.msg_in_bg_selected,
                select_fg: p.history_text_in_fg_selected,
                select_link: p.history_link_in_fg_selected,
                select_mono: p.msg_in_mono_fg_selected,
                spoiler: p.msg_in_date_fg,
                select_spoiler: p.msg_in_date_fg_selected,
                quote: super::name_color(p, content_color),
                dark: self.dark,
                syntax,
            }
        }
    }

    pub(super) fn text_state(&self, key: TextKey) -> (TextState, bool) {
        let t = &self.text_ui;
        let selection = t.selection.as_ref().filter(|s| s.key == key);
        (
            TextState {
                selection: selection.map(|s| s.range.clone()),
                hovered_link: t.hover.filter(|(k, _)| *k == key).map(|(_, l)| l),
                pressed_link: t
                    .press
                    .as_ref()
                    .filter(|p| p.key == key)
                    .and_then(|p| p.hit.link),
                revealed: t.revealed.get(&key).copied(),
                code_highlight: self.code_highlight(key),
                ripple: t.ripple.as_ref().filter(|r| r.key == key).map(|r| {
                    let t = (r.start.elapsed().as_secs_f32() * 1000. / RIPPLE_SHOW_MS).min(1.) - 1.;
                    crate::rich_text::PressRipple {
                        rects: r.rects.clone(),
                        center: r.center,
                        // easeOutQuint
                        shown: t * t * t * t * t + 1.,
                        opacity: r.released.map_or(1., |at| {
                            1. - (at.elapsed().as_secs_f32() * 1000. / RIPPLE_HIDE_MS).min(1.)
                        }),
                        mono: r.mono,
                    }
                }),
            },
            selection.is_some_and(|s| s.dragging),
        )
    }

    pub(super) fn on_text_event(
        &mut self,
        key: TextKey,
        layout: &Layout,
        event: TextEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let revealed = self.text_ui.revealed.contains_key(&key);
        match event {
            TextEvent::Down {
                hit,
                click_count,
                shift,
                position,
            } => {
                let hidden_spoiler = hit.spoiler && !revealed;
                // The press reaction: links, code and a code block's header.
                if !hidden_spoiler {
                    let rich = layout.rich();
                    let target = match (hit.pre_header, hit.link) {
                        (Some(q), _) => layout.pre_header_rect(q).map(|r| (vec![r], true)),
                        (None, Some(l)) => Some((
                            layout.link_rects(l),
                            matches!(rich.link(l), Some(Link::Code(_))),
                        )),
                        _ => None,
                    };
                    if let Some((rects, mono)) = target.filter(|(r, _)| !r.is_empty()) {
                        self.text_ui.ripple = Some(TextRipple {
                            key,
                            rects,
                            center: position,
                            start: Instant::now(),
                            released: None,
                            mono,
                        });
                    }
                }
                if hidden_spoiler || hit.link.is_some() || hit.pre_header.is_some() {
                    self.text_ui.press = Some(TextPress { key, hit });
                    cx.notify();
                    return;
                }
                let rich = layout.rich();
                let selection = match self.text_ui.selection.take() {
                    Some(s) if shift && s.key == key => TextSelection {
                        range: ordered(s.anchor.start, hit.index),
                        dragging: true,
                        ..s
                    },
                    _ if click_count >= 2 => {
                        let word = rich.word_at(hit.index);
                        TextSelection {
                            key,
                            anchor: word.clone(),
                            range: word,
                            dragging: true,
                            words: true,
                        }
                    }
                    _ => TextSelection {
                        key,
                        anchor: hit.index..hit.index,
                        range: hit.index..hit.index,
                        dragging: true,
                        words: false,
                    },
                };
                self.text_ui.selection = Some(selection);
                cx.notify();
            }
            TextEvent::Move {
                hit,
                inside,
                pressed,
                origin,
            } => {
                let code = hit.code.filter(|_| inside && !pressed);
                let code_changed = self.code_hover(key, code, origin);
                let hover = if inside && !(hit.spoiler && !revealed) {
                    hit.link.map(|l| (key, l))
                } else {
                    None
                };
                let mut changed = code_changed;
                let mine = self.text_ui.hover.is_some_and(|(k, _)| k == key);
                if hover.is_some() || mine {
                    changed |= self.text_ui.hover != hover;
                    self.text_ui.hover = hover;
                }
                // A press on a link that moves becomes a selection.
                if pressed
                    && let Some(press) = &self.text_ui.press
                    && press.key == key
                    && press.hit.index != hit.index
                {
                    let from = press.hit.index;
                    self.text_ui.press = None;
                    self.release_ripple();
                    self.text_ui.selection = Some(TextSelection {
                        key,
                        anchor: from..from,
                        range: ordered(from, hit.index),
                        dragging: true,
                        words: false,
                    });
                    changed = true;
                }
                if let Some(s) = &mut self.text_ui.selection
                    && s.key == key
                    && s.dragging
                {
                    if pressed {
                        let range = if s.words {
                            let word = layout.rich().word_at(hit.index);
                            s.anchor.start.min(word.start)..s.anchor.end.max(word.end)
                        } else {
                            ordered(s.anchor.start, hit.index)
                        };
                        if range != s.range {
                            s.range = range;
                            changed = true;
                        }
                    } else {
                        s.dragging = false;
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            }
            TextEvent::Up { hit, inside } => {
                self.release_ripple();
                if let Some(press) = self.text_ui.press.take()
                    && press.key == key
                    && inside
                {
                    if press.hit.spoiler && !revealed {
                        if hit.spoiler {
                            self.text_ui.revealed.insert(key, Instant::now());
                        }
                    } else if let Some(quote) = press.hit.pre_header {
                        if hit.pre_header == Some(quote)
                            && let Some(code) = layout.rich().quote_code(quote)
                        {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                            self.show_toast("Code copied to clipboard.", Some(ToastIcon::Copy));
                        }
                    } else if let Some(link) = press.hit.link
                        && hit.link == Some(link)
                        && let Some(link) = layout.rich().link(link).cloned()
                    {
                        self.activate_link(key, link, &hit, layout, window, cx);
                    }
                }
                if let Some(s) = &mut self.text_ui.selection
                    && s.key == key
                {
                    s.dragging = false;
                    if s.range.is_empty() {
                        self.text_ui.selection = None;
                    }
                }
                cx.notify();
            }
            TextEvent::Context { hit, position } => {
                self.text_ui.press = None;
                self.open_message_menu(key, Some(&hit), Some(layout.rich()), position);
                cx.notify();
            }
            TextEvent::DownOutside => {
                if self
                    .text_ui
                    .selection
                    .as_ref()
                    .is_some_and(|s| s.key == key && !s.dragging)
                {
                    self.text_ui.selection = None;
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn show_toast(&mut self, text: &str, icon: Option<ToastIcon>) {
        self.text_ui.toast = Some(Toast::new(text.to_string(), icon));
    }

    /// Ctrl+C with selected message text. `false` when nothing is selected.
    pub(super) fn copy_selected_text(&mut self, cx: &mut App) -> bool {
        let Some(s) = &self.text_ui.selection else {
            return false;
        };
        if s.range.is_empty() {
            return false;
        }
        let cache = self.text_ui.cache.borrow();
        let Some(entry) = cache.get(&s.key) else {
            return false;
        };
        let text = entry.measured.rich.selected_text(s.range.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn release_ripple(&mut self) {
        if let Some(r) = &mut self.text_ui.ripple {
            r.released.get_or_insert_with(Instant::now);
        }
    }

    /// Drops a ripple that has faded; true while one animates.
    pub(super) fn tick_ripple(&mut self) -> bool {
        if self.text_ui.ripple.as_ref().is_some_and(|r| {
            r.released
                .is_some_and(|t| t.elapsed().as_secs_f32() * 1000. >= RIPPLE_HIDE_MS)
        }) {
            self.text_ui.ripple = None;
        }
        self.text_ui.ripple.is_some()
    }

    /// The selected range of this message's text, if any.
    pub(super) fn text_selection_range(&self, key: TextKey) -> Option<Range<usize>> {
        self.text_ui
            .selection
            .as_ref()
            .filter(|s| s.key == key && !s.range.is_empty())
            .map(|s| s.range.clone())
    }

    /// A message's shown text ("Copy Text").
    pub(super) fn message_text(&self, key: TextKey) -> Option<String> {
        let cache = self.text_ui.cache.borrow();
        cache.get(&key).map(|e| e.measured.rich.text.clone())
    }

    /// `Data::Session::hideShownSpoilers`: showing another chat hides every spoiler
    /// that was clicked open.
    pub(super) fn hide_spoilers_on_chat_change(&mut self) {
        if self.text_ui.revealed_peer != self.selected {
            self.text_ui.revealed_peer = self.selected;
            self.text_ui.revealed.clear();
        }
    }

    /// Esc: a selection goes first.
    pub(super) fn clear_text_selection(&mut self) -> bool {
        self.text_ui.selection.take().is_some()
    }

    fn activate_link(
        &mut self,
        key: TextKey,
        link: Link,
        hit: &Hit,
        layout: &Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = window.mouse_position();
        match link {
            Link::Code(code) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                self.show_toast("Text copied to clipboard.", Some(ToastIcon::Copy));
                // Copied: the hint goes until the mouse leaves this line (ours).
                self.dismiss_code_hint(key);
            }
            Link::Url(url) => self.open_url(&url, window, cx),
            // tdesktop opens the mail app; we show the menu the right button shows
            // (an intended difference).
            Link::Email(_) => self.open_message_menu(key, Some(hit), Some(layout.rich()), at),
            Link::Mention(username) => {
                let _ = self.commands.send(Command::OpenUsername {
                    username,
                    post: None,
                });
            }
            // tdesktop shows the profile; profiles come later, the chat opens.
            Link::MentionName(user) => {
                self.go_to_original(PeerKey::user(user), None, window, cx);
            }
            Link::BotCommand(command) => {
                let _ = self.commands.send(Command::Send {
                    peer: key.0,
                    text: command,
                });
            }
            Link::Date(date) => self.open_date_menu(date, at),
            Link::Phone(phone) => self.open_phone_menu(phone, at),
            Link::BankCard(card) => self.open_card_menu(card, at),
            // Search by hashtag comes later.
            Link::Hashtag(_) | Link::Cashtag(_) => {}
        }
        cx.notify();
    }

    /// A link: t.me links open here (`TryConvertUrlToLocal`), the rest in the browser.
    fn open_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        match local_link(url) {
            Some(LocalLink::Username { name, post }) => {
                let _ = self.commands.send(Command::OpenUsername {
                    username: name,
                    post,
                });
            }
            Some(LocalLink::Channel { id, post }) => {
                self.go_to_original(PeerKey::channel(id), post, window, cx);
            }
            None => {
                let has_scheme = url.split_once(':').is_some_and(|(scheme, _)| {
                    !scheme.is_empty()
                        && scheme
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                });
                // `QUrl::fromUserInput`: an address without a scheme is http.
                let target = if has_scheme {
                    url.to_string()
                } else {
                    format!("http://{url}")
                };
                cx.open_url(&target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_links_open_inside() {
        assert_eq!(
            local_link("https://t.me/durov"),
            Some(LocalLink::Username {
                name: "durov".into(),
                post: None
            })
        );
        assert_eq!(
            local_link("t.me/telegram/123?single"),
            Some(LocalLink::Username {
                name: "telegram".into(),
                post: Some(123)
            })
        );
        assert_eq!(
            local_link("https://t.me/c/1234567/89"),
            Some(LocalLink::Channel {
                id: 1234567,
                post: Some(89)
            })
        );
        assert_eq!(
            local_link("tg://resolve?domain=durov&post=5"),
            Some(LocalLink::Username {
                name: "durov".into(),
                post: Some(5)
            })
        );
        assert_eq!(local_link("https://t.me/joinchat/abc"), None);
        assert_eq!(local_link("https://t.me/+abcdef"), None);
        assert_eq!(local_link("https://example.com/durov"), None);
    }
}
