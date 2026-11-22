//! Markdown in a sent message, as tdesktop applies it when the message goes
//! (`InputField::getTextWithAppliedMarkdown` with `MarkdownTagAccumulator`):
//! `**bold**`, `__italic__`, `~~strike~~`, `` `code` ``, ```` ```pre``` ````,
//! `||spoiler||`. A marker counts only between separators (not inside a word),
//! a code block's markers stand at a line's start, inline code ends at a line's
//! end, a tag inside an applied one stays text, an unclosed one stays text, one
//! cutting a link stays text, and text already formatted in the field takes no
//! markdown.

use std::ops::Range;

use crate::text_input::{Tag, TagSpan};

/// The text with the markdown applied: its tags (byte ranges) and the languages
/// of its code blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    pub tags: Vec<TagSpan>,
    pub languages: Vec<(Range<usize>, String)>,
}

/// `TagStartExpression`: the marker and the characters allowed around it.
struct Expr {
    marker: &'static [char],
    tag: Tag,
    good_before: String,
    bad_after: &'static str,
    bad_before: &'static str,
    /// `None`: anything may follow a closing marker.
    good_after: Option<String>,
}

/// `TextUtilities::Separators`.
fn separators(additional: &str) -> String {
    let mut s = String::from(" \u{10}\n\r\t.,:;<>|'\"[]{}!?%^()-+=");
    s.push('\u{fdd0}');
    s.push('\u{fdd1}');
    s.push('\u{2029}');
    s.push('\u{2028}');
    s.push_str("\u{ab}\u{bb}\u{201c}\u{201d}\u{2018}\u{2019}\u{2026}");
    s.push_str(additional);
    s
}

fn expressions() -> Vec<Expr> {
    let bold = separators("`~/");
    let italic = separators("`*~/");
    let mono = separators("*~/");
    let spoiler = separators("|*~/");
    vec![
        Expr {
            marker: &['*', '*'],
            tag: Tag::Bold,
            good_before: bold.clone(),
            bad_after: "*",
            bad_before: "*",
            good_after: Some(bold),
        },
        Expr {
            marker: &['_', '_'],
            tag: Tag::Italic,
            good_before: italic.clone(),
            bad_after: "_",
            bad_before: "_",
            good_after: Some(italic.clone()),
        },
        Expr {
            marker: &['~', '~'],
            tag: Tag::Strike,
            good_before: italic,
            bad_after: "~",
            bad_before: "~",
            good_after: None,
        },
        Expr {
            marker: &['`'],
            tag: Tag::Code,
            good_before: mono.clone(),
            bad_after: "`\n\r",
            bad_before: "`\n\r",
            good_after: Some(mono.clone()),
        },
        Expr {
            marker: &['`', '`', '`'],
            tag: Tag::Pre,
            good_before: mono.clone(),
            bad_after: "`",
            bad_before: "`",
            good_after: Some(mono),
        },
        Expr {
            marker: &['|', '|'],
            tag: Tag::Spoiler,
            good_before: spoiler.clone(),
            bad_after: "|",
            bad_before: "|",
            good_after: Some(spoiler),
        },
    ]
}

const INVALID: usize = usize::MAX / 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Open = 0,
    Close = 1,
}

/// `TagSearchItem`: where the next marker of one kind is.
#[derive(Clone)]
struct Item {
    offset: usize,
    /// `None`: not looked for yet (`-1`).
    position: Option<usize>,
    checked: [bool; 2],
}

impl Item {
    fn new() -> Self {
        Self {
            offset: 0,
            position: None,
            checked: [false, false],
        }
    }

    fn match_position(&self) -> usize {
        self.position.unwrap_or(INVALID)
    }

    fn apply_offset(&mut self, offset: usize) {
        if self.position.is_some_and(|p| p < offset) {
            self.position = None;
        }
        self.offset = self.offset.max(offset);
    }

    fn check(text: &[char], edge: Edge, e: &Expr, position: usize) -> bool {
        let len = e.marker.len();
        if position > 0 {
            let before = text[position - 1];
            if e.tag == Tag::Pre && before != '\n' && before != '\r' {
                return false;
            }
            if (edge == Edge::Open && !e.good_before.contains(before))
                || (edge == Edge::Close && e.bad_before.contains(before))
            {
                return false;
            }
        }
        if position + len < text.len() {
            let after = text[position + len];
            if (edge == Edge::Open && e.bad_after.contains(after))
                || (edge == Edge::Close
                    && e.good_after.as_ref().is_some_and(|g| !g.contains(after)))
            {
                return false;
            }
        }
        true
    }

    fn fill(&mut self, text: &[char], edge: Edge, e: &Expr) {
        let edge_index = edge as usize;
        if let Some(position) = self.position {
            if self.checked[edge_index] {
                return;
            } else if position != INVALID && Self::check(text, edge, e, position) {
                self.checked[edge_index] = true;
                return;
            } else {
                self.checked = [false, false];
            }
        }
        loop {
            let found = find(text, e.marker, self.offset);
            let Some(position) = found else {
                self.offset = INVALID;
                self.position = Some(INVALID);
                break;
            };
            self.position = Some(position);
            self.offset = position + e.marker.len();
            if Self::check(text, edge, e, position) {
                break;
            }
        }
        if self.position == Some(INVALID) {
            self.checked = [true, true];
        } else {
            self.checked = [false, false];
            self.checked[edge_index] = true;
        }
    }
}

fn find(text: &[char], marker: &[char], from: usize) -> Option<usize> {
    if from >= text.len() {
        return None;
    }
    (from..=text.len().checked_sub(marker.len())?).find(|&i| text[i..i + marker.len()] == *marker)
}

/// A found markdown tag (`InputField::MarkdownTag`), in chars of the text.
#[derive(Clone, Debug)]
struct Found {
    start: usize,
    len: Option<usize>,
    expr: usize,
    closed: bool,
}

/// `MarkdownTagAccumulator::feed` + `finish` over one run of unformatted text.
fn accumulate(text: &[char], exprs: &[Expr]) -> Vec<Found> {
    let mut tags: Vec<Found> = Vec::new();
    let mut items = vec![Item::new(); exprs.len()];
    let mut current = 0;
    let finish =
        |tags: &mut Vec<Found>, current: &mut usize, index: usize, till: usize, closed: bool| {
            let tag = &mut tags[index];
            if tag.len.is_none() {
                tag.len = Some(till.saturating_sub(tag.start));
                tag.closed = closed;
            }
            if index == *current {
                *current += 1;
            }
        };
    let mut try_finish = 0;
    loop {
        while try_finish < tags.len() {
            if tags[try_finish].len.is_some() {
                try_finish += 1;
                continue;
            }
            let index = tags[try_finish].expr;
            let e = &exprs[index];
            let at_least = tags[try_finish].start + e.marker.len() + 1;
            items[index].apply_offset(at_least);
            items[index].fill(text, Edge::Close, e);
            // `finishByNewline`: inline code ends at the line's end.
            if e.tag == Tag::Code {
                let from = tags[try_finish].start + 1;
                let newline = (from..text.len())
                    .find(|&i| text[i] == '\n' || text[i] == '\r')
                    .unwrap_or(INVALID);
                if items[index].match_position() > newline {
                    finish(&mut tags, &mut current, try_finish, newline, false);
                    try_finish += 1;
                    continue;
                }
            }
            let position = items[index].match_position();
            if position < INVALID {
                let till = position + e.marker.len();
                finish(&mut tags, &mut current, try_finish, till, true);
                items[index].apply_offset(till);
            }
            try_finish += 1;
        }
        for (i, item) in items.iter_mut().enumerate() {
            item.fill(text, Edge::Open, &exprs[i]);
        }
        let min = (0..items.len())
            .filter(|&i| items[i].match_position() < INVALID)
            .min_by_key(|&i| items[i].match_position());
        let Some(min) = min else {
            break;
        };
        tags.push(Found {
            start: items[min].match_position(),
            len: None,
            expr: min,
            closed: false,
        });
    }
    // `finishTags`: what is still open ends unclosed.
    let end = text.len();
    while current < tags.len() {
        let index = current;
        finish(&mut tags, &mut current, index, end, false);
    }
    tags
}

/// Links in the text (`ParseEntities(text, 0)`): a tag cutting one is not applied
/// (`http://test.com/__test__/test` goes as it is).
fn links(text: &[char]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest: String = text[i..text.len().min(i + 8)].iter().collect();
        let starts =
            rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("www.");
        let at_word_start = i == 0 || text[i - 1].is_whitespace();
        if starts && at_word_start {
            let end = (i..text.len())
                .find(|&j| text[j].is_whitespace())
                .unwrap_or(text.len());
            out.push(i..end);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// `ReadPreLanguageName`: `^([a-zA-Z0-9+\-]+)[\r\n]`.
fn pre_language(text: &[char]) -> Option<String> {
    let n = text
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '+' || **c == '-')
        .count();
    (n > 0 && text.get(n).is_some_and(|c| *c == '\n' || *c == '\r'))
        .then(|| text[..n].iter().collect::<String>().to_lowercase())
}

/// Applies the markdown of `text` outside its `tags` (formatted in the field).
pub fn apply(text: &str, tags: &[TagSpan]) -> Applied {
    let chars: Vec<char> = text.chars().collect();
    // Byte offset -> char index.
    let byte_to_char = |byte: usize| text[..byte.min(text.len())].chars().count();
    let formatted: Vec<Range<usize>> = tags
        .iter()
        .map(|t| byte_to_char(t.range.start)..byte_to_char(t.range.end))
        .collect();
    let exprs = expressions();
    // Runs of unformatted text, each searched on its own (a formatted fragment
    // finishes the markdown tags, `MarkdownTagAccumulator::feed`).
    let mut found: Vec<(usize, Found)> = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if formatted.iter().any(|r| r.contains(&start)) {
            start += 1;
            continue;
        }
        let end = (start..chars.len())
            .find(|i| formatted.iter().any(|r| r.contains(i)))
            .unwrap_or(chars.len());
        for f in accumulate(&chars[start..end], &exprs) {
            found.push((start, f));
        }
        start = end;
    }
    let links = links(&chars);

    // `getTextWithAppliedMarkdown`.
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    // Each original char's place in the result (removed ones: where they were).
    let mut map = vec![0usize; chars.len() + 1];
    let mut from = 0;
    let mut new_tags: Vec<(Range<usize>, Tag, Option<String>)> = Vec::new();
    let copy = |out: &mut Vec<char>, map: &mut Vec<usize>, range: Range<usize>| {
        for i in range {
            map[i] = out.len();
            out.push(chars[i]);
        }
    };
    for (base, tag) in &found {
        let e = &exprs[tag.expr];
        let marker = e.marker.len();
        let tag_start = base + tag.start;
        let len = tag.len.unwrap_or(0);
        if !tag.closed || tag_start < from {
            continue;
        }
        let mut entity_len = len as isize - 2 * marker as isize;
        if entity_len <= 0 {
            continue;
        }
        let tag_end = tag_start + len;
        if links.iter().any(|l| {
            l.start < tag_end && l.end > tag_start && (l.end > tag_end || l.start < tag_start)
        }) {
            continue;
        }
        copy(&mut out, &mut map, from..tag_start);
        let mut entity_start = tag_start + marker;
        let mut language = None;
        if e.tag == Tag::Pre {
            let inner = &chars[entity_start..entity_start + entity_len as usize];
            if let Some(name) = pre_language(inner) {
                // ```language-name{\n}code
                entity_start += name.chars().count() + 1;
                entity_len -= name.chars().count() as isize + 1;
                language = Some(name);
            } else if matches!(chars[entity_start], '\n' | '\r')
                && out.last().is_none_or(|c| *c == '\n' || *c == '\r')
            {
                entity_start += 1;
                entity_len -= 1;
            }
            let entity_end = entity_start as isize + entity_len;
            if entity_len > 0
                && matches!(chars[entity_end as usize - 1], '\n' | '\r')
                && (chars.len() <= entity_end as usize + marker
                    || matches!(chars[entity_end as usize + marker], '\n' | '\r'))
            {
                entity_len -= 1;
            }
        }
        // The markers (and what the code block drops) go.
        for m in map.iter_mut().take(tag_end).skip(tag_start) {
            *m = out.len();
        }
        if entity_len > 0 {
            let begin = out.len();
            copy(
                &mut out,
                &mut map,
                entity_start..entity_start + entity_len as usize,
            );
            new_tags.push((begin..out.len(), e.tag, language));
        }
        from = tag_end;
    }
    copy(&mut out, &mut map, from..chars.len());
    map[chars.len()] = out.len();

    // Char indices -> byte offsets of the result.
    let result: String = out.iter().collect();
    let mut char_to_byte: Vec<usize> = result.char_indices().map(|(b, _)| b).collect();
    char_to_byte.push(result.len());
    let to_bytes = |r: Range<usize>| char_to_byte[r.start]..char_to_byte[r.end];
    let mut tags_out: Vec<TagSpan> = formatted
        .iter()
        .zip(tags)
        .map(|(r, t)| TagSpan {
            range: to_bytes(map[r.start]..map[r.end]),
            tag: t.tag,
        })
        .filter(|t| !t.range.is_empty())
        .collect();
    let mut languages = Vec::new();
    for (range, tag, language) in new_tags {
        let range = to_bytes(range);
        if let Some(language) = language {
            languages.push((range.clone(), language));
        }
        tags_out.push(TagSpan { range, tag });
    }
    tags_out.sort_by_key(|t| (t.range.start, std::cmp::Reverse(t.range.end)));
    Applied {
        text: result,
        tags: tags_out,
        languages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> (String, Vec<(String, Tag)>) {
        let a = apply(text, &[]);
        let tags = a
            .tags
            .iter()
            .map(|t| (a.text[t.range.clone()].to_string(), t.tag))
            .collect();
        (a.text, tags)
    }

    /// The scenarios checked against tdesktop 7.4.1 (sent to Saved Messages).
    #[test]
    fn like_tdesktop() {
        assert_eq!(
            run("T1 inline `code` here"),
            (
                "T1 inline code here".into(),
                vec![("code".into(), Tag::Code)]
            )
        );
        // A code block's markers must start a line.
        assert_eq!(run("T2 ```one line block```").1, vec![]);
        assert_eq!(run("T3 ```rust\nfn main() {}\n```").1, vec![]);
        let a = apply("```rust\nfn main() {}\n```", &[]);
        assert_eq!(a.text, "fn main() {}");
        assert_eq!(a.languages[0].1, "rust");
        assert_eq!(
            run("T4 **bold** __italic__ ~~strike~~ ||spoiler||"),
            (
                "T4 bold italic strike spoiler".into(),
                vec![
                    ("bold".into(), Tag::Bold),
                    ("italic".into(), Tag::Italic),
                    ("strike".into(), Tag::Strike),
                    ("spoiler".into(), Tag::Spoiler),
                ]
            )
        );
        // Inside an applied tag the markers stay.
        assert_eq!(
            run("T5 **bold __bold italic__ bold**"),
            (
                "T5 bold __bold italic__ bold".into(),
                vec![("bold __bold italic__ bold".into(), Tag::Bold)]
            )
        );
        assert_eq!(
            run("T10 `**not bold**` and **`code in bold`**"),
            (
                "T10 **not bold** and `code in bold`".into(),
                vec![
                    ("**not bold**".into(), Tag::Code),
                    ("`code in bold`".into(), Tag::Bold)
                ]
            )
        );
        assert_eq!(
            run("T12 ** spaced ** and **unclosed"),
            (
                "T12  spaced  and **unclosed".into(),
                vec![(" spaced ".into(), Tag::Bold)]
            )
        );
        // Not inside words.
        assert_eq!(run("T13 a~~b~~c x__y__z").1, vec![]);
        let (text, tags) = run("T18 before\n```\ncode line\n```\nafter");
        assert_eq!(text, "T18 before\ncode line\nafter");
        assert_eq!(tags, vec![("code line".into(), Tag::Pre)]);
        assert_eq!(
            run("T19 [link text](https://telegram.org) and > not quote").1,
            vec![]
        );
        // A link keeps its underscores.
        assert_eq!(run("see https://test.com/__test__/test").1, vec![]);
    }

    #[test]
    fn formatted_text_takes_no_markdown() {
        let text = "**a** b";
        let a = apply(
            text,
            &[TagSpan {
                range: 0..5,
                tag: Tag::Italic,
            }],
        );
        assert_eq!(a.text, "**a** b");
        assert_eq!(
            a.tags,
            vec![TagSpan {
                range: 0..5,
                tag: Tag::Italic
            }]
        );
        // Formatted text after markdown shifts with it.
        let a = apply(
            "**a** b",
            &[TagSpan {
                range: 6..7,
                tag: Tag::Italic,
            }],
        );
        assert_eq!(a.text, "a b");
        assert!(a.tags.contains(&TagSpan {
            range: 2..3,
            tag: Tag::Italic
        }));
        assert!(a.tags.contains(&TagSpan {
            range: 0..1,
            tag: Tag::Bold
        }));
    }
}
