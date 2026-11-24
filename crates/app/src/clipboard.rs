//! Text with formatting on the clipboard, the way tdesktop puts it there
//! (`TextUtilities::MimeDataFromText`): the plain text, plus the text again as
//! UTF-8 in `application/x-td-field-text` and its tags in
//! `application/x-td-field-tags` (`SerializeTags`: a `QDataStream` of UTF-16
//! offsets and tag strings like `**`, `__`, `custom-emoji://<id>?<n>`, several
//! on one range joined by `\`). Qt registers a custom mime type as a clipboard
//! format of the same name, so text copied in tdesktop pastes here with its
//! custom emoji and formatting, and the other way round.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

pub const TAGS_TEXT_MIME: &str = "application/x-td-field-text";
pub const TAGS_MIME: &str = "application/x-td-field-tags";

/// `InputField::kCustomEmojiTagStart`.
pub const CUSTOM_EMOJI_TAG_START: &str = "custom-emoji://";
/// `kTagSeparator`.
const TAG_SEPARATOR: char = '\\';

/// Text and its tags: byte ranges with tag strings (several joined by `\`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tagged {
    pub text: String,
    pub tags: Vec<(Range<usize>, String)>,
}

/// `InputField::CustomEmojiLink`: unique per emoji (`MakeUniqueCustomEmojiLink`),
/// so neighbours are never merged into one tag.
pub fn custom_emoji_link(id: i64) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{CUSTOM_EMOJI_TAG_START}{}?{n}", id as u64)
}

/// `InputField::CustomEmojiEntityData` as a document id.
pub fn custom_emoji_id(tag: &str) -> Option<i64> {
    let data = tag.strip_prefix(CUSTOM_EMOJI_TAG_START)?;
    let data = data.split('?').next()?;
    data.parse::<u64>().ok().map(|v| v as i64)
}

/// `SplitTags`.
pub fn split_tags(tag: &str) -> impl Iterator<Item = &str> {
    tag.split(TAG_SEPARATOR)
}

fn utf16_less(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `JoinTag` of a sorted list: code and pre stay out of combinations, except
/// code inside quotes only (`IsSeparateTag`).
fn join_tag(list: &[&str]) -> String {
    let quote = |t: &str| t == ">" || t == ">^";
    let separate = |t: &str| t == "`" || t == "```";
    let Some(first) = list.first() else {
        return String::new();
    };
    let mut out = (*first).to_string();
    let mut only_quotes = quote(first);
    for tag in &list[1..] {
        if !separate(tag) || (only_quotes && *tag == "`") {
            out.push(TAG_SEPARATOR);
            out.push_str(tag);
        }
        only_quotes = only_quotes && quote(tag);
    }
    out
}

/// Single tags on possibly overlapping byte ranges as tdesktop's tags: one
/// combined tag per stretch (`ConvertEntitiesToTextTags`), equal neighbours
/// merged (`SimplifyTags`).
pub fn combine(singles: &[(Range<usize>, String)]) -> Vec<(Range<usize>, String)> {
    let mut points: Vec<usize> = singles
        .iter()
        .filter(|(r, _)| !r.is_empty())
        .flat_map(|(r, _)| [r.start, r.end])
        .collect();
    points.sort_unstable();
    points.dedup();
    let mut out: Vec<(Range<usize>, String)> = Vec::new();
    for pair in points.windows(2) {
        let (from, till) = (pair[0], pair[1]);
        let mut active: Vec<&str> = singles
            .iter()
            .filter(|(r, _)| r.start <= from && r.end >= till && !r.is_empty())
            .map(|(_, t)| t.as_str())
            .collect();
        if active.is_empty() {
            continue;
        }
        active.sort_by(|a, b| utf16_less(a, b));
        active.dedup();
        let tag = join_tag(&active);
        match out.last_mut() {
            Some((r, last)) if r.end == from && *last == tag => r.end = till,
            _ => out.push((from..till, tag)),
        }
    }
    out
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}

/// `SerializeTags` (`QDataStream`, `Qt_5_1`: big-endian, a `QString` as its
/// byte length and UTF-16 units) with offsets in UTF-16 units of `text`.
pub fn serialize_tags(text: &str, tags: &[(Range<usize>, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    if tags.is_empty() {
        return out;
    }
    out.extend_from_slice(&(tags.len() as i32).to_be_bytes());
    for (range, id) in tags {
        let start = utf16_offset(text, range.start);
        let end = utf16_offset(text, range.end);
        out.extend_from_slice(&(start as i32).to_be_bytes());
        out.extend_from_slice(&((end - start) as i32).to_be_bytes());
        let units: Vec<u16> = id.encode_utf16().collect();
        out.extend_from_slice(&((units.len() * 2) as u32).to_be_bytes());
        for u in units {
            out.extend_from_slice(&u.to_be_bytes());
        }
    }
    out
}

/// `DeserializeTags`: the tags as byte ranges of `text`; nothing if anything is
/// out of place.
pub fn deserialize_tags(data: &[u8], text: &str) -> Vec<(Range<usize>, String)> {
    // UTF-16 offset -> byte offset (only at char boundaries).
    let mut bytes_at = Vec::with_capacity(text.len() + 1);
    for (i, ch) in text.char_indices() {
        bytes_at.push(i);
        if ch.len_utf16() == 2 {
            bytes_at.push(i);
        }
    }
    bytes_at.push(text.len());
    let length = bytes_at.len() - 1;
    let mut at = 0usize;
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let slice = data.get(*at..*at + n)?;
        *at += n;
        Some(slice)
    };
    let int =
        |at: &mut usize| -> Option<i32> { Some(i32::from_be_bytes(take(at, 4)?.try_into().ok()?)) };
    let Some(count) = int(&mut at) else {
        return Vec::new();
    };
    if count <= 0 || count as usize > length {
        return Vec::new();
    }
    let mut out = Vec::new();
    for _ in 0..count {
        let (Some(offset), Some(len)) = (int(&mut at), int(&mut at)) else {
            return out;
        };
        let Some(size) = int(&mut at).map(|v| v as u32) else {
            return out;
        };
        let id = if size == u32::MAX {
            String::new()
        } else {
            let Some(raw) = take(&mut at, size as usize) else {
                return out;
            };
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        };
        if offset < 0 || len <= 0 || (offset + len) as usize > length {
            return out;
        }
        let (start, end) = (bytes_at[offset as usize], bytes_at[(offset + len) as usize]);
        if start < end {
            out.push((start..end, id));
        }
    }
    out
}

/// Puts tagged text on the clipboard (GPUI's plain text where that fails).
pub fn copy(tagged: Tagged, cx: &mut gpui::App) {
    if !write(&tagged.text, Some(&tagged)) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(tagged.text));
    }
}

/// Puts `plain` on the clipboard, with `tagged` (when it has tags) in
/// tdesktop's formats. `false` when the clipboard could not be opened.
#[cfg(windows)]
pub fn write(plain: &str, tagged: Option<&Tagged>) -> bool {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    // SAFETY: the clipboard is opened, filled and closed on this thread.
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        // Qt's `QWindowsMimeText`: CRLF line ends and a terminating zero.
        let wide: Vec<u16> = plain
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let wide_bytes: Vec<u8> = wide.iter().flat_map(|u| u.to_ne_bytes()).collect();
        set_data(CF_UNICODETEXT.0.into(), &wide_bytes);
        if let Some(tagged) = tagged.filter(|t| !t.tags.is_empty()) {
            set_data(format(TAGS_TEXT_MIME), tagged.text.as_bytes());
            set_data(
                format(TAGS_MIME),
                &serialize_tags(&tagged.text, &tagged.tags),
            );
        }
        let _ = CloseClipboard();
    }
    true
}

#[cfg(not(windows))]
pub fn write(_plain: &str, _tagged: Option<&Tagged>) -> bool {
    false
}

/// tdesktop's tagged text on the clipboard, if it is there.
#[cfg(windows)]
pub fn read_tagged() -> Option<Tagged> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, IsClipboardFormatAvailable, OpenClipboard,
    };
    let (text_format, tags_format) = (format(TAGS_TEXT_MIME), format(TAGS_MIME));
    // SAFETY: the clipboard is opened, read and closed on this thread.
    unsafe {
        if IsClipboardFormatAvailable(text_format).is_err()
            || IsClipboardFormatAvailable(tags_format).is_err()
        {
            return None;
        }
        OpenClipboard(None).ok()?;
        let text = get_data(text_format);
        let tags = get_data(tags_format);
        let _ = CloseClipboard();
        let mut text = String::from_utf8_lossy(&text?).into_owned();
        // A global's size may be rounded up.
        while text.ends_with('\0') {
            text.pop();
        }
        let tags = deserialize_tags(&tags?, &text);
        Some(Tagged { text, tags })
    }
}

#[cfg(not(windows))]
pub fn read_tagged() -> Option<Tagged> {
    None
}

#[cfg(windows)]
fn format(name: &str) -> u32 {
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a zero-terminated name.
    unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) }
}

/// # Safety
/// The clipboard must be open.
#[cfg(windows)]
unsafe fn set_data(format: u32, data: &[u8]) {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::SetClipboardData;
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    // SAFETY: a fresh global of the data's size, filled while locked; the
    // clipboard owns it once set.
    unsafe {
        let Ok(global): Result<HGLOBAL, _> = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)) else {
            return;
        };
        let at = GlobalLock(global);
        if at.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), at.cast::<u8>(), data.len());
        let _ = GlobalUnlock(global);
        let _ = SetClipboardData(format, Some(HANDLE(global.0)));
    }
}

/// # Safety
/// The clipboard must be open.
#[cfg(windows)]
unsafe fn get_data(format: u32) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    // SAFETY: the handle is the clipboard's, read while locked.
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let at = GlobalLock(global);
        if at.is_null() {
            return None;
        }
        let out = std::slice::from_raw_parts(at.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(global);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_overlapping_tags_like_tdesktop() {
        // "ab" bold, "bc" italic.
        let tags = combine(&[(0..2, "**".into()), (1..3, "__".into())]);
        assert_eq!(
            tags,
            vec![
                (0..1, "**".to_string()),
                (1..2, "**\\__".to_string()),
                (2..3, "__".to_string()),
            ]
        );
        // Code never joins bold.
        assert_eq!(
            combine(&[(0..2, "**".into()), (0..2, "`".into())]),
            vec![(0..2, "**".to_string())]
        );
    }

    #[test]
    fn tags_round_trip_in_utf16() {
        let text = "a😀b";
        let tags = vec![
            (1..5, "custom-emoji://5?1".to_string()),
            (5..6, "**".into()),
        ];
        let data = serialize_tags(text, &tags);
        // Count, then offset 1 and length 2 (UTF-16).
        assert_eq!(&data[..12], &[0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
        assert_eq!(deserialize_tags(&data, text), tags);
        assert_eq!(custom_emoji_id("custom-emoji://5?1"), Some(5));
        assert_eq!(deserialize_tags(&data, "a"), Vec::new());
    }
}
