//! The message field: multi-line, wrapping, growing up to tdesktop's maximum height and
//! scrolling inside after that, with formatting (`Ui::InputField`'s tags).
//!
//! Started from GPUI's `examples/input.rs` (Copyright Zed Industries, Apache-2.0):
//! IME composition, clipboard, grapheme-aware movement. Added: wrapping and several
//! paragraphs, Enter sends / Shift+Enter breaks the line (as in tdesktop), Up/Down by
//! visual line, inner scrolling, drafts (`set_text`), tdesktop palette colors.
//!
//! The text is laid out and drawn by `rich_text`, as messages are: the same line
//! breaks, block quotes and code blocks, spoilers under particles. Formatting is kept
//! as tags over byte ranges (tdesktop's `TextWithTags`); block tags (quote, code
//! block) always cover whole paragraphs. Shortcuts are tdesktop's
//! (`InputField::MarkdownActions`), a second press on fully formatted text removes
//! the format (`toggleSelectionMarkdown`).

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, SharedString, Size, Style, UTF16Selection, Window, actions, div, fill,
    linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgba, size,
};
use tdr_theme::{Color, Palette};
use unicode_segmentation::*;

use crate::rich_text::{
    Kind, Layout as RichLayout, Measured, RichText, TextColors, TextEntity, TextState,
};

actions!(
    text_input,
    [
        PastePlain,
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Enter,
        Newline,
        Undo,
        Redo,
        FormatBold,
        FormatItalic,
        FormatUnderline,
        FormatStrike,
        FormatBlockquote,
        FormatMono,
        FormatSpoiler,
        FormatClear,
        FormatLink,
        FormatDate,
        Tab,
    ]
);

/// A format of the field's text (tdesktop's tags: `**`, `__`, `^^`, `~~`, `` ` ``,
/// ` ``` `, `>`, `||`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tag {
    Bold,
    Italic,
    Underline,
    Strike,
    /// One-line code; on whole lines it becomes a code block.
    Code,
    Pre,
    Blockquote,
    Spoiler,
    /// A custom emoji over its fallback emoji (one per emoji, never merged).
    CustomEmoji(i64),
}

impl Tag {
    /// Covers whole paragraphs.
    pub fn block(self) -> bool {
        matches!(self, Tag::Pre | Tag::Blockquote)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TagSpan {
    pub range: Range<usize>,
    pub tag: Tag,
}

/// Half of the caret's blink period: Windows' `GetCaretBlinkTime` (Qt's
/// `cursorFlashTime` is twice it); `None` when blinking is off.
fn caret_blink_time() -> Option<std::time::Duration> {
    #[cfg(windows)]
    {
        // SAFETY: no arguments, returns the user's setting.
        let ms = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime() };
        (ms != 0 && ms != u32::MAX).then(|| std::time::Duration::from_millis(ms.into()))
    }
    #[cfg(not(windows))]
    {
        Some(std::time::Duration::from_millis(500))
    }
}

/// Emitted on Enter with the (trimmed) text and its formatting; the field is cleared.
pub struct Submit {
    pub text: String,
    pub tags: Vec<TagSpan>,
    /// Languages of the code blocks the markdown made (```` ```rust ````).
    pub languages: Vec<(Range<usize>, String)>,
}

/// The right button in the field: its menu at this window point.
pub struct FieldMenu(pub Point<Pixels>);

const CONTEXT: &str = "TextInput";
/// chat_helpers.style: historyComposeFieldMaxHeight. Not a whole number of lines:
/// scrolled down, the top line shows in part, under the fade.
const MAX_HEIGHT: f32 = 224.;
/// historyComposeFieldFadeHeight: the fades over a scrolled field's edges.
const FADE_HEIGHT: f32 = 6.;
/// At most this many undo steps.
const UNDO_LIMIT: usize = 200;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(CONTEXT)),
        // `insertFromMimeDataInner`: Ctrl+Shift+V drops the formatting.
        KeyBinding::new("secondary-shift-v", PastePlain, Some(CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        // Qt's standard keys of a text edit (`QKeySequence` on Windows).
        KeyBinding::new("shift-home", SelectHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(CONTEXT)),
        KeyBinding::new("secondary-home", DocStart, Some(CONTEXT)),
        KeyBinding::new("secondary-end", DocEnd, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-home", SelectDocStart, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-end", SelectDocEnd, Some(CONTEXT)),
        KeyBinding::new("secondary-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("secondary-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-left", SelectWordLeft, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-right", SelectWordRight, Some(CONTEXT)),
        KeyBinding::new("secondary-backspace", DeleteWordLeft, Some(CONTEXT)),
        KeyBinding::new("secondary-delete", DeleteWordRight, Some(CONTEXT)),
        KeyBinding::new("shift-backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("shift-insert", Paste, Some(CONTEXT)),
        KeyBinding::new("secondary-insert", Copy, Some(CONTEXT)),
        KeyBinding::new("shift-delete", Cut, Some(CONTEXT)),
        KeyBinding::new("enter", Enter, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(CONTEXT)),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        KeyBinding::new("secondary-z", Undo, Some(CONTEXT)),
        KeyBinding::new("secondary-y", Redo, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-z", Redo, Some(CONTEXT)),
        // `InputField::MarkdownActions`.
        KeyBinding::new("secondary-b", FormatBold, Some(CONTEXT)),
        KeyBinding::new("secondary-i", FormatItalic, Some(CONTEXT)),
        KeyBinding::new("secondary-u", FormatUnderline, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-x", FormatStrike, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-.", FormatBlockquote, Some(CONTEXT)),
        KeyBinding::new("secondary-shift->", FormatBlockquote, Some(CONTEXT)),
        KeyBinding::new("secondary->", FormatBlockquote, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-m", FormatMono, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-p", FormatSpoiler, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-n", FormatClear, Some(CONTEXT)),
        KeyBinding::new("secondary-k", FormatLink, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-d", FormatDate, Some(CONTEXT)),
    ]);
}

/// One undo step: the text, its formatting and the selection.
#[derive(Clone)]
struct Snapshot {
    content: SharedString,
    tags: Vec<TagSpan>,
    selected: Range<usize>,
    reversed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// Letters typed one after another: one undo step until a space, Enter,
    /// a deletion or a move (Qt's edit blocks).
    Typing,
    Other,
}

/// The measured text, kept while the text, formatting, composition and scale stay.
type MeasureKey = (SharedString, Vec<TagSpan>, Option<Range<usize>>, bool, u32);
type MeasureCache = Rc<RefCell<Option<(MeasureKey, Rc<Measured>)>>>;

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    tags: Vec<TagSpan>,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// The last layout and where it was painted.
    layout: Option<Rc<RichLayout>>,
    bounds: Option<Bounds<Pixels>>,
    measured: MeasureCache,
    /// Scrolled-out height when the text is taller than the maximum.
    scroll_y: Pixels,
    is_selecting: bool,
    /// The caret blinks from this moment (reset by typing, moving and focusing, as
    /// Qt's text controls do), and what it was reset for.
    blink_since: std::time::Instant,
    blink_state: (SharedString, Range<usize>, bool),
    palette: &'static Palette,
    /// The block quote's color (`coloredQuoteCache` of your own color).
    quote_color: Color,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<EditKind>,
    /// One line only (the chat search): no line feeds, typed or pasted.
    single_line: bool,
    /// `InstantReplaces::Default` while typing (the message field).
    instant_replaces: bool,
    /// Text replaced while typing (or by a suggestion): Backspace at its end
    /// brings the original back (`revertFormatReplace`).
    replaced: Vec<Replaced>,
    /// Emoji suggestions over the field (the message field).
    suggest: Option<Rc<RefCell<crate::emoji_suggest::Suggest>>>,
    /// The message field keeps formatting and custom emoji pasted from the
    /// clipboard (`FieldTagMimeProcessor`); the cell says whether custom emoji
    /// need Premium there.
    rich_paste: Option<Rc<std::cell::Cell<bool>>>,
}

/// One replacement made while typing.
#[derive(Clone)]
struct Replaced {
    range: Range<usize>,
    original: String,
}

/// An object over `range` after `edit` replaced `inserted` bytes: moved, or gone
/// when the edit touches it (text typed right after it is not part of it).
fn shift_object(
    range: &Range<usize>,
    edit: &Range<usize>,
    inserted: usize,
) -> Option<Range<usize>> {
    if edit.end <= range.start {
        let start = range.start - edit.len() + inserted;
        Some(start..start + range.len())
    } else if edit.start >= range.end {
        Some(range.clone())
    } else {
        None
    }
}

impl EventEmitter<Submit> for TextInput {}
impl EventEmitter<FieldMenu> for TextInput {}

/// The line height of the field's font (`historyComposeField`: the 13 px font, Qt's
/// rounded metrics at the current scale).
pub fn line_height() -> Pixels {
    crate::scale::font(13.).line_height()
}

/// The paragraph around `range`: from after the previous line feed to the next one.
fn paragraphs_around(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text[..range.start.min(text.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let end = text[range.end.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i);
    start..end.max(start)
}

/// Ctrl+Left: the start of the word at or before `offset`, spaces skipped (Qt's
/// `PreviousWord`; punctuation is a word of its own).
fn previous_word(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .split_word_bound_indices()
        .rev()
        .find(|(_, w)| !w.chars().all(char::is_whitespace))
        .map_or(0, |(i, _)| i)
}

/// Ctrl+Right: the start of the next word, after the spaces (Qt's `NextWord`).
fn next_word(text: &str, offset: usize) -> usize {
    let mut passed = false;
    for (i, w) in text.split_word_bound_indices() {
        if i + w.len() <= offset {
            continue;
        }
        let blank = w.chars().all(char::is_whitespace);
        if i >= offset && passed && !blank {
            return i;
        }
        if i >= offset || !blank {
            passed = true;
        }
    }
    text.len()
}

/// Keeps tags inside the text and on character boundaries, block tags over whole
/// paragraphs, and joins touching spans of one tag.
fn normalize(text: &str, tags: &mut Vec<TagSpan>) {
    let clamp = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    for t in tags.iter_mut() {
        t.range = clamp(t.range.start)..clamp(t.range.end).max(clamp(t.range.start));
        if t.tag.block() {
            // A part left over that ends with a line feed ends before it (it does not
            // take the next paragraph); one that starts after a line feed likewise.
            while t.range.end > t.range.start && text[..t.range.end].ends_with('\n') {
                t.range.end -= 1;
            }
            while t.range.start < t.range.end && text[t.range.start..].starts_with('\n') {
                t.range.start += 1;
            }
            if !t.range.is_empty() {
                t.range = paragraphs_around(text, t.range.clone());
            }
        }
    }
    tags.retain(|t| !t.range.is_empty());
    tags.sort_by_key(|t| (t.tag, t.range.start));
    let mut merged: Vec<TagSpan> = Vec::with_capacity(tags.len());
    for t in tags.drain(..) {
        match merged.last_mut() {
            Some(last)
                if last.tag == t.tag
                    && t.range.start <= last.range.end
                    && !matches!(t.tag, Tag::CustomEmoji(_)) =>
            {
                last.range.end = last.range.end.max(t.range.end);
            }
            _ => merged.push(t),
        }
    }
    *tags = merged;
}

/// Tags after replacing `range` with `inserted` bytes: text typed inside a span, or
/// right after its end, takes its format (Qt's char format of the previous char).
fn shift_tags(tags: &mut [TagSpan], range: Range<usize>, inserted: usize) {
    let removed = range.end - range.start;
    let map = |x: usize| {
        if x <= range.start {
            x
        } else if x >= range.end {
            x - removed
        } else {
            range.start
        }
    };
    let s = range.start;
    for t in tags.iter_mut() {
        let (mut a, mut b) = (map(t.range.start), map(t.range.end));
        if let Tag::CustomEmoji(_) = t.tag {
            // One object: text typed after it is not part of it, an edit inside or
            // of a part of it removes it.
            let whole = t.range.len();
            if inserted > 0 && a >= s {
                a += inserted;
                b += inserted;
            } else if inserted > 0 && a < s && s < b {
                b = a;
            }
            if b - a != whole {
                b = a;
            }
            t.range = a..b;
            continue;
        }
        if inserted > 0 {
            if a < s && s <= b {
                b += inserted;
            } else if a >= s {
                a += inserted;
                b += inserted;
            }
        }
        t.range = a..b;
    }
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        palette: &'static Palette,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(half) = caret_blink_time() {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(half / 4).await;
                    let alive = this.update(cx, |input, cx| {
                        // Repaint around each toggle only.
                        let phase =
                            input.blink_since.elapsed().as_millis() % half.as_millis().max(1);
                        if phase < (half / 4).as_millis() + 5 && input.blink_state.2 {
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            tags: Vec::new(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: None,
            bounds: None,
            measured: Rc::default(),
            scroll_y: px(0.),
            is_selecting: false,
            blink_since: std::time::Instant::now(),
            blink_state: (SharedString::default(), 0..0, false),
            palette,
            quote_color: palette.history_peer1_name_fg,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            single_line: false,
            instant_replaces: false,
            replaced: Vec::new(),
            suggest: None,
            rich_paste: None,
        }
    }

    /// The message field: instant replaces and emoji suggestions.
    pub fn enable_emoji_helpers(
        &mut self,
        suggest: Rc<RefCell<crate::emoji_suggest::Suggest>>,
        custom_locked: Rc<std::cell::Cell<bool>>,
    ) {
        self.instant_replaces = true;
        self.suggest = Some(suggest);
        self.rich_paste = Some(custom_locked);
    }

    /// The caret's place in the window for a byte offset (the suggestions point
    /// at it), with the line height.
    pub fn caret_bounds(&self, offset: usize) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let at = self.point_for_index(offset.min(self.content.len()))?;
        Some(Bounds::new(
            point(
                bounds.left() + px(at.x),
                bounds.top() + px(at.y) - self.scroll_y,
            ),
            size(px(1.), line_height()),
        ))
    }

    /// Inside a code tag at `pos` (no replaces there).
    fn in_code(&self, pos: usize) -> bool {
        self.tags.iter().any(|t| {
            matches!(t.tag, Tag::Code | Tag::Pre) && t.range.start < pos && t.range.end >= pos
        })
    }

    /// `processInstantReplaces` after one typed character.
    fn instant_replace(&mut self) {
        let pos = self.selected_range.end;
        if !self.instant_replaces || !self.selected_range.is_empty() || self.in_code(pos) {
            return;
        }
        let Some((len, with)) = tdr_emoji::suggestions::instant_replace(&self.content[..pos])
        else {
            return;
        };
        // An emoji in the variant (skin tone) chosen for it.
        let with = match tdr_emoji::by_text(with) {
            Some(e) => {
                tdr_emoji::get(crate::emoji::variant_of(e)).map_or(with.to_string(), |o| o.text())
            }
            None => with.to_string(),
        };
        let from = pos - len;
        let original = self.content[from..pos].to_string();
        self.replace_tracked(from..pos, &with, original);
    }

    /// Replaces `range` by `with`, remembering `original` for Backspace.
    fn replace_tracked(&mut self, range: Range<usize>, with: &str, original: String) {
        self.edit(range.clone(), with, EditKind::Other);
        let end = range.start + with.len();
        self.selected_range = end..end;
        self.last_edit = None;
        self.replaced.push(Replaced {
            range: range.start..end,
            original,
        });
    }

    /// `revertFormatReplace`: Backspace right after a replacement types back what
    /// was replaced.
    fn revert_replace(&mut self) -> bool {
        let pos = self.selected_range.end;
        if !self.selected_range.is_empty() {
            return false;
        }
        let Some(k) = self.replaced.iter().position(|r| r.range.end == pos) else {
            return false;
        };
        let r = self.replaced.remove(k);
        self.edit(r.range.clone(), &r.original, EditKind::Other);
        let end = r.range.start + r.original.len();
        self.selected_range = end..end;
        self.last_edit = None;
        true
    }

    /// `getEmojiQuery`: what to suggest emoji for at the caret, where the query
    /// starts and where the suggestions point.
    fn emoji_query(&self) -> (crate::emoji_suggest::Query, usize, usize) {
        use crate::emoji_suggest::Query;
        let none = (Query::None, 0, 0);
        if !self.selected_range.is_empty() || self.marked_range.is_some() {
            return none;
        }
        let pos = self.selected_range.end.min(self.content.len());
        let line_start = self.content[..pos].rfind('\n').map_or(0, |i| i + 1);
        let line = &self.content[line_start..pos];
        let custom = |a: usize, b: usize| {
            self.tags.iter().any(|t| {
                matches!(t.tag, Tag::CustomEmoji(_)) && t.range.start < b && t.range.end > a
            })
        };
        // The emoji in the line, the text after the last one is the fragment.
        let parts = tdr_emoji::split(line);
        let mut fragment = 0;
        if let Some((range, Some(emoji))) = parts.last()
            && range.end == line.len()
        {
            if custom(line_start + range.start, line_start + range.end) {
                return none;
            }
            // A run of the same emoji before the caret.
            let mut start = range.start;
            let mut count = 1;
            for (r, e) in parts.iter().rev().skip(1) {
                if *e == Some(*emoji)
                    && r.end == start
                    && !custom(line_start + r.start, line_start + r.end)
                {
                    start = r.start;
                    count += 1;
                } else {
                    break;
                }
            }
            let one = range.len();
            return (
                Query::Emoji {
                    emoji: *emoji,
                    count,
                },
                line_start + start,
                pos - one,
            );
        }
        for (range, emoji) in parts.iter().rev() {
            if emoji.is_some() {
                fragment = range.end;
                break;
            }
        }
        let text = &line[fragment..];
        let legacy = tdr_emoji::suggestions::max_length();
        let modern = crate::emoji_suggest::max_query_length();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut units = 0;
        for k in (0..chars.len()).rev() {
            let (at, ch) = chars[k];
            units += ch.len_utf16();
            if ch == ':' {
                let previous = (k > 0).then(|| chars[k - 1].1);
                if previous.is_some_and(char::is_alphanumeric)
                    || k + 1 == chars.len()
                    || chars[k + 1].1.is_whitespace()
                {
                    return none;
                }
                let start = line_start + fragment + at;
                let anchor = line_start + fragment + chars[k + 1].0 + chars[k + 1].1.len_utf8();
                return (Query::Text(text[at..].to_string()), start, anchor);
            }
            if units > legacy && units > modern {
                return none;
            }
        }
        // `suggestExactFirstWord`: the whole field is one fragment.
        let length = text.encode_utf16().count();
        if length == 0
            || text.starts_with(char::is_whitespace)
            || length > modern
            || line_start + fragment != 0
            || pos != self.content.len()
        {
            return none;
        }
        (Query::Text(text.to_string()), 0, 0)
    }

    /// Tells the suggestions about a change of the text (`by_key`: typed).
    fn text_changed_for_suggest(&mut self, by_key: bool) {
        let Some(suggest) = self.suggest.clone() else {
            return;
        };
        let (query, start, anchor) = self.emoji_query();
        suggest
            .borrow_mut()
            .text_changed(query, start, anchor, by_key);
    }

    /// The caret moved without typing: the suggestions hide.
    fn cursor_moved_for_suggest(&mut self) {
        if let Some(suggest) = &self.suggest {
            suggest.borrow_mut().cursor_moved();
        }
    }

    /// A key for the suggestions first; true when they took it.
    fn suggest_key(&mut self, key: crate::emoji_suggest::Key, cx: &mut Context<Self>) -> bool {
        use crate::emoji_suggest::KeyResult;
        let Some(suggest) = self.suggest.clone() else {
            return false;
        };
        let result = suggest.borrow_mut().key(key);
        match result {
            KeyResult::Ignored => false,
            KeyResult::Consumed => {
                cx.notify();
                true
            }
            KeyResult::Chosen(row) => {
                self.choose_suggestion(&row, cx);
                true
            }
        }
    }

    /// `replaceCurrent`: the query becomes the chosen emoji.
    pub fn choose_suggestion(&mut self, row: &crate::emoji_suggest::Row, cx: &mut Context<Self>) {
        use crate::emoji_suggest::Query;
        let Some(suggest) = self.suggest.clone() else {
            return;
        };
        let (query, start) = {
            let s = suggest.borrow();
            (s.query.clone(), s.query_start)
        };
        let Some(emoji) = tdr_emoji::get(row.emoji) else {
            return;
        };
        let text = emoji.text();
        let pos = self.selected_range.end.min(self.content.len());
        match query {
            Query::Text(_) if start <= pos => {
                let original = self.content[start..pos].to_string();
                self.replace_tracked(start..pos, &text, original);
                if let Some(id) = row.custom {
                    self.tags.push(TagSpan {
                        range: start..start + text.len(),
                        tag: Tag::CustomEmoji(id),
                    });
                }
            }
            Query::Emoji { count, .. } if start <= pos => {
                let one = (pos - start) / count.max(1);
                let mut at = start;
                self.edit(start..pos, &text.repeat(count), EditKind::Other);
                for _ in 0..count {
                    if let Some(id) = row.custom {
                        self.tags.push(TagSpan {
                            range: at..at + text.len(),
                            tag: Tag::CustomEmoji(id),
                        });
                    }
                    at += text.len();
                }
                let _ = one;
                self.selected_range = at..at;
                self.last_edit = None;
            }
            _ => return,
        }
        normalize(&self.content, &mut self.tags);
        self.text_changed_for_suggest(false);
        cx.notify();
    }

    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        if !self.suggest_key(crate::emoji_suggest::Key::Tab, cx) {
            cx.propagate();
        }
    }

    /// The lines the text takes, wrapped (1 when empty).
    pub fn shown_lines(&self) -> usize {
        self.layout.as_ref().map_or(1, |l| {
            (l.height / f32::from(line_height())).round().max(1.) as usize
        })
    }

    /// Something besides spaces is typed.
    pub fn has_text(&self) -> bool {
        self.content.chars().any(|c| !c.is_whitespace())
    }

    pub fn set_single_line(&mut self, single: bool) {
        self.single_line = single;
    }

    /// Whether the caret is drawn now: on for the first half of every blink period.
    fn caret_on(&self) -> bool {
        match caret_blink_time() {
            Some(half) => {
                (self.blink_since.elapsed().as_millis() / half.as_millis().max(1)).is_multiple_of(2)
            }
            None => true,
        }
    }

    pub fn set_palette(&mut self, palette: &'static Palette, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    pub fn set_quote_color(&mut self, color: Color, cx: &mut Context<Self>) {
        if self.quote_color != color {
            self.quote_color = color;
            cx.notify();
        }
    }

    pub fn text(&self) -> String {
        self.content.to_string()
    }

    pub fn tags(&self) -> Vec<TagSpan> {
        self.tags.clone()
    }

    /// Replaces the text (e.g. with the draft of another chat), cursor at the end.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.set_text_with_tags(text, Vec::new(), cx);
    }

    pub fn set_text_with_tags(&mut self, text: String, tags: Vec<TagSpan>, cx: &mut Context<Self>) {
        let end = text.len();
        self.content = text.into();
        self.tags = tags;
        normalize(&self.content, &mut self.tags);
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = px(0.);
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.replaced.clear();
        self.cursor_moved_for_suggest();
        cx.notify();
    }

    // The field's menu asks these.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn has_selection(&self) -> bool {
        !self.selected_range.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// The selection has formatting to clear.
    pub fn selection_has_tags(&self) -> bool {
        let r = self.selected_range.clone();
        !r.is_empty()
            && self
                .tags
                .iter()
                .any(|t| t.range.start < r.end && r.start < t.range.end)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            tags: self.tags.clone(),
            selected: self.selected_range.clone(),
            reversed: self.selection_reversed,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.content = s.content;
        self.tags = s.tags;
        self.selected_range = self.valid(s.selected);
        self.selection_reversed = s.reversed;
        self.marked_range = None;
        self.replaced.clear();
    }

    /// Remembers the state before an edit (one step for a run of typed letters).
    fn before_edit(&mut self, kind: EditKind) {
        if kind == EditKind::Other || self.last_edit != Some(EditKind::Typing) {
            self.undo.push(self.snapshot());
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some(kind);
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
            self.last_edit = None;
            cx.notify();
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
            self.last_edit = None;
            cx.notify();
        }
    }

    /// Replaces `range` with `text`, moving the formatting along.
    fn edit(&mut self, range: Range<usize>, text: &str, kind: EditKind) {
        let range = self.valid(range);
        if range.is_empty() && text.is_empty() {
            return;
        }
        self.before_edit(kind);
        self.content =
            (self.content[0..range.start].to_owned() + text + &self.content[range.end..]).into();
        self.replaced = std::mem::take(&mut self.replaced)
            .into_iter()
            .filter_map(|r| {
                shift_object(&r.range, &range, text.len()).map(|range| Replaced { range, ..r })
            })
            .collect();
        shift_tags(&mut self.tags, range, text.len());
        normalize(&self.content, &mut self.tags);
    }

    /// The selection for a format: without a line feed at either end.
    fn format_range(&self) -> Range<usize> {
        let mut r = self.valid(self.selected_range.clone());
        if self.content[r.clone()].starts_with('\n') {
            r.start += 1;
        }
        if r.end > r.start && self.content[r.clone()].ends_with('\n') {
            r.end -= 1;
        }
        r
    }

    fn covered(&self, tag: Tag, r: &Range<usize>) -> bool {
        let mut at = r.start;
        let mut spans: Vec<&TagSpan> = self.tags.iter().filter(|t| t.tag == tag).collect();
        spans.sort_by_key(|t| t.range.start);
        for t in spans {
            if t.range.start <= at && t.range.end > at {
                at = t.range.end;
            }
        }
        at >= r.end
    }

    fn remove_tag(&mut self, tag: Option<Tag>, r: &Range<usize>) {
        let mut out = Vec::with_capacity(self.tags.len());
        for t in self.tags.drain(..) {
            let hit = tag.is_none_or(|tag| t.tag == tag)
                && t.range.start < r.end
                && r.start < t.range.end;
            if !hit {
                out.push(t);
                continue;
            }
            if t.range.start < r.start {
                out.push(TagSpan {
                    range: t.range.start..r.start,
                    tag: t.tag,
                });
            }
            if r.end < t.range.end {
                out.push(TagSpan {
                    range: r.end..t.range.end,
                    tag: t.tag,
                });
            }
        }
        self.tags = out;
    }

    /// `InputField::toggleSelectionMarkdown`: the format is removed where the whole
    /// selection has it, added otherwise. Code over several whole lines is a code
    /// block.
    pub fn toggle(&mut self, tag: Tag, cx: &mut Context<Self>) {
        let mut r = self.format_range();
        if r.is_empty() {
            return;
        }
        let whole_lines = (r.start == 0 || self.content[..r.start].ends_with('\n'))
            && (r.end == self.content.len() || self.content[r.end..].starts_with('\n'));
        // `ChooseCodeTagForRange`: a block only over several whole lines.
        let single_line = !self.content[r.clone()].contains('\n');
        let tag = if tag == Tag::Code && whole_lines && !single_line {
            Tag::Pre
        } else {
            tag
        };
        if tag.block() {
            r = paragraphs_around(&self.content, r);
        }
        self.before_edit(EditKind::Other);
        if self.covered(tag, &r) {
            self.remove_tag(Some(tag), &r);
        } else {
            if tag.block() {
                self.remove_tag(Some(Tag::Pre), &r);
                self.remove_tag(Some(Tag::Blockquote), &r);
            }
            self.tags.push(TagSpan {
                range: r.clone(),
                tag,
            });
        }
        normalize(&self.content, &mut self.tags);
        cx.notify();
    }

    /// `clearSelectionMarkdown`.
    pub fn clear_format(&mut self, cx: &mut Context<Self>) {
        let r = self.format_range();
        if r.is_empty() {
            return;
        }
        self.before_edit(EditKind::Other);
        self.remove_tag(None, &r);
        normalize(&self.content, &mut self.tags);
        cx.notify();
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        // The chat search: Enter opens the first result (search comes later).
        if self.single_line {
            return;
        }
        if self.suggest_key(crate::emoji_suggest::Key::Enter, cx) {
            return;
        }
        let trimmed = self.content.trim();
        if trimmed.is_empty() {
            return;
        }
        let lead = self.content.len() - self.content.trim_start().len();
        let text = trimmed.to_string();
        let mut tags = self.tags.clone();
        for t in &mut tags {
            t.range = t.range.start.saturating_sub(lead).min(text.len())
                ..t.range.end.saturating_sub(lead).min(text.len());
        }
        tags.retain(|t| !t.range.is_empty());
        self.set_text(String::new(), cx);
        // `getTextWithAppliedMarkdown`, then `PrepareForSending`'s trim.
        let applied = crate::markdown::apply(&text, &tags);
        let lead = applied.text.len() - applied.text.trim_start().len();
        let text = applied.text.trim().to_string();
        let fit = |r: &Range<usize>| {
            r.start.saturating_sub(lead).min(text.len())..r.end.saturating_sub(lead).min(text.len())
        };
        let tags: Vec<TagSpan> = applied
            .tags
            .iter()
            .map(|t| TagSpan {
                range: fit(&t.range),
                tag: t.tag,
            })
            .filter(|t| !t.range.is_empty())
            .collect();
        let languages = applied
            .languages
            .iter()
            .map(|(r, l)| (fit(r), l.clone()))
            .collect();
        if text.is_empty() {
            return;
        }
        cx.emit(Submit {
            text,
            tags,
            languages,
        });
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.single_line {
            return;
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.suggest_key(crate::emoji_suggest::Key::Arrow(-1), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.suggest_key(crate::emoji_suggest::Key::Arrow(1), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.suggest_key(crate::emoji_suggest::Key::Arrow(-1), cx) {
            return;
        }
        let target = self.vertical_target(self.cursor_offset(), -1.).unwrap_or(0);
        self.move_to(target, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.suggest_key(crate::emoji_suggest::Key::Arrow(1), cx) {
            return;
        }
        let target = self
            .vertical_target(self.cursor_offset(), 1.)
            .unwrap_or(self.content.len());
        self.move_to(target, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(self.cursor_offset(), -1.).unwrap_or(0);
        self.select_to(target, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .vertical_target(self.cursor_offset(), 1.)
            .unwrap_or(self.content.len());
        self.select_to(target, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    /// Home/End: start and end of the current visual line.
    fn line_start(&self) -> usize {
        self.point_for_index(self.cursor_offset())
            .map(|p| self.index_for_point(point(-1., p.y + 1.)))
            .unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.point_for_index(self.cursor_offset())
            .map(|p| self.index_for_point(point(100_000., p.y + 1.)))
            .unwrap_or(self.content.len())
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(), cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(previous_word(&self.content, self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(next_word(&self.content, self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(previous_word(&self.content, self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_word(&self.content, self.cursor_offset()), cx);
    }

    /// Ctrl+Backspace / Ctrl+Delete: to the word's start / the next word's start
    /// (Qt's `DeleteStartOfWord` / `DeleteEndOfWord`); a selection is deleted.
    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(previous_word(&self.content, self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(next_word(&self.content, self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.revert_replace() {
            self.text_changed_for_suggest(true);
            cx.notify();
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    /// The field's menu: "Delete" removes the selection.
    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let r = self.selected_range.clone();
            self.edit(r.clone(), "", EditKind::Other);
            self.selected_range = r.start..r.start;
            cx.notify();
        }
    }

    /// A press at a window point: the caret (or selection) goes there. The whole
    /// field area takes presses, not only its text lines.
    pub fn press_at(
        &mut self,
        position: Point<Pixels>,
        shift: bool,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        self.last_edit = None;
        let index = self.index_for_mouse_position(position);
        if shift {
            self.select_to(index, cx);
        } else if click_count == 2 {
            // A word (Qt: double click).
            let mut start = 0;
            for (i, word) in self.content.split_word_bound_indices() {
                if index < i + word.len() || i + word.len() == self.content.len() {
                    start = i;
                    let end = i + word.len();
                    self.selected_range = start..end;
                    self.selection_reversed = false;
                    break;
                }
            }
            let _ = start;
            cx.notify();
        } else if click_count >= 3 {
            self.selected_range = paragraphs_around(&self.content, index..index);
            self.selection_reversed = false;
            cx.notify();
        } else {
            self.move_to(index, cx)
        }
    }

    /// Ctrl+Shift+. by the physical key: with another layout (Russian: "ю") no
    /// binding matches it, Qt's shortcuts still take it.
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                GetKeyState, VK_OEM_PERIOD, VK_SHIFT,
            };
            let m = &event.keystroke.modifiers;
            // SAFETY: reads the thread's key state.
            let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
            if m.control && !m.alt && down(VK_SHIFT.0) && down(VK_OEM_PERIOD.0) {
                self.toggle(Tag::Blockquote, cx);
                cx.stop_propagation();
            }
        }
        #[cfg(not(windows))]
        let _ = (event, cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        cx.stop_propagation();
        self.press_at(event.position, event.modifiers.shift, event.click_count, cx);
    }

    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        cx.stop_propagation();
        // A press outside the selection moves the caret there first (Qt).
        let index = self.index_for_mouse_position(event.position);
        if !(self.selected_range.start <= index && index <= self.selected_range.end) {
            self.move_to(index, cx);
        }
        cx.emit(FieldMenu(event.position));
    }

    /// The mouse button is up: a selection by the mouse ends.
    pub fn release(&mut self) {
        self.is_selecting = false;
    }

    fn drag_to(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.is_selecting || event.pressed_button != Some(MouseButton::Left) {
            self.is_selecting = false;
            return;
        }
        self.select_to(self.index_for_mouse_position(event.position), cx);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.paste_tagged(cx) {
            return;
        }
        self.paste_plain(&PastePlain, window, cx);
    }

    fn paste_plain(&mut self, _: &PastePlain, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.last_edit = None;
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
            self.last_edit = None;
        }
    }

    /// `InsertCustomEmojiAtCursor`: the custom emoji `id` over its fallback emoji
    /// `alt` in place of the selection, the caret after it.
    pub fn insert_custom_emoji(&mut self, alt: &str, id: i64, cx: &mut Context<Self>) {
        let r = self.selected_range.clone();
        self.edit(r.clone(), alt, EditKind::Other);
        let end = r.start + alt.len();
        self.tags.push(TagSpan {
            range: r.start..end,
            tag: Tag::CustomEmoji(id),
        });
        normalize(&self.content, &mut self.tags);
        self.selected_range = end..end;
        self.last_edit = None;
        cx.notify();
    }

    /// `InsertEmojiAtCursor`: `text` in place of the selection, the caret after it.
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let r = self.selected_range.clone();
        self.edit(r.clone(), text, EditKind::Other);
        let end = r.start + text.len();
        self.selected_range = end..end;
        self.last_edit = None;
        cx.notify();
    }

    /// tdesktop's tagged text from the clipboard in place of the selection, its
    /// formatting and custom emoji kept (`insertFromMimeDataInner` with the
    /// field's tag processor). `false` when there is none.
    fn paste_tagged(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(locked) = self.rich_paste.as_ref().map(|l| l.get()) else {
            return false;
        };
        let Some(tagged) = crate::clipboard::read_tagged() else {
            return false;
        };
        if tagged.text.is_empty() {
            return false;
        }
        let mut text = tagged.text.clone();
        if self.single_line {
            text = text.replace('\n', " ");
        }
        let r = self.valid(self.selected_range.clone());
        self.edit(r.clone(), &text, EditKind::Other);
        let start = r.start;
        for (range, id) in &tagged.tags {
            for part in crate::clipboard::split_tags(id) {
                let tag = match part {
                    "**" => Tag::Bold,
                    "__" => Tag::Italic,
                    "^^" => Tag::Underline,
                    "~~" => Tag::Strike,
                    "`" => Tag::Code,
                    "||" => Tag::Spoiler,
                    ">" | ">^" => Tag::Blockquote,
                    p if p.starts_with("```") => Tag::Pre,
                    p => match crate::clipboard::custom_emoji_id(p) {
                        // Premium emoji stay plain without Premium.
                        Some(id)
                            if !locked || crate::custom_emoji::info(id).is_none_or(|i| i.free) =>
                        {
                            Tag::CustomEmoji(id)
                        }
                        _ => continue,
                    },
                };
                self.tags.push(TagSpan {
                    range: start + range.start..start + range.end,
                    tag,
                });
            }
        }
        normalize(&self.content, &mut self.tags);
        let end = start + text.len();
        self.selected_range = end..end;
        self.last_edit = None;
        self.text_changed_for_suggest(true);
        cx.notify();
        true
    }

    pub fn paste_text(&mut self, cx: &mut Context<Self>) {
        if self.paste_tagged(cx) {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let r = self.selected_range.clone();
            let mut text = text.replace("\r\n", "\n");
            if self.single_line {
                text = text.replace('\n', " ");
            }
            self.edit(r.clone(), &text, EditKind::Other);
            let end = r.start + text.len();
            self.selected_range = end..end;
            self.last_edit = None;
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        // Nothing selected here: the chat's selected text gets the key.
        if self.selected_range.is_empty() {
            cx.propagate();
        }
        self.copy_selection(cx);
    }

    /// `createMimeDataFromSelectionInner`: the selection with its tags.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let r = self.valid(self.selected_range.clone());
        let singles: Vec<(Range<usize>, String)> = self
            .tags
            .iter()
            .filter_map(|t| {
                let (s, e) = (t.range.start.max(r.start), t.range.end.min(r.end));
                let tag = match t.tag {
                    Tag::Bold => "**".to_string(),
                    Tag::Italic => "__".to_string(),
                    Tag::Underline => "^^".to_string(),
                    Tag::Strike => "~~".to_string(),
                    Tag::Code => "`".to_string(),
                    Tag::Pre => "```".to_string(),
                    Tag::Blockquote => ">".to_string(),
                    Tag::Spoiler => "||".to_string(),
                    Tag::CustomEmoji(id) => crate::clipboard::custom_emoji_link(id),
                };
                (s < e).then(|| (s - r.start..e - r.start, tag))
            })
            .collect();
        crate::clipboard::copy(
            crate::clipboard::Tagged {
                text: self.content[r].to_string(),
                tags: crate::clipboard::combine(&singles),
            },
            cx,
        );
    }

    fn cut(&mut self, _: &Cut, _window: &mut Window, cx: &mut Context<Self>) {
        self.cut_selection(cx);
    }

    pub fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.copy_selection(cx);
            self.delete_selection(cx);
        }
    }

    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    /// Clamps a byte offset into the text, onto a character boundary. Offsets come from
    /// layouts, the platform input method and mouse positions; slicing at a bad one
    /// would panic, and a panic in a GPUI callback ends the process.
    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn valid(&self, range: Range<usize>) -> Range<usize> {
        let start = self.clamp(range.start);
        start..self.clamp(range.end).max(start)
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp(offset);
        self.selected_range = offset..offset;
        self.last_edit = None;
        self.cursor_moved_for_suggest();
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Position of a byte offset relative to the text's top-left (unscrolled).
    fn point_for_index(&self, offset: usize) -> Option<Point<f32>> {
        let layout = self.layout.as_ref()?;
        if self.content.is_empty() {
            return Some(point(0., 0.));
        }
        Some(layout.point_for_index(offset))
    }

    /// Byte offset closest to a point relative to the text's top-left (unscrolled).
    fn index_for_point(&self, at: Point<f32>) -> usize {
        match &self.layout {
            Some(layout) if !self.content.is_empty() => layout.hit(at).index,
            _ => 0,
        }
    }

    fn vertical_target(&self, offset: usize, direction: f32) -> Option<usize> {
        if self.content.is_empty() {
            return None;
        }
        let lh = f32::from(line_height());
        let pos = self.point_for_index(offset)?;
        let y = pos.y + lh * direction + lh / 2.;
        if y < 0. || y >= f32::from(self.total_height()) {
            return None;
        }
        Some(
            self.index_for_point(point(pos.x, y))
                .min(self.content.len()),
        )
    }

    fn total_height(&self) -> Pixels {
        match &self.layout {
            Some(l) => px(l.height).max(line_height()),
            None => line_height(),
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        // An empty field shows (and lays out) the placeholder: its indices are not ours.
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.bounds else {
            return 0;
        };
        let local = point(
            f32::from(position.x - bounds.left()),
            f32::from(position.y - bounds.top() + self.scroll_y),
        );
        self.index_for_point(local).min(self.content.len())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.cursor_moved_for_suggest();
        let offset = self.clamp(offset);
        self.selected_range = self.valid(self.selected_range.clone());
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.last_edit = None;
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// The text as `rich_text` lays it out: the tags as entities, the composition
    /// underlined; the placeholder when empty.
    fn display(&self) -> (SharedString, Vec<TextEntity>, bool) {
        if self.content.is_empty() {
            return (self.placeholder.clone(), Vec::new(), true);
        }
        let mut entities: Vec<TextEntity> = self
            .tags
            .iter()
            .map(|t| TextEntity {
                range: t.range.clone(),
                kind: match t.tag {
                    Tag::Bold => Kind::Bold,
                    Tag::Italic => Kind::Italic,
                    Tag::Underline => Kind::Underline,
                    Tag::Strike => Kind::Strike,
                    Tag::Code => Kind::Code,
                    Tag::Pre => Kind::Pre(String::new()),
                    Tag::Blockquote => Kind::Blockquote,
                    Tag::Spoiler => Kind::Spoiler,
                    Tag::CustomEmoji(id) => Kind::CustomEmoji(id),
                },
            })
            .collect();
        if let Some(m) = &self.marked_range {
            entities.push(TextEntity {
                range: m.clone(),
                kind: Kind::Underline,
            });
        }
        (self.content.clone(), entities, false)
    }

    /// `placeholderFg`, `placeholderFgActive` while focused.
    fn colors(&self, placeholder: bool, focused: bool) -> TextColors {
        let p = self.palette;
        let fg = if placeholder && focused {
            p.placeholder_fg_active
        } else if placeholder {
            p.placeholder_fg
        } else {
            p.history_compose_area_fg
        };
        // Qt's selection under the field's text: the accent at a quarter.
        let select_bg = Color((p.window_bg_active.rgba() & 0xffff_ff00) | 0x40);
        TextColors {
            fg,
            link: p.window_active_text_fg,
            mono: p.msg_in_mono_fg,
            select_bg,
            select_fg: fg,
            select_link: p.window_active_text_fg,
            select_mono: p.msg_in_mono_fg,
            spoiler: p.msg_in_date_fg,
            select_spoiler: p.msg_in_date_fg_selected,
            quote: self.quote_color,
            dark: std::ptr::eq(p, tdr_theme::palette(true)),
            syntax: [fg; 8],
            copy_hint: false,
        }
    }
}

/// Measures (or reuses) the field's text.
fn measured(
    cache: &MeasureCache,
    text: SharedString,
    entities: &[TextEntity],
    key_tags: Vec<TagSpan>,
    marked: Option<Range<usize>>,
    placeholder: bool,
    window: &Window,
) -> Rc<Measured> {
    let key: MeasureKey = (
        text.clone(),
        key_tags,
        marked,
        placeholder,
        window.scale_factor().to_bits(),
    );
    if let Some((k, m)) = cache.borrow().as_ref()
        && *k == key
    {
        return m.clone();
    }
    let rich = Rc::new(crate::rich_text::prepare(&text, entities));
    let m = Rc::new(Measured::new(rich, window));
    *cache.borrow_mut() = Some((key, m.clone()));
    m
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[self.valid(range)].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let joined;
        let new_text = if self.single_line && new_text.contains(['\n', '\r']) {
            joined = new_text.replace("\r\n", " ").replace(['\n', '\r'], " ");
            joined.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.valid(range);
        // A letter typed after letters joins their undo step; a space, a line feed,
        // a deletion or a paste is a step of its own.
        let typing = range.is_empty()
            && new_text.graphemes(true).count() == 1
            && !new_text.chars().any(char::is_whitespace);
        self.edit(
            range.clone(),
            new_text,
            if typing {
                EditKind::Typing
            } else {
                EditKind::Other
            },
        );
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        if new_text.encode_utf16().count() == 1 {
            self.instant_replace();
        }
        self.text_changed_for_suggest(true);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.valid(range);
        self.edit(range.clone(), new_text, EditKind::Typing);
        self.marked_range = if new_text.is_empty() {
            None
        } else {
            Some(range.start..range.start + new_text.len())
        };
        // The new selection is relative to `new_text` (in UTF-16 units), not to the
        // whole content.
        let in_new = |utf16: usize| {
            let mut units = 0;
            for (i, ch) in new_text.char_indices() {
                if units >= utf16 {
                    return i;
                }
                units += ch.len_utf16();
            }
            new_text.len()
        };
        let selected = new_selected_range_utf16
            .as_ref()
            .map(|r| range.start + in_new(r.start)..range.start + in_new(r.end))
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selected_range = self.valid(selected);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.point_for_index(range.start)?;
        let origin = point(
            bounds.left() + px(start.x),
            bounds.top() + px(start.y) - self.scroll_y,
        );
        Some(Bounds::new(origin, size(px(2.), line_height())))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_for_mouse_position(point_in_window);
        Some(self.offset_to_utf16(index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Rc<RichLayout>>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let (text, entities, placeholder) = input.display();
        let tags = input.tags.clone();
        let marked = input.marked_range.clone();
        let cache = input.measured.clone();
        let mut layout_style = Style::default();
        layout_style.size.width = relative(1.).into();
        let id =
            window.request_measured_layout(layout_style, move |known, available, window, _cx| {
                let width = known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(w) => w,
                    _ => px(10_000.),
                });
                let m = measured(
                    &cache,
                    text.clone(),
                    &entities,
                    tags.clone(),
                    marked.clone(),
                    placeholder,
                    window,
                );
                let layout = m.layout(f32::from(width), (0., 0.));
                Size {
                    width,
                    height: px(layout.height)
                        .max(line_height())
                        .min(crate::scale::px(MAX_HEIGHT)),
                }
            });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // Typing, moving the caret or focusing shows it at once and restarts blinking.
        let focused = self.input.read(cx).focus_handle.is_focused(window);
        self.input.update(cx, |input, _| {
            let state = (input.content.clone(), input.selected_range.clone(), focused);
            if state != input.blink_state {
                input.blink_state = state;
                input.blink_since = std::time::Instant::now();
            }
        });
        let input = self.input.read(cx);
        let (text, entities, placeholder) = input.display();
        let m = measured(
            &input.measured,
            text,
            &entities,
            input.tags.clone(),
            input.marked_range.clone(),
            placeholder,
            window,
        );
        let layout = Rc::new(m.layout(f32::from(bounds.size.width), (0., 0.)));
        let lh = line_height();
        let cursor = if input.content.is_empty() {
            point(0., 0.)
        } else {
            layout.point_for_index(input.cursor_offset())
        };
        // Scroll to the caret.
        let mut scroll_y = input.scroll_y;
        let visible = bounds.size.height;
        let cy = px(cursor.y);
        if cy < scroll_y {
            scroll_y = cy;
        } else if cy + lh > scroll_y + visible {
            scroll_y = cy + lh - visible;
        }
        let total = px(layout.height).max(lh);
        scroll_y = scroll_y.min((total - visible).max(px(0.))).max(px(0.));
        let layout_for_input = layout.clone();
        self.input.update(cx, |input, _| {
            input.scroll_y = scroll_y;
            input.layout = Some(layout_for_input);
            input.bounds = Some(bounds);
        });
        Some(layout)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = layout.take() else {
            return;
        };
        let input = self.input.read(cx);
        // A selection by the mouse follows it outside the field too (window-level
        // listeners: an element's own move events stop at its edge).
        if input.is_selecting {
            let entity = self.input.clone();
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble {
                    entity.update(cx, |i, cx| i.drag_to(e, cx));
                }
            });
            let entity = self.input.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble && e.button == MouseButton::Left {
                    entity.update(cx, |i, _| i.release());
                }
            });
        }
        let focus_handle = input.focus_handle.clone();
        let scroll_y = input.scroll_y;
        let placeholder = input.content.is_empty();
        let colors = input.colors(placeholder, focus_handle.is_focused(window));
        let selected = input.selected_range.clone();
        let caret_on = input.caret_on();
        let cursor = if placeholder {
            point(0., 0.)
        } else {
            layout.point_for_index(input.cursor_offset())
        };
        let caret_color = input.palette.active_line_fg;
        let bg = input.palette.history_compose_area_bg.rgba();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let origin = point(bounds.left(), bounds.top() - scroll_y);
        let text_bounds = Bounds::new(origin, size(bounds.size.width, px(layout.height)));
        let lh = line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let state = TextState {
                selection: (!placeholder && !selected.is_empty()).then_some(selected.clone()),
                shown_spoilers: (!placeholder).then_some(selected.clone()),
                ..Default::default()
            };
            RichText::paint_at(layout.clone(), colors, state, text_bounds, window, cx);
            // Qt's default caret: one device pixel wide.
            if focus_handle.is_focused(window) && selected.is_empty() && caret_on {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + px(cursor.x), origin.y + px(cursor.y)),
                        size(px(1. / window.scale_factor()), lh),
                    ),
                    rgba(caret_color.rgba()),
                ));
            }
            // `InitMessageFieldFade`: the background fading to transparent at 90 %
            // over the top while the field is scrolled down, and over the bottom
            // while there is more text below.
            let total = px(layout.height).max(lh);
            let scroll_max = (total - bounds.size.height).max(px(0.));
            let descent = px(crate::scale::font(13.).descent);
            let (solid, clear) = (rgba(bg), rgba(bg & 0xffff_ff00));
            let fade = crate::scale::px(FADE_HEIGHT);
            if scroll_y > px(0.) {
                window.paint_quad(fill(
                    Bounds::new(bounds.origin, size(bounds.size.width, fade)),
                    linear_gradient(
                        180.,
                        linear_color_stop(solid, 0.),
                        linear_color_stop(clear, 0.9),
                    ),
                ));
            }
            if scroll_y + descent < scroll_max {
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left(), bounds.bottom() - fade),
                        size(bounds.size.width, fade),
                    ),
                    linear_gradient(
                        0.,
                        linear_color_stop(solid, 0.),
                        linear_color_stop(clear, 0.9),
                    ),
                ));
            }
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .line_height(line_height())
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_plain))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(|this, _: &FormatBold, _, cx| this.toggle(Tag::Bold, cx)))
            .on_action(cx.listener(|this, _: &FormatItalic, _, cx| this.toggle(Tag::Italic, cx)))
            .on_action(
                cx.listener(|this, _: &FormatUnderline, _, cx| this.toggle(Tag::Underline, cx)),
            )
            .on_action(cx.listener(|this, _: &FormatStrike, _, cx| this.toggle(Tag::Strike, cx)))
            .on_action(
                cx.listener(|this, _: &FormatBlockquote, _, cx| this.toggle(Tag::Blockquote, cx)),
            )
            .on_action(cx.listener(|this, _: &FormatMono, _, cx| this.toggle(Tag::Code, cx)))
            .on_action(cx.listener(|this, _: &FormatSpoiler, _, cx| this.toggle(Tag::Spoiler, cx)))
            .on_action(cx.listener(|this, _: &FormatClear, _, cx| this.clear_format(cx)))
            // Links and dates need their boxes (tdesktop's edit link / date boxes).
            .on_action(cx.listener(|_, _: &FormatLink, _, _| {}))
            .on_action(cx.listener(|_, _: &FormatDate, _, _| {}))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The field's tags as the API's entities, in UTF-16 offsets
/// (`ConvertTextTagsToEntities`).
/// The entities of a sent text, with the code blocks' languages.
pub fn out_entities_with(
    text: &str,
    tags: &[TagSpan],
    languages: &[(Range<usize>, String)],
) -> Vec<tdr_data::OutEntity> {
    let utf16 = |byte: usize| -> i32 {
        text[..byte.min(text.len())]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as i32
    };
    let mut out = out_entities(text, tags);
    for (range, language) in languages {
        let offset = utf16(range.start);
        let length = utf16(range.end) - offset;
        if let Some(e) = out.iter_mut().find(|e| {
            e.offset == offset && e.length == length && matches!(e.kind, tdr_data::OutKind::Pre(_))
        }) {
            e.kind = tdr_data::OutKind::Pre(language.clone());
        }
    }
    out
}

pub fn out_entities(text: &str, tags: &[TagSpan]) -> Vec<tdr_data::OutEntity> {
    use tdr_data::OutKind;
    let utf16 = |byte: usize| -> i32 {
        text[..byte.min(text.len())]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as i32
    };
    let mut out: Vec<tdr_data::OutEntity> = tags
        .iter()
        .filter(|t| !t.range.is_empty())
        .map(|t| {
            let offset = utf16(t.range.start);
            tdr_data::OutEntity {
                offset,
                length: utf16(t.range.end) - offset,
                kind: match t.tag {
                    Tag::Bold => OutKind::Bold,
                    Tag::Italic => OutKind::Italic,
                    Tag::Underline => OutKind::Underline,
                    Tag::Strike => OutKind::Strike,
                    Tag::Code => OutKind::Code,
                    Tag::Pre => OutKind::Pre(String::new()),
                    Tag::Blockquote => OutKind::Blockquote,
                    Tag::Spoiler => OutKind::Spoiler,
                    Tag::CustomEmoji(id) => OutKind::CustomEmoji(id),
                },
            }
        })
        .collect();
    out.sort_by_key(|e| (e.offset, -e.length));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(range: Range<usize>, tag: Tag) -> TagSpan {
        TagSpan { range, tag }
    }

    #[test]
    fn typing_after_a_format_continues_it() {
        let mut tags = vec![span(0..4, Tag::Bold)];
        // "bold" + "er" typed at its end.
        shift_tags(&mut tags, 4..4, 2);
        assert_eq!(tags[0].range, 0..6);
        // Before its start: it moves.
        shift_tags(&mut tags, 0..0, 1);
        assert_eq!(tags[0].range, 1..7);
        // A deletion across its start cuts it.
        shift_tags(&mut tags, 0..3, 0);
        assert_eq!(tags[0].range, 0..4);
    }

    #[test]
    fn entities_are_in_utf16() {
        // Cyrillic is 2 bytes and 1 unit, the emoji 4 bytes and 2.
        let text = "Привет \u{1F600} bold";
        let start = text.find("bold").unwrap();
        let out = out_entities(text, &[span(start..text.len(), Tag::Bold)]);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].offset, out[0].length), (10, 4));
        assert_eq!(out[0].kind, tdr_data::OutKind::Bold);
    }

    #[test]
    fn word_moves_like_qt() {
        let text = "one two,  three\nfour";
        assert_eq!(next_word(text, 0), 4);
        assert_eq!(next_word(text, 1), 4);
        assert_eq!(next_word(text, 4), 7);
        assert_eq!(next_word(text, 7), 10);
        assert_eq!(next_word(text, 10), 16);
        assert_eq!(next_word(text, 16), text.len());
        assert_eq!(previous_word(text, text.len()), 16);
        assert_eq!(previous_word(text, 16), 10);
        assert_eq!(previous_word(text, 12), 10);
        assert_eq!(previous_word(text, 10), 7);
        assert_eq!(previous_word(text, 4), 0);
    }

    #[test]
    fn a_split_block_keeps_to_its_paragraphs() {
        let text = "a quoted line\n\nlet x = 1;";
        // The quote's part left after a code block took the last line.
        let mut tags = vec![span(0..15, Tag::Blockquote), span(15..25, Tag::Pre)];
        normalize(text, &mut tags);
        assert!(tags.contains(&span(0..13, Tag::Blockquote)));
        assert!(tags.contains(&span(15..25, Tag::Pre)));
    }

    #[test]
    fn block_tags_cover_paragraphs() {
        let text = "one\ntwo three\nfour";
        let mut tags = vec![
            span(6..9, Tag::Blockquote),
            span(1..2, Tag::Bold),
            span(2..3, Tag::Bold),
        ];
        normalize(text, &mut tags);
        assert!(tags.contains(&span(4..13, Tag::Blockquote)));
        assert!(tags.contains(&span(1..3, Tag::Bold)));
    }
}
