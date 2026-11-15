//! Message text with formatting, laid out and drawn like tdesktop's `Ui::Text::String`
//! (`lib_ui/ui/text`): entities become styled spans, block quotes and code blocks
//! become paragraphs of their own (`BlockParser::ensureAtNewline`), lines wrap at
//! tdesktop's break points and every line is `lineHeight` tall. Drawn by our own
//! element, so selection, link clicks and spoilers work on the same geometry.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Bounds, ContentMask, Corners, CursorStyle, Element, ElementId, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    LineLayout, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    SharedString, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, font, point, px,
    size,
};
use tdr_theme::Color;

use crate::scale;

const FONT_FAMILY: &str = "Open Sans";
const FONT_SIZE: f32 = 13.;

// chat_helpers.style: historyQuoteStyle and historyTextStyle's blockquote / pre.
const QUOTE_VERTICAL_SKIP: f32 = 4.;
const QUOTE_OUTLINE: f32 = 3.;
const QUOTE_RADIUS: f32 = 5.;
/// padding: left, top, right, bottom.
const BLOCKQUOTE_PADDING: [f32; 4] = [10., 2., 20., 2.];
const PRE_PADDING: [f32; 4] = [10., 2., 4., 2.];
const PRE_HEADER: f32 = 20.;
const PRE_HEADER_POSITION: (f32, f32) = (10., 2.);
/// `chat/mini_quote` at (4, 4) and `chat/mini_copy` at (4, 2) from the top right.
const BLOCKQUOTE_ICON_POSITION: (f32, f32) = (4., 4.);
const PRE_ICON_POSITION: (f32, f32) = (4., 2.);
/// `lng_code_block_header_copy`.
const PRE_HEADER_COPY: &str = "copy";

// Span flags (`TextBlockFlag`).
const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const UNDERLINE: u8 = 4;
const STRIKE: u8 = 8;
const MONO: u8 = 16;
const SPOILER: u8 = 32;

/// An entity of the text, as a byte range of it.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEntity {
    pub range: Range<usize>,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Bold,
    Italic,
    Underline,
    Strike,
    Spoiler,
    /// Inline code: a click copies it.
    Code,
    Pre(String),
    Blockquote,
    /// A link to this address (the text itself for plain links).
    Url(String),
    Email,
    Mention,
    MentionName(i64),
    Hashtag,
    Cashtag,
    BotCommand,
    Phone,
    BankCard,
    /// A date the server marked (`messageEntityFormattedDate`).
    Date(i32),
}

/// What a click on a span does (tdesktop's click handlers).
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Url(String),
    Email(String),
    /// `@username` without the `@`.
    Mention(String),
    MentionName(i64),
    Hashtag(String),
    Cashtag(String),
    BotCommand(String),
    Phone(String),
    BankCard(String),
    /// `MonospaceClickHandler`: copies the code.
    Code(String),
    Date(i32),
}

#[derive(Debug, Clone, PartialEq)]
struct Span {
    range: Range<usize>,
    flags: u8,
    link: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
struct Quote {
    pre: bool,
    /// The code block's header: its language or "copy".
    header: String,
    /// What the header copies.
    code: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Paragraph {
    /// Without the line feed.
    range: Range<usize>,
    quote: Option<usize>,
}

/// A text prepared for layout (`Ui::Text::String` after `BlockParser`).
#[derive(Debug, Clone, PartialEq)]
pub struct Rich {
    pub text: String,
    spans: Vec<Span>,
    paragraphs: Vec<Paragraph>,
    quotes: Vec<Quote>,
    links: Vec<Link>,
    pub has_spoilers: bool,
}

fn is_quote(kind: &Kind) -> bool {
    matches!(kind, Kind::Pre(_) | Kind::Blockquote)
}

fn link_of(kind: &Kind, text: &str) -> Option<Link> {
    Some(match kind {
        Kind::Url(url) => Link::Url(url.clone()),
        Kind::Email => Link::Email(text.into()),
        Kind::Mention => Link::Mention(text.trim_start_matches('@').into()),
        Kind::MentionName(id) => Link::MentionName(*id),
        Kind::Hashtag => Link::Hashtag(text.into()),
        Kind::Cashtag => Link::Cashtag(text.into()),
        Kind::BotCommand => Link::BotCommand(text.into()),
        Kind::Phone => Link::Phone(text.into()),
        Kind::BankCard => Link::BankCard(text.into()),
        // tdesktop makes single-line code clickable only (`isSingleLine`).
        Kind::Code if !text.trim().is_empty() && !text.trim().contains('\n') => {
            Link::Code(text.trim().into())
        }
        Kind::Date(date) => Link::Date(*date),
        _ => return None,
    })
}

/// Turns the text and its entities into spans and paragraphs. Block quotes and code
/// blocks start and end on their own lines: a line feed is inserted where the text
/// has none (`ensureAtNewline`, `_newlineAwaited`).
pub fn prepare(text: &str, entities: &[TextEntity]) -> Rich {
    let mut valid: Vec<&TextEntity> = entities
        .iter()
        .filter(|e| {
            e.range.start < e.range.end
                && e.range.end <= text.len()
                && text.is_char_boundary(e.range.start)
                && text.is_char_boundary(e.range.end)
        })
        .collect();
    valid.sort_by_key(|e| (e.range.start, std::cmp::Reverse(e.range.end)));
    // Quotes do not nest, links do not overlap: the first one wins.
    let mut quotes_in: Vec<&TextEntity> = Vec::new();
    let mut links_in: Vec<(&TextEntity, Link)> = Vec::new();
    let mut styles: Vec<(&TextEntity, u8)> = Vec::new();
    for e in valid {
        if is_quote(&e.kind) {
            if quotes_in
                .last()
                .is_none_or(|q| q.range.end <= e.range.start)
            {
                quotes_in.push(e);
            }
            continue;
        }
        let flag = match e.kind {
            Kind::Bold => BOLD,
            Kind::Italic => ITALIC,
            Kind::Underline => UNDERLINE,
            Kind::Strike => STRIKE,
            Kind::Spoiler => SPOILER,
            Kind::Code => MONO,
            _ => 0,
        };
        if flag != 0 {
            styles.push((e, flag));
        }
        if let Some(link) = link_of(&e.kind, &text[e.range.clone()])
            && links_in
                .last()
                .is_none_or(|(l, _)| l.range.end <= e.range.start)
        {
            links_in.push((e, link));
        }
    }

    let mut out = String::with_capacity(text.len() + 4);
    let mut spans: Vec<Span> = Vec::new();
    let mut para_quotes: Vec<Option<usize>> = vec![None];
    let mut quotes: Vec<Quote> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut link_ids: Vec<Option<u16>> = vec![None; links_in.len()];
    // The open quote: its index, where it ends in `text`, where it starts in `out`.
    let mut open: Option<(usize, usize, usize)> = None;
    let mut next_quote = 0;
    let mut awaited = false;

    fn push(out: &mut String, spans: &mut Vec<Span>, ch: char, flags: u8, link: Option<u16>) {
        let start = out.len();
        out.push(ch);
        let end = out.len();
        match spans.last_mut() {
            Some(last) if last.flags == flags && last.link == link && last.range.end == start => {
                last.range.end = end;
            }
            _ => spans.push(Span {
                range: start..end,
                flags,
                link,
            }),
        }
    }
    let close = |out: &String,
                 quotes: &mut Vec<Quote>,
                 para_quotes: &mut Vec<Option<usize>>,
                 awaited: &mut bool,
                 (q, _, from): (usize, usize, usize)| {
        if quotes[q].pre {
            quotes[q].code = out[from..].trim_end_matches('\n').to_string();
        }
        if out.ends_with('\n') {
            *para_quotes.last_mut().unwrap() = None;
        } else {
            *awaited = true;
        }
    };

    let mut chars = text.char_indices().peekable();
    loop {
        let i = chars.peek().map_or(text.len(), |(i, _)| *i);
        if let Some(o) = open
            && o.1 <= i
        {
            close(&out, &mut quotes, &mut para_quotes, &mut awaited, o);
            open = None;
        }
        let Some((i, ch)) = chars.next() else { break };
        if let Some(q) = quotes_in.get(next_quote).filter(|q| q.range.start == i) {
            next_quote += 1;
            awaited = false;
            if !out.is_empty() && !out.ends_with('\n') {
                push(&mut out, &mut spans, '\n', 0, None);
                para_quotes.push(None);
            }
            let (pre, header) = match &q.kind {
                Kind::Pre(language) if !language.is_empty() => (true, language.clone()),
                Kind::Pre(_) => (true, PRE_HEADER_COPY.to_string()),
                _ => (false, String::new()),
            };
            quotes.push(Quote {
                pre,
                header,
                code: String::new(),
            });
            *para_quotes.last_mut().unwrap() = Some(quotes.len() - 1);
            open = Some((quotes.len() - 1, q.range.end, out.len()));
        }
        if awaited {
            awaited = false;
            if ch != '\n' {
                push(&mut out, &mut spans, '\n', 0, None);
                para_quotes.push(None);
            }
        }
        let mut flags = if open.is_some_and(|(q, _, _)| quotes[q].pre) {
            MONO
        } else {
            0
        };
        for (e, flag) in &styles {
            if e.range.start <= i && i < e.range.end {
                flags |= flag;
            }
        }
        let mut link = None;
        for (k, (e, l)) in links_in.iter().enumerate() {
            if e.range.start <= i && i < e.range.end {
                let id = *link_ids[k].get_or_insert_with(|| {
                    links.push(l.clone());
                    (links.len() - 1) as u16
                });
                link = Some(id);
                break;
            }
        }
        if ch == '\n' {
            push(&mut out, &mut spans, '\n', 0, None);
            para_quotes.push(open.map(|(q, _, _)| q));
        } else {
            push(&mut out, &mut spans, ch, flags, link);
        }
    }

    let mut paragraphs = Vec::with_capacity(para_quotes.len());
    let mut start = 0;
    for (k, line) in out.split('\n').enumerate() {
        paragraphs.push(Paragraph {
            range: start..start + line.len(),
            quote: para_quotes.get(k).copied().flatten(),
        });
        start += line.len() + 1;
    }
    let has_spoilers = spans.iter().any(|s| s.flags & SPOILER != 0);
    Rich {
        text: out,
        spans,
        paragraphs,
        quotes,
        links,
        has_spoilers,
    }
}

impl Rich {
    pub fn link(&self, id: u16) -> Option<&Link> {
        self.links.get(id as usize)
    }

    /// The code a code block's header copies.
    pub fn quote_code(&self, quote: usize) -> Option<&str> {
        self.quotes.get(quote).map(|q| q.code.as_str())
    }

    fn span_at(&self, index: usize) -> Option<&Span> {
        let k = self.spans.partition_point(|s| s.range.end <= index);
        self.spans.get(k).filter(|s| s.range.start <= index)
    }

    /// The selected text, for the clipboard.
    pub fn selected_text(&self, range: Range<usize>) -> String {
        let start = floor_boundary(&self.text, range.start.min(self.text.len()));
        let end = floor_boundary(&self.text, range.end.min(self.text.len()));
        self.text[start..end].to_string()
    }

    /// The word around `index` (a double click selects it).
    pub fn word_at(&self, index: usize) -> Range<usize> {
        use unicode_segmentation::UnicodeSegmentation;
        for (start, word) in self.text.split_word_bound_indices() {
            let end = start + word.len();
            if index < end || end == self.text.len() {
                if word.trim().is_empty() && word.contains('\n') {
                    return index..index;
                }
                return start..end;
            }
        }
        index..index
    }
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The monospace family tdesktop picks on Windows (`ManualMonospaceFont`).
fn mono_family(window: &Window) -> SharedString {
    static FAMILY: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| {
            let names = window.text_system().all_font_names();
            [
                "Cascadia Mono",
                "Consolas",
                "Liberation Mono",
                "Menlo",
                "Courier",
            ]
            .into_iter()
            .find(|f| names.iter().any(|n| n == f))
            .unwrap_or("Consolas")
            .into()
        })
        .clone()
}

fn run_font(flags: u8, mono: &SharedString) -> gpui::Font {
    if flags & MONO != 0 {
        return font(mono.clone());
    }
    let mut f = font(FONT_FAMILY);
    if flags & BOLD != 0 {
        f.weight = FontWeight::SEMIBOLD;
    }
    if flags & ITALIC != 0 {
        f.style = FontStyle::Italic;
    }
    f
}

fn quote_padding(quote: &Quote) -> [f32; 4] {
    // `String::quotePadding`: the style's padding plus the header and the skips.
    let [l, t, r, b] = if quote.pre {
        PRE_PADDING
    } else {
        BLOCKQUOTE_PADDING
    };
    let header = if quote.pre { PRE_HEADER } else { 0. };
    [
        scale::lg(l),
        scale::lg(t) + scale::lg(header) + scale::lg(QUOTE_VERTICAL_SKIP),
        scale::lg(r),
        scale::lg(b) + scale::lg(QUOTE_VERTICAL_SKIP),
    ]
}

/// Icon sizes in style pixels (`chat/mini_quote` 16x12, `chat/mini_copy` 16x16).
fn quote_icon_size(pre: bool) -> (f32, f32) {
    if pre { (16., 16.) } else { (16., 12.) }
}

/// `String::quoteMinWidth`: room for the header and the icon.
fn quote_min_width(quote: &Quote, mono_header_width: f32) -> f32 {
    let [l, _, r, _] = if quote.pre {
        PRE_PADDING
    } else {
        BLOCKQUOTE_PADDING
    };
    let (iw, ih) = quote_icon_size(quote.pre);
    let position = if quote.pre {
        PRE_ICON_POSITION
    } else {
        BLOCKQUOTE_ICON_POSITION
    };
    let header = if quote.pre { PRE_HEADER } else { 0. };
    let iconsize = scale::lg(iw + position.0).max(scale::lg(ih + position.1));
    let corner = scale::lg(header)
        .max(scale::lg(QUOTE_RADIUS))
        .max(scale::lg(QUOTE_OUTLINE))
        .max(iconsize);
    let top = scale::lg(l)
        + if quote.pre {
            mono_header_width + scale::lg(PRE_HEADER_POSITION.0)
        } else {
            0.
        }
        + scale::lg(r).max(scale::lg(position.0) + scale::lg(iw));
    top.max(2. * corner)
}

/// A paragraph shaped as one line: widths and positions for wrapping and hit tests.
struct ShapedParagraph {
    layout: Arc<LineLayout>,
    /// Allowed line starts (byte offsets in the paragraph), ascending, with its end.
    breaks: Vec<usize>,
}

impl ShapedParagraph {
    fn x(&self, index: usize) -> f32 {
        f32::from(self.layout.x_for_index(index))
    }
}

/// The text measured once (fonts and break points); laid out at any width.
pub struct Measured {
    pub rich: Rc<Rich>,
    paragraphs: Vec<ShapedParagraph>,
    /// Natural width of each paragraph (trailing spaces left out).
    natural: Vec<f32>,
    /// `quote->maxWidth`: the widest paragraph of each quote plus its minimum width.
    quote_max: Vec<f32>,
    quote_min: Vec<f32>,
}

fn runs_for(rich: &Rich, range: Range<usize>, mono: &SharedString) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut k = rich.spans.partition_point(|s| s.range.end <= range.start);
    let mut at = range.start;
    while at < range.end {
        let (end, flags) = match rich.spans.get(k) {
            Some(s) if s.range.start <= at => (s.range.end.min(range.end), s.flags),
            _ => (range.end, 0),
        };
        runs.push(TextRun {
            len: end - at,
            font: run_font(flags, mono),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        at = end;
        k += 1;
    }
    runs
}

fn trim_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let ch = text[..end].chars().next_back().unwrap();
        if ch == ' ' || ch == '\t' || ch == '\u{3000}' {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// tdesktop's break points: Unicode line breaks, except after '/' or '.' in the
/// middle of a word (`WordParser::isLineBreak`), never at a no-break space.
fn break_points(text: &str) -> Vec<usize> {
    unicode_linebreak::linebreaks(text)
        .filter(|(i, _)| {
            *i == text.len() || {
                let prev = text[..*i].chars().next_back();
                !matches!(prev, Some('/') | Some('.'))
            }
        })
        .map(|(i, _)| i)
        .collect()
}

impl Measured {
    pub fn new(rich: Rc<Rich>, window: &Window) -> Self {
        let mono = mono_family(window);
        let size = scale::font(FONT_SIZE).size;
        let text_system = window.text_system();
        let mut paragraphs = Vec::with_capacity(rich.paragraphs.len());
        let mut natural = Vec::with_capacity(rich.paragraphs.len());
        for p in &rich.paragraphs {
            let text = &rich.text[p.range.clone()];
            let runs = runs_for(&rich, p.range.clone(), &mono);
            let layout = text_system.layout_line(text, size, &runs, None);
            let shaped = ShapedParagraph {
                layout,
                breaks: break_points(text),
            };
            natural.push(shaped.x(trim_end(text, 0, text.len())));
            paragraphs.push(shaped);
        }
        let mono_font = font(mono.clone());
        let mono_header_widths: Vec<f32> = rich
            .quotes
            .iter()
            .map(|q| {
                if !q.pre {
                    return 0.;
                }
                let run = TextRun {
                    len: q.header.len(),
                    font: mono_font.clone(),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                f32::from(text_system.layout_line(&q.header, size, &[run], None).width)
            })
            .collect();
        let quote_min: Vec<f32> = rich
            .quotes
            .iter()
            .zip(&mono_header_widths)
            .map(|(q, w)| quote_min_width(q, *w))
            .collect();
        let mut quote_max = quote_min.clone();
        for (p, w) in rich.paragraphs.iter().zip(&natural) {
            if let Some(q) = p.quote {
                quote_max[q] = quote_max[q].max(quote_min[q] + w);
            }
        }
        Self {
            rich,
            paragraphs,
            natural,
            quote_max,
            quote_min,
        }
    }

    fn ends_with_quote(&self) -> bool {
        self.rich
            .paragraphs
            .last()
            .is_some_and(|p| p.quote.is_some())
    }

    /// `String::maxWidth` with the date's skip block at the end.
    pub fn natural_width(&self, skip_width: f32) -> f32 {
        let mut max: f32 = 0.;
        let last = self.rich.paragraphs.len().saturating_sub(1);
        for (k, (p, w)) in self.rich.paragraphs.iter().zip(&self.natural).enumerate() {
            let mut width = w + p.quote.map_or(0., |q| self.quote_min[q]);
            if k == last && p.quote.is_none() {
                width += skip_width;
            }
            max = max.max(width);
        }
        if self.ends_with_quote() {
            max = max.max(skip_width);
        }
        max
    }

    /// Lines at `width` (`Renderer::enumerate`), with the skip block placed after
    /// the last line or on a line of its own.
    pub fn layout(self: &Rc<Self>, width: f32, skip: (f32, f32)) -> Layout {
        let line_height = scale::font(FONT_SIZE).height;
        let mut lines = Vec::new();
        let mut boxes = Vec::new();
        let mut y = 0.;
        let mut current: Option<usize> = None;
        let mut quote_top = 0.;
        let mut box_width = 0.;
        let skip_v = scale::lg(QUOTE_VERTICAL_SKIP);
        let close =
            |q: usize, y: &mut f32, quote_top: f32, box_width: f32, boxes: &mut Vec<QuoteBox>| {
                *y += quote_padding(&self.rich.quotes[q])[3];
                boxes.push(QuoteBox {
                    quote: q,
                    bounds: Bounds::new(
                        point(px(0.), px(quote_top + skip_v)),
                        size(px(box_width), px(*y - skip_v - (quote_top + skip_v))),
                    ),
                });
            };
        for (k, p) in self.rich.paragraphs.iter().enumerate() {
            if p.quote != current {
                if let Some(q) = current {
                    close(q, &mut y, quote_top, box_width, &mut boxes);
                }
                current = p.quote;
                if let Some(q) = p.quote {
                    quote_top = y;
                    y += quote_padding(&self.rich.quotes[q])[1];
                    box_width = width.min(self.quote_max[q]);
                }
            }
            let (x0, avail) = match p.quote {
                Some(q) => {
                    let [l, _, r, _] = quote_padding(&self.rich.quotes[q]);
                    (l, (box_width - l - r).max(1.))
                }
                None => (0., width),
            };
            let shaped = &self.paragraphs[k];
            let text = &self.rich.text[p.range.clone()];
            for range in wrap(text, shaped, avail) {
                let line_width = shaped.x(range.end) - shaped.x(range.start);
                lines.push(Line {
                    paragraph: k,
                    range: p.range.start + range.start..p.range.start + range.end,
                    x: x0,
                    y,
                    width: line_width,
                    avail,
                    quote: p.quote,
                });
                y += line_height;
            }
        }
        if let Some(q) = current {
            close(q, &mut y, quote_top, box_width, &mut boxes);
        }
        let (skip_w, skip_h) = skip;
        let skip_inline = !self.ends_with_quote()
            && lines
                .last()
                .is_some_and(|l| l.quote.is_none() && l.width + skip_w <= width);
        if !skip_inline {
            y += skip_h;
        }
        Layout {
            measured: self.clone(),
            width,
            height: y,
            lines,
            boxes,
            line_height,
        }
    }
}

/// Greedy wrapping at the break points; a word longer than the line is cut at
/// grapheme boundaries (tdesktop's "unfinished" words).
fn wrap(text: &str, shaped: &ShapedParagraph, avail: f32) -> Vec<Range<usize>> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut out = Vec::new();
    if text.is_empty() {
        out.push(0..0);
        return out;
    }
    let mut start = 0;
    while start < text.len() {
        let mut fit = None;
        for &b in shaped.breaks.iter().filter(|b| **b > start) {
            let end = trim_end(text, start, b);
            if shaped.x(end) - shaped.x(start) <= avail + 0.01 || end == start {
                fit = Some(b);
            } else {
                break;
            }
        }
        match fit {
            Some(b) => {
                out.push(start..trim_end(text, start, b));
                start = b;
            }
            None => {
                let mut end = start;
                for (i, g) in text[start..].grapheme_indices(true) {
                    let next = start + i + g.len();
                    if end > start && shaped.x(next) - shaped.x(start) > avail + 0.01 {
                        break;
                    }
                    end = next;
                }
                out.push(start..end);
                start = end;
            }
        }
    }
    out
}

pub struct Line {
    paragraph: usize,
    /// Bytes of `Rich::text`, trailing spaces left out.
    pub range: Range<usize>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// The width the line may take (a selection to its end fills it).
    pub avail: f32,
    quote: Option<usize>,
}

pub struct QuoteBox {
    quote: usize,
    bounds: Bounds<Pixels>,
}

/// The text laid out at one width.
pub struct Layout {
    measured: Rc<Measured>,
    pub width: f32,
    pub height: f32,
    pub lines: Vec<Line>,
    boxes: Vec<QuoteBox>,
    line_height: f32,
}

impl Layout {
    /// Hidden text: one rectangle per line and selection state, relative to the
    /// text's top left (`pushSpoilerRange`, `fillRectsFromRanges`).
    pub fn spoiler_rects(&self, selection: Option<&Range<usize>>) -> Vec<(Bounds<Pixels>, bool)> {
        let rich = self.rich();
        let mut out: Vec<(Bounds<Pixels>, bool)> = Vec::new();
        for line in &self.lines {
            let mut cuts = vec![line.range.start, line.range.end];
            if let Some(s) = selection {
                cuts.extend(
                    [s.start, s.end]
                        .into_iter()
                        .filter(|c| line.range.contains(c)),
                );
            }
            for s in &rich.spans {
                for c in [s.range.start, s.range.end] {
                    if line.range.contains(&c) {
                        cuts.push(c);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a >= b || rich.span_at(a).is_none_or(|s| s.flags & SPOILER == 0) {
                    continue;
                }
                let selected = selection.is_some_and(|s| s.start <= a && b <= s.end);
                let rect = Bounds::from_corners(
                    point(px(line.x + self.line_x(line, a)), px(line.y)),
                    point(
                        px(line.x + self.line_x(line, b)),
                        px(line.y + self.line_height),
                    ),
                );
                match out.last_mut() {
                    Some((last, sel))
                        if *sel == selected
                            && last.origin.y == rect.origin.y
                            && (last.right() - rect.origin.x).abs() < px(0.5) =>
                    {
                        last.size.width = rect.right() - last.origin.x;
                    }
                    _ => out.push((rect, selected)),
                }
            }
        }
        out
    }
}

/// What is under the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hit {
    /// The nearest position between characters (for selection).
    pub index: usize,
    /// Over a character (not beside the text).
    pub over_text: bool,
    pub link: Option<u16>,
    pub spoiler: bool,
    /// Over a code block's header: copies its code.
    pub pre_header: Option<usize>,
}

impl Layout {
    pub fn rich(&self) -> &Rich {
        &self.measured.rich
    }

    fn line_x(&self, line: &Line, index: usize) -> f32 {
        let shaped = &self.measured.paragraphs[line.paragraph];
        let start = self.rich().paragraphs[line.paragraph].range.start;
        shaped.x(index - start) - shaped.x(line.range.start - start)
    }

    /// Hit test at `p`, relative to the text's top left.
    pub fn hit(&self, p: Point<f32>) -> Hit {
        let rich = self.rich();
        let mut hit = Hit::default();
        for b in &self.boxes {
            let q = &rich.quotes[b.quote];
            let o = b.bounds.origin;
            if q.pre
                && p.x >= f32::from(o.x)
                && p.x < f32::from(o.x + b.bounds.size.width)
                && p.y >= f32::from(o.y)
                && p.y < f32::from(o.y) + scale::lg(PRE_HEADER)
            {
                hit.pre_header = Some(b.quote);
            }
        }
        let Some(first) = self.lines.first() else {
            return hit;
        };
        if p.y < first.y {
            hit.index = first.range.start;
            return hit;
        }
        // The line at that height, or the last one above it (between quote lines).
        let k = self.lines.partition_point(|l| l.y <= p.y).saturating_sub(1);
        let line = &self.lines[k];
        if p.y >= line.y + self.line_height && k + 1 == self.lines.len() {
            hit.index = rich.text.len();
            return hit;
        }
        let local = p.x - line.x;
        let inside_y = p.y < line.y + self.line_height;
        let shaped = &self.measured.paragraphs[line.paragraph];
        let pstart = rich.paragraphs[line.paragraph].range.start;
        let base = shaped.x(line.range.start - pstart);
        if local <= 0. {
            hit.index = line.range.start;
        } else if local >= line.width {
            hit.index = line.range.end;
        } else {
            let ix = shaped.layout.closest_index_for_x(px(base + local)) + pstart;
            hit.index = ix.clamp(line.range.start, line.range.end);
            if inside_y
                && let Some(under) = shaped.layout.index_for_x(px(base + local))
                && let Some(span) = rich.span_at(under + pstart)
            {
                hit.over_text = true;
                hit.link = span.link;
                hit.spoiler = span.flags & SPOILER != 0;
            }
        }
        hit
    }
}

/// Colors of a message's text (`TextPalette`, quote and pre caches).
#[derive(Clone, Copy, Debug)]
pub struct TextColors {
    pub fg: Color,
    pub link: Color,
    pub mono: Color,
    pub select_bg: Color,
    pub select_fg: Color,
    pub select_link: Color,
    pub select_mono: Color,
    pub spoiler: Color,
    pub select_spoiler: Color,
    /// The block quote's color (`ColorIndexValues::name`): outline, background and
    /// icon derive from it, links inside take it.
    pub quote: Color,
    /// Dark themes draw code blocks on black (`preBgOverride`).
    pub dark: bool,
}

fn hsla(color: Color) -> Hsla {
    gpui::rgba(color.rgba()).into()
}

fn with_alpha(color: Color, alpha: f32) -> Hsla {
    let mut c = hsla(color);
    c.a = alpha;
    c
}

/// The state the view keeps for one text.
#[derive(Clone, Debug, Default)]
pub struct TextState {
    pub selection: Option<Range<usize>>,
    pub hovered_link: Option<u16>,
    pub pressed_link: Option<u16>,
    /// When the spoilers were revealed (a click on one shows all, 200 ms fade).
    pub revealed: Option<Instant>,
}

/// Spoiler reveal fade (`st::fadeWrapDuration`).
const SPOILER_REVEAL_MS: f32 = 200.;

/// Mouse input over a text, for the view to act on.
pub enum TextEvent {
    Down {
        hit: Hit,
        click_count: usize,
        shift: bool,
    },
    /// The mouse moved over the text (or anywhere while it is being dragged).
    Move {
        hit: Hit,
        inside: bool,
        pressed: bool,
    },
    Up {
        hit: Hit,
        inside: bool,
    },
    /// The left button went down outside this text.
    DownOutside,
}

type Handler = Rc<dyn Fn(TextEvent, &mut Window, &mut App)>;

/// The text element.
pub struct RichText {
    id: ElementId,
    layout: Rc<Layout>,
    colors: TextColors,
    state: TextState,
    /// This text is being dragged over (selection): moves outside it still count.
    dragging: bool,
    handler: Handler,
}

impl RichText {
    pub fn new(
        id: impl Into<ElementId>,
        layout: Rc<Layout>,
        colors: TextColors,
        state: TextState,
        dragging: bool,
        handler: impl Fn(TextEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            layout,
            colors,
            state,
            dragging,
            handler: Rc::new(handler),
        }
    }
}

impl IntoElement for RichText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RichText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = px(self.layout.width).into();
        style.size.height = px(self.layout.height).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = bounds.origin;
        let layout = self.layout.clone();
        let local =
            move |p: Point<Pixels>| point(f32::from(p.x - origin.x), f32::from(p.y - origin.y));

        // The cursor: a hand over links, spoilers and code headers, a beam over text.
        if hitbox.is_hovered(window) {
            let hit = layout.hit(local(window.mouse_position()));
            let hidden_spoiler = hit.spoiler && self.reveal_progress() < 1.;
            let cursor = if hit.link.is_some() || hidden_spoiler || hit.pre_header.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            };
            window.set_cursor_style(cursor, hitbox);
        } else if self.dragging {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
        }

        self.paint_quotes(bounds, window, cx);
        self.paint_lines(bounds, window, cx);

        let handler = self.handler.clone();
        let hb = hitbox.clone();
        let l = layout.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() || e.button != MouseButton::Left {
                return;
            }
            if hb.is_hovered(window) {
                let hit = l.hit(local(e.position));
                handler(
                    TextEvent::Down {
                        hit,
                        click_count: e.click_count,
                        shift: e.modifiers.shift,
                    },
                    window,
                    cx,
                );
            } else {
                handler(TextEvent::DownOutside, window, cx);
            }
        });
        let handler = self.handler.clone();
        let hb = hitbox.clone();
        let l = layout.clone();
        let dragging = self.dragging;
        let hovered_link = self.state.hovered_link;
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            let inside = hb.is_hovered(window);
            if !inside && !dragging && hovered_link.is_none() {
                return;
            }
            let hit = l.hit(local(e.position));
            handler(
                TextEvent::Move {
                    hit,
                    inside,
                    pressed: e.pressed_button == Some(MouseButton::Left),
                },
                window,
                cx,
            );
        });
        let handler = self.handler.clone();
        let hb = hitbox.clone();
        let l = layout;
        let pressed = self.state.pressed_link.is_some();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if !phase.bubble() || e.button != MouseButton::Left {
                return;
            }
            let inside = hb.is_hovered(window);
            if !inside && !dragging && !pressed {
                return;
            }
            let hit = l.hit(local(e.position));
            handler(TextEvent::Up { hit, inside }, window, cx);
        });
    }
}

impl RichText {
    fn reveal_progress(&self) -> f32 {
        self.state.revealed.map_or(0., |t| {
            (t.elapsed().as_secs_f32() * 1000. / SPOILER_REVEAL_MS).min(1.)
        })
    }

    /// Block quotes and code blocks (`FillQuotePaint`): a rounded box, the outline
    /// on its left, the header of a code block, the icon in the top right corner.
    fn paint_quotes(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let rich = self.layout.rich();
        let colors = &self.colors;
        let radius = px(scale::lg(QUOTE_RADIUS));
        let outline = px(scale::lg(QUOTE_OUTLINE));
        for b in &self.layout.boxes {
            let q = &rich.quotes[b.quote];
            let r = Bounds::new(bounds.origin + b.bounds.origin, b.bounds.size);
            // `EnsurePreCache` / `EnsureBlockquoteCache` with the default opacities.
            let (bg, line, header, icon) = if q.pre {
                let bg = if colors.dark {
                    gpui::hsla(0., 0., 0., 192. / 255.)
                } else {
                    with_alpha(colors.mono, 0.12)
                };
                (
                    bg,
                    with_alpha(colors.mono, 0.9),
                    with_alpha(colors.mono, 0.3),
                    with_alpha(colors.mono, 0.9 * 0.6),
                )
            } else {
                let c = hsla(colors.quote);
                (
                    with_alpha(colors.quote, 0.12),
                    with_alpha(colors.quote, 0.9),
                    c,
                    c,
                )
            };
            let rounded = |color: Hsla| gpui::fill(r, color).corner_radii(Corners::all(radius));
            let header_h = if q.pre {
                px(scale::lg(PRE_HEADER))
            } else {
                px(0.)
            };
            // Body: right of the outline, below the header.
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::from_corners(
                        point(r.origin.x + outline, r.origin.y + header_h),
                        r.bottom_right(),
                    ),
                }),
                |window| window.paint_quad(rounded(bg)),
            );
            if q.pre {
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: Bounds::from_corners(
                            point(r.origin.x + outline, r.origin.y),
                            point(r.right(), r.origin.y + header_h),
                        ),
                    }),
                    |window| window.paint_quad(rounded(header)),
                );
            }
            window.with_content_mask(
                Some(ContentMask {
                    bounds: Bounds::from_corners(r.origin, point(r.origin.x + outline, r.bottom())),
                }),
                |window| window.paint_quad(rounded(line)),
            );
            // The icon.
            let (kind, position) = if q.pre {
                (crate::icons::Icon::MiniCopy, PRE_ICON_POSITION)
            } else {
                (crate::icons::Icon::MiniQuote, BLOCKQUOTE_ICON_POSITION)
            };
            let icon_color = Color(color_to_u32(icon));
            if let Some((image, w, h, pad)) =
                crate::icons::tinted_image(kind, icon_color, window.scale_factor())
            {
                let left = r.right() - px(w) - px(scale::lg(position.0));
                let top = r.origin.y + px(scale::lg(position.1));
                let image_bounds = Bounds::new(
                    point(left - px(pad), top - px(pad)),
                    size(px(w + 2. * pad), px(h + 2. * pad)),
                );
                let _ = window.paint_image(image_bounds, Corners::default(), image, 0, false);
            }
            // The header: the language or "copy" (`fillParagraphBg`).
            if q.pre {
                let mono = mono_family(window);
                let font_size = scale::font(FONT_SIZE).size;
                let run = TextRun {
                    len: q.header.len(),
                    font: font(mono),
                    color: hsla(colors.mono),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    q.header.clone().into(),
                    font_size,
                    &[run],
                    None,
                );
                let lh = px(scale::font(FONT_SIZE).height);
                let ascent = shaped.ascent;
                let descent = shaped.descent;
                let baseline = r.origin.y + px(scale::lg(PRE_HEADER_POSITION.1)) + ascent;
                let top = baseline - (lh + ascent - descent) / 2.;
                let _ = shaped.paint(
                    point(r.origin.x + px(scale::lg(PRE_HEADER_POSITION.0)), top),
                    lh,
                    window,
                    cx,
                );
            }
        }
    }

    fn paint_lines(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let rich = self.layout.rich();
        let colors = self.colors;
        let mono = mono_family(window);
        let f = scale::font(FONT_SIZE);
        let lh = px(f.height);
        let reveal = if rich.has_spoilers {
            self.reveal_progress()
        } else {
            1.
        };
        let selection = self.state.selection.clone().filter(|s| s.start < s.end);
        let text_len = rich.text.len();
        let content = window.content_mask().bounds;
        for line in &self.layout.lines {
            let top = bounds.origin.y + px(line.y);
            if top > content.bottom() || top + lh < content.origin.y {
                continue;
            }
            let left = bounds.origin.x + px(line.x);
            let range = line.range.clone();
            // Runs split at spans and the selection.
            let mut cuts = vec![range.start, range.end];
            if let Some(s) = &selection {
                cuts.extend([s.start, s.end].into_iter().filter(|c| range.contains(c)));
            }
            for s in &rich.spans {
                if s.range.start > range.start && s.range.start < range.end {
                    cuts.push(s.range.start);
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            let mut runs = Vec::new();
            for w in cuts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a >= b {
                    continue;
                }
                let span = rich.span_at(a);
                let flags = span.map_or(0, |s| s.flags);
                let link = span.and_then(|s| s.link);
                let selected = selection
                    .as_ref()
                    .is_some_and(|s| s.start <= a && b <= s.end);
                let is_mono = flags & MONO != 0;
                let in_quote_link =
                    link.is_some() && line.quote.is_some_and(|q| !rich.quotes[q].pre);
                let color = match (selected, is_mono, link.is_some()) {
                    (false, true, _) => hsla(colors.mono),
                    (true, true, _) => hsla(colors.select_mono),
                    (_, false, true) if in_quote_link => hsla(colors.quote),
                    (false, false, true) => hsla(colors.link),
                    (true, false, true) => hsla(colors.select_link),
                    (false, false, false) => hsla(colors.fg),
                    (true, false, false) => hsla(colors.select_fg),
                };
                let spoiler = flags & SPOILER != 0 && reveal < 1.;
                let color = if spoiler {
                    let mut c = color;
                    c.a *= reveal;
                    c
                } else {
                    color
                };
                let underline_on = !is_mono
                    && (flags & UNDERLINE != 0
                        || (link.is_some() && self.state.hovered_link == link));
                let thickness = px(scale::lg(1.));
                runs.push(TextRun {
                    len: b - a,
                    font: run_font(flags, &mono),
                    color,
                    background_color: None,
                    underline: underline_on.then_some(UnderlineStyle {
                        thickness,
                        color: Some(color),
                        wavy: false,
                    }),
                    strikethrough: (!is_mono && flags & STRIKE != 0).then_some(
                        StrikethroughStyle {
                            thickness,
                            color: Some(color),
                        },
                    ),
                });
            }
            let text: SharedString = rich.text[range.clone()].to_string().into();
            let shaped = if text.is_empty() {
                None
            } else {
                Some(window.text_system().shape_line(text, f.size, &runs, None))
            };
            let x_of = |i: usize| -> Pixels {
                shaped
                    .as_ref()
                    .map_or(px(0.), |s| s.x_for_index(i - range.start))
            };
            // Selection: under the selected characters, to the line's end when it goes
            // on, from its start when it came from above (`fullWidthSelection`).
            if let Some(s) = &selection {
                let from = s.start.max(range.start);
                let till = s.end.min(range.end);
                let till_end = s.end > range.end && range.end < text_len && s.start <= range.end;
                let span = if till_end {
                    Some((x_of(from.min(range.end)), px(line.avail)))
                } else if from < till {
                    Some((x_of(from), x_of(till)))
                } else {
                    None
                };
                if let Some((a, b)) = span
                    && b > a
                {
                    window.paint_quad(gpui::fill(
                        Bounds::from_corners(point(left + a, top), point(left + b, top + lh)),
                        hsla(colors.select_bg),
                    ));
                }
            }
            if let Some(shaped) = &shaped {
                let ascent = shaped.ascent;
                let descent = shaped.descent;
                let baseline = top + px(f.ascent);
                let origin_y = baseline - (lh + ascent - descent) / 2.;
                let _ = shaped.paint(point(left, origin_y), lh, window, cx);
            }
        }
        if rich.has_spoilers && reveal < 1. {
            window.request_animation_frame();
        }
    }
}

/// The text with its spoilers: the particles are a layer above it that fades out
/// while the hidden text fades in (`_spoilerOpacity`).
pub fn view(
    id: impl Into<ElementId>,
    layout: Rc<Layout>,
    colors: TextColors,
    state: TextState,
    dragging: bool,
    handler: impl Fn(TextEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    use gpui::{ParentElement, Styled};
    let (w, h) = (layout.width, layout.height);
    let reveal = state.revealed.map_or(0., |t| {
        (t.elapsed().as_secs_f32() * 1000. / SPOILER_REVEAL_MS).min(1.)
    });
    let spoilers = (layout.rich().has_spoilers && reveal < 1.)
        .then(|| layout.spoiler_rects(state.selection.as_ref()))
        .filter(|r| !r.is_empty());
    let text = RichText::new(id, layout, colors, state, dragging, handler);
    let mut root = gpui::div()
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .child(text);
    if let Some(rects) = spoilers {
        let (normal, selected) = (colors.spoiler, colors.select_spoiler);
        root = root.child(
            gpui::div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(w))
                .h(px(h))
                .opacity(1. - reveal)
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let frame = crate::spoiler::current_frame();
                            for (r, sel) in &rects {
                                let rect = Bounds::new(bounds.origin + r.origin, r.size);
                                let color = if *sel { selected } else { normal };
                                crate::spoiler::fill(rect, bounds.origin, color, frame, window);
                            }
                            window.request_animation_frame();
                        },
                    )
                    .w(px(w))
                    .h(px(h)),
                ),
        );
    }
    root.into_any_element()
}

fn color_to_u32(c: Hsla) -> u32 {
    let rgba = gpui::Rgba::from(c);
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (b(rgba.r) << 24) | (b(rgba.g) << 16) | (b(rgba.b) << 8) | b(rgba.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(range: Range<usize>, kind: Kind) -> TextEntity {
        TextEntity { range, kind }
    }

    #[test]
    fn quotes_get_their_own_paragraphs() {
        // "ab" + quote "cd" in the middle of a line + "ef".
        let r = prepare("abcdef", &[e(2..4, Kind::Blockquote)]);
        assert_eq!(r.text, "ab\ncd\nef");
        let quotes: Vec<_> = r.paragraphs.iter().map(|p| p.quote).collect();
        assert_eq!(quotes, [None, Some(0), None]);
        // Line feeds already there are kept, none added.
        let r = prepare("ab\ncd\nef", &[e(3..5, Kind::Blockquote)]);
        assert_eq!(r.text, "ab\ncd\nef");
        let quotes: Vec<_> = r.paragraphs.iter().map(|p| p.quote).collect();
        assert_eq!(quotes, [None, Some(0), None]);
        // A quote that takes the line feed after it ends there.
        let r = prepare("ab\ncd\nef", &[e(3..6, Kind::Blockquote)]);
        assert_eq!(r.text, "ab\ncd\nef");
        let quotes: Vec<_> = r.paragraphs.iter().map(|p| p.quote).collect();
        assert_eq!(quotes, [None, Some(0), None]);
        // Several lines in one quote.
        let r = prepare("x\ny\nz", &[e(0..3, Kind::Blockquote)]);
        let quotes: Vec<_> = r.paragraphs.iter().map(|p| p.quote).collect();
        assert_eq!(quotes, [Some(0), Some(0), None]);
    }

    #[test]
    fn code_blocks_keep_their_code() {
        let r = prepare(
            "see\nfn main() {}\nok",
            &[e(4..16, Kind::Pre("rust".into()))],
        );
        assert_eq!(r.quotes[0].header, "rust");
        assert_eq!(r.quotes[0].code, "fn main() {}");
        assert!(r.spans.iter().any(|s| s.flags & MONO != 0));
        let r = prepare("x", &[e(0..1, Kind::Pre(String::new()))]);
        assert_eq!(r.quotes[0].header, "copy");
    }

    #[test]
    fn spans_and_links() {
        let r = prepare(
            "bold link code",
            &[
                e(0..4, Kind::Bold),
                e(5..9, Kind::Url("https://a.b".into())),
                e(10..14, Kind::Code),
                e(0..2, Kind::Italic),
            ],
        );
        assert_eq!(r.span_at(0).unwrap().flags, BOLD | ITALIC);
        assert_eq!(r.span_at(2).unwrap().flags, BOLD);
        let link = r.span_at(6).unwrap().link.unwrap();
        assert_eq!(r.link(link), Some(&Link::Url("https://a.b".into())));
        let code = r.span_at(11).unwrap();
        assert_eq!(code.flags, MONO);
        assert_eq!(r.link(code.link.unwrap()), Some(&Link::Code("code".into())));
        // Overlapping links: the first wins.
        let r = prepare(
            "abcd",
            &[e(0..3, Kind::Mention), e(1..4, Kind::Url("u".into()))],
        );
        assert_eq!(r.links, [Link::Mention("abc".into())]);
    }

    #[test]
    fn breaks_skip_slashes_and_dots() {
        let b = break_points("a b/c.d e");
        // After "a " and "e" end; not after '/' or '.'.
        assert!(b.contains(&2));
        assert!(!b.contains(&4));
        assert!(!b.contains(&6));
        assert!(b.contains(&9));
    }
}
