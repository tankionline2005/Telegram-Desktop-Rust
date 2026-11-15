//! The message field: multi-line, wrapping, growing up to tdesktop's maximum height and
//! scrolling inside after that.
//!
//! Started from GPUI's `examples/input.rs` (Copyright Zed Industries, Apache-2.0):
//! IME composition, clipboard, grapheme-aware movement. Added: wrapping and several
//! paragraphs, Enter sends / Shift+Enter breaks the line (as in tdesktop), Up/Down by
//! visual line, inner scrolling, drafts (`set_text`), tdesktop palette colors.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Size, Style, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px,
    relative, rgba, size,
};
use tdr_theme::Palette;
use unicode_segmentation::*;

actions!(
    text_input,
    [
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
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Enter,
        Newline,
    ]
);

/// Emitted on Enter with the text; the field is cleared.
pub struct Submit(pub String);

const CONTEXT: &str = "TextInput";
/// Line height of the 13 px message font.
pub const LINE_HEIGHT: f32 = 17.;
/// chat_helpers.style: historyComposeFieldMaxHeight (224 px, ~13 lines).
const MAX_LINES: f32 = 13.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
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
        KeyBinding::new("secondary-c", Copy, Some(CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("enter", Enter, Some(CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(CONTEXT)),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
    ]);
}

/// The last layout: one wrapped line per paragraph (text between `\n`).
#[derive(Default)]
struct Layout {
    lines: Vec<WrappedLine>,
    /// Byte offset of each paragraph in the content.
    starts: Vec<usize>,
    /// Top of each paragraph, relative to the text's top (before scrolling).
    tops: Vec<Pixels>,
    bounds: Option<Bounds<Pixels>>,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Layout,
    /// Scrolled-out height when the text is taller than the maximum.
    scroll_y: Pixels,
    is_selecting: bool,
    palette: &'static Palette,
}

impl EventEmitter<Submit> for TextInput {}

fn line_height() -> Pixels {
    px(LINE_HEIGHT)
}

impl TextInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        palette: &'static Palette,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: Layout::default(),
            scroll_y: px(0.),
            is_selecting: false,
            palette,
        }
    }

    pub fn set_palette(&mut self, palette: &'static Palette, cx: &mut Context<Self>) {
        self.palette = palette;
        cx.notify();
    }

    pub fn text(&self) -> String {
        self.content.to_string()
    }

    /// Replaces the text (e.g. with the draft of another chat), cursor at the end.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        let end = text.len();
        self.content = text.into();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_y = px(0.);
        cx.notify();
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.content.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.set_text(String::new(), cx);
        cx.emit(Submit(text));
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(self.cursor_offset(), -1.).unwrap_or(0);
        self.move_to(target, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
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
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    /// Home/End: start and end of the current visual line.
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .point_for_index(self.cursor_offset())
            .and_then(|p| self.index_for_point(point(px(0.), p.y)))
            .unwrap_or(0);
        self.move_to(target, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .point_for_index(self.cursor_offset())
            .and_then(|p| self.index_for_point(point(px(100_000.), p.y)))
            .unwrap_or(self.content.len());
        self.move_to(target, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
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

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
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
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.valid(self.selected_range.clone())].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.valid(self.selected_range.clone())].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
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
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Paragraph containing a byte offset (the end of a paragraph belongs to it).
    fn paragraph_of(&self, offset: usize) -> Option<usize> {
        if self.layout.starts.is_empty() {
            return None;
        }
        Some(
            self.layout
                .starts
                .iter()
                .rposition(|&s| s <= offset)
                .unwrap_or(0),
        )
    }

    /// Position of a byte offset relative to the text's top-left (unscrolled).
    fn point_for_index(&self, offset: usize) -> Option<Point<Pixels>> {
        let p = self.paragraph_of(offset)?;
        let line = self.layout.lines.get(p)?;
        let local = (offset - self.layout.starts[p]).min(line.len());
        let pos = line.position_for_index(local, line_height())?;
        Some(point(pos.x, pos.y + self.layout.tops[p]))
    }

    /// Byte offset closest to a point relative to the text's top-left (unscrolled).
    fn index_for_point(&self, at: Point<Pixels>) -> Option<usize> {
        if self.layout.lines.is_empty() {
            return None;
        }
        let p = self
            .layout
            .tops
            .iter()
            .rposition(|&top| top <= at.y)
            .unwrap_or(0);
        let line = &self.layout.lines[p];
        let local_y = (at.y - self.layout.tops[p]).max(px(0.));
        let height = line.size(line_height()).height;
        let local = point(at.x.max(px(0.)), local_y.min(height - px(1.)).max(px(0.)));
        let index = match line.closest_index_for_position(local, line_height()) {
            Ok(i) | Err(i) => i,
        };
        Some(self.layout.starts[p] + index.min(line.len()))
    }

    fn vertical_target(&self, offset: usize, direction: f32) -> Option<usize> {
        if self.content.is_empty() {
            return None;
        }
        let pos = self.point_for_index(offset)?;
        let y = pos.y + line_height() * direction + line_height() / 2.;
        let total = self.total_height();
        if y < px(0.) || y >= total {
            return None;
        }
        self.index_for_point(point(pos.x, y))
            .map(|i| i.min(self.content.len()))
    }

    fn total_height(&self) -> Pixels {
        match (self.layout.tops.last(), self.layout.lines.last()) {
            (Some(&top), Some(line)) => top + line.size(line_height()).height,
            _ => line_height(),
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        // An empty field shows (and lays out) the placeholder: its indices are not ours.
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.layout.bounds else {
            return 0;
        };
        let local = point(
            position.x - bounds.left(),
            position.y - bounds.top() + self.scroll_y,
        );
        self.index_for_point(local)
            .unwrap_or(0)
            .min(self.content.len())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
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
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.valid(range);
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
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
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
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
            bounds.left() + start.x,
            bounds.top() + start.y - self.scroll_y,
        );
        Some(Bounds::new(origin, size(px(2.), line_height())))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.layout.bounds?;
        let local = point(
            point_in_window.x - bounds.left(),
            point_in_window.y - bounds.top() + self.scroll_y,
        );
        let index = self.index_for_point(local)?;
        Some(self.offset_to_utf16(index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    tops: Vec<Pixels>,
    starts: Vec<usize>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Text runs for `text`, with the IME composition underlined.
fn runs_for(text: &str, marked: Option<&Range<usize>>, run: TextRun) -> Vec<TextRun> {
    match marked {
        Some(m) if m.end <= text.len() => vec![
            TextRun {
                len: m.start,
                ..run.clone()
            },
            TextRun {
                len: m.end - m.start,
                underline: Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: text.len() - m.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        _ => vec![TextRun {
            len: text.len(),
            ..run
        }],
    }
}

/// Shapes `text` paragraph by paragraph (each `\n` starts a new one).
fn shape(
    window: &Window,
    text: &str,
    runs: &[TextRun],
    font_size: Pixels,
    width: Pixels,
) -> (Vec<WrappedLine>, Vec<usize>, Vec<Pixels>) {
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut tops = Vec::new();
    let mut top = px(0.);
    let mut start = 0;
    for paragraph in text.split('\n') {
        let end = start + paragraph.len();
        // Slice the runs to this paragraph.
        let mut para_runs = Vec::new();
        let mut run_start = 0;
        for r in runs {
            let run_end = run_start + r.len;
            let s = run_start.max(start);
            let e = run_end.min(end);
            if s < e {
                para_runs.push(TextRun {
                    len: e - s,
                    ..r.clone()
                });
            }
            run_start = run_end;
        }
        if para_runs.is_empty()
            && let Some(first) = runs.first()
        {
            para_runs.push(TextRun {
                len: 0,
                ..first.clone()
            });
        }
        let shaped = window
            .text_system()
            .shape_text(
                SharedString::from(paragraph.to_string()),
                font_size,
                &para_runs,
                Some(width),
                None,
            )
            .ok()
            .and_then(|mut v| v.pop())
            .unwrap_or_default();
        starts.push(start);
        tops.push(top);
        top += shaped.size(line_height()).height.max(line_height());
        lines.push(shaped);
        start = end + 1;
    }
    (lines, starts, tops)
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
        let text = if input.content.is_empty() {
            input.placeholder.to_string()
        } else {
            input.content.to_string()
        };
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let run = TextRun {
            len: 0,
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut layout_style = Style::default();
        layout_style.size.width = relative(1.).into();
        let id =
            window.request_measured_layout(layout_style, move |known, available, window, _cx| {
                let width = known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(w) => w,
                    _ => px(10_000.),
                });
                let runs = vec![TextRun {
                    len: text.len(),
                    ..run.clone()
                }];
                let (lines, _, tops) = shape(window, &text, &runs, font_size, width);
                let total = match (tops.last(), lines.last()) {
                    (Some(&t), Some(l)) => t + l.size(line_height()).height.max(line_height()),
                    _ => line_height(),
                };
                Size {
                    width,
                    height: total.clamp(line_height(), line_height() * MAX_LINES),
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
        let input = self.input.read(cx);
        let content = input.content.clone();
        let palette = input.palette;
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let (display_text, color) = if content.is_empty() {
            (
                input.placeholder.clone(),
                rgba(palette.placeholder_fg.rgba()).into(),
            )
        } else {
            (content.clone(), style.color)
        };
        let base = TextRun {
            len: 0,
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let marked = if content.is_empty() {
            None
        } else {
            input.marked_range.as_ref()
        };
        let runs = runs_for(&display_text, marked, base);
        let (lines, starts, tops) =
            shape(window, &display_text, &runs, font_size, bounds.size.width);

        // Place the cursor and selection with the new layout, and scroll to the cursor.
        let selected = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let lh = line_height();
        let pos = |offset: usize| -> Point<Pixels> {
            let p = starts.iter().rposition(|&s| s <= offset).unwrap_or(0);
            let line = &lines[p];
            let local = (offset - starts[p]).min(line.len());
            let at = line.position_for_index(local, lh).unwrap_or_default();
            point(at.x, at.y + tops[p])
        };
        let cursor_pos = if content.is_empty() {
            point(px(0.), px(0.))
        } else {
            pos(cursor_offset)
        };
        let mut scroll_y = input.scroll_y;
        let visible = bounds.size.height;
        if cursor_pos.y < scroll_y {
            scroll_y = cursor_pos.y;
        } else if cursor_pos.y + lh > scroll_y + visible {
            scroll_y = cursor_pos.y + lh - visible;
        }
        let total = match (tops.last(), lines.last()) {
            (Some(&t), Some(l)) => t + l.size(lh).height.max(lh),
            _ => lh,
        };
        scroll_y = scroll_y.min((total - visible).max(px(0.))).max(px(0.));
        let origin = point(bounds.left(), bounds.top() - scroll_y);

        let cursor = selected.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(origin.x + cursor_pos.x, origin.y + cursor_pos.y),
                    size(px(1.5), lh),
                ),
                rgba(palette.active_line_fg.rgba()),
            )
        });
        let mut selections = Vec::new();
        if !selected.is_empty() && !content.is_empty() {
            let color = rgba((palette.window_bg_active.rgba() & 0xffffff00) | 0x40);
            let (a, b) = (pos(selected.start), pos(selected.end));
            let mut y = a.y;
            while y <= b.y {
                let x0 = if y == a.y { a.x } else { px(0.) };
                let x1 = if y == b.y { b.x } else { bounds.size.width };
                if x1 > x0 {
                    selections.push(fill(
                        Bounds::from_corners(
                            point(origin.x + x0, origin.y + y),
                            point(origin.x + x1, origin.y + y + lh),
                        ),
                        color,
                    ));
                }
                y += lh;
            }
        }
        self.input.update(cx, |input, _| input.scroll_y = scroll_y);
        PrepaintState {
            lines,
            tops,
            starts,
            cursor,
            selections,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        let scroll_y = self.input.read(cx).scroll_y;
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.selections.drain(..) {
                window.paint_quad(quad);
            }
            for (line, top) in prepaint.lines.iter().zip(&prepaint.tops) {
                let origin = point(bounds.left(), bounds.top() + *top - scroll_y);
                let _ = line.paint(
                    origin,
                    line_height(),
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                );
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        let lines = std::mem::take(&mut prepaint.lines);
        let tops = std::mem::take(&mut prepaint.tops);
        let starts = std::mem::take(&mut prepaint.starts);
        self.input.update(cx, |input, _cx| {
            input.layout = Layout {
                lines,
                starts,
                tops,
                bounds: Some(bounds),
            };
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
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
