//! Emoji by short codes (`:smile:`), ported from lib_ui's `emoji_suggestions`
//! with codegen's `emoji/replaces.cpp`: the replacements read from
//! `emoji_autocomplete.json` (`PrepareReplaces`, `CheckAndConvertReplaces`), the
//! `Completer` that finds them by a query, and the field's instant replaces
//! (`InstantReplaces::Default`).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// One short code (`internal::Replacement`).
pub struct Replacement {
    pub emoji: u16,
    /// `:code:`.
    pub replacement: String,
    /// The code's words, sorted (`ReplacementWords`).
    words: Vec<String>,
}

struct Data {
    list: Vec<Replacement>,
    /// Replacements by the first character of any of their words.
    by_first: HashMap<char, Vec<usize>>,
    max_length: usize,
    /// What is typed (UTF-16 units) -> what it becomes.
    instant: HashMap<Vec<u16>, String>,
    instant_max: usize,
}

/// `ReplacementWords`: split by everything but letters and numbers; `-` and `+`
/// stay inside a word before a number.
fn replacement_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_alphanumeric()
            || ((ch == '-' || ch == '+') && chars.get(i + 1).is_some_and(|c| c.is_numeric()))
        {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words.sort();
    words
}

fn code_ok(code: &str) -> bool {
    code.len() > 2
        && code.starts_with(':')
        && code.ends_with(':')
        && code[1..code.len() - 1]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '_'))
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `ConvertEmojiId`: what codegen skips or maps to another emoji.
fn convert_id(id: &str, replacement: &str) -> Option<String> {
    let tone = replacement
        .find("_tone")
        .is_some_and(|at| replacement[at + 5..].starts_with(|c: char| c.is_ascii_digit()));
    if tone {
        return None;
    }
    let u: Vec<u32> = id.chars().map(u32::from).collect();
    let not_supported = match u.as_slice() {
        [0x23, 0xFE0F] | [0x2A, 0xFE0F] => true,
        [d, 0xFE0F] if (0x30..=0x39).contains(d) => true,
        [t] if (0x1F3FB..=0x1F3FF).contains(t) => true,
        [r] if (0x1F1E6..=0x1F1FF).contains(r) => true,
        [0x2640, 0xFE0F] | [0x2642, 0xFE0F] | [0x2695, 0xFE0F] => true,
        _ => false,
    };
    if not_supported {
        return None;
    }
    let s = |v: &[u32]| {
        v.iter()
            .filter_map(|c| char::from_u32(*c))
            .collect::<String>()
    };
    const MALE: &str = "\u{200D}\u{2642}\u{FE0F}";
    const FEMALE: &str = "\u{200D}\u{2640}\u{FE0F}";
    let converted = match u.as_slice() {
        [0x1F926] | [0x1F938] | [0x1F939] | [0x1F93C] | [0x1F93D] => format!("{id}{MALE}"),
        [0x1F937] | [0x1F93E] => format!("{id}{FEMALE}"),
        [
            0x1F469,
            0x200D,
            0x2764,
            0xFE0F,
            0x200D,
            0x1F48B,
            0x200D,
            0x1F468,
        ] => s(&[0x1F48F]),
        [0x1F468, 0x200D, 0x1F469, 0x200D, 0x1F466] => s(&[0x1F46A]),
        [0x1F469, 0x200D, 0x2764, 0xFE0F, 0x200D, 0x1F468] => s(&[0x1F491]),
        [a, b] if (0x1F1E6..=0x1F1FF).contains(a) && (0x1F1E6..=0x1F1FF).contains(b) => {
            let flag = |x: char, y: char| {
                (
                    0x1F1E6 + (x as u32 - 'a' as u32),
                    0x1F1E6 + (y as u32 - 'a' as u32),
                )
            };
            let pairs = [
                (('a', 'c'), ('s', 'h')),
                (('b', 'v'), ('n', 'o')),
                (('c', 'p'), ('f', 'r')),
                (('d', 'g'), ('i', 'o')),
                (('e', 'a'), ('e', 's')),
                (('h', 'm'), ('a', 'u')),
                (('m', 'f'), ('f', 'r')),
                (('s', 'j'), ('n', 'o')),
                (('t', 'a'), ('s', 'h')),
                (('u', 'm'), ('u', 's')),
            ];
            match pairs
                .iter()
                .find(|(from, _)| flag(from.0, from.1) == (*a, *b))
            {
                Some((_, to)) => {
                    let (x, y) = flag(to.0, to.1);
                    s(&[x, y])
                }
                None => id.to_string(),
            }
        }
        _ => id.to_string(),
    };
    Some(converted)
}

/// `findAndSort` with its fallbacks: the emoji of a replacement's id.
fn resolve(id: &str) -> Option<u16> {
    const MALE: &str = "\u{200D}\u{2642}\u{FE0F}";
    const FEMALE: &str = "\u{200D}\u{2640}\u{FE0F}";
    let find =
        |id: &str| crate::by_text(id).or_else(|| crate::by_text(&id.replace('\u{FE0F}', "")));
    if let Some(e) = find(id) {
        return Some(e);
    }
    if let Some(base) = id.strip_suffix(MALE).or_else(|| id.strip_suffix(FEMALE)) {
        return find(base);
    }
    find(&format!("{id}{MALE}")).or_else(|| find(&format!("{id}{FEMALE}")))
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../../../assets/emoji/emoji_autocomplete.json"
        ))
        .unwrap_or_default();
        let mut list = Vec::new();
        let mut add = |id: &str, code: &str| {
            if let Some(emoji) = resolve(id) {
                let words = replacement_words(code);
                if !words.is_empty() {
                    list.push(Replacement {
                        emoji,
                        replacement: code.to_string(),
                        words,
                    });
                }
            }
        };
        if let Some(map) = json.as_object() {
            // The JSON's own order is its keys' (codegen reads a QJsonObject).
            let sorted: BTreeMap<&String, &serde_json::Value> = map.iter().collect();
            for value in sorted.values() {
                let get = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("");
                let output: String = get("output")
                    .split('-')
                    .filter_map(|h| u32::from_str_radix(h, 16).ok())
                    .filter_map(char::from_u32)
                    .collect();
                let code = get("alpha_code");
                if !code_ok(code) {
                    continue;
                }
                let Some(id) = convert_id(&output, code) else {
                    continue;
                };
                add(&id, code);
                for alias in get("aliases").split('|') {
                    if alias != ":shrug:" && code_ok(alias) {
                        add(&id, alias);
                    }
                }
            }
        }
        add("\u{1F44D}", ":like:");
        add("\u{1F44E}", ":dislike:");
        add("\u{1F914}", ":hmm:");
        add("\u{1F973}", ":party:");
        // `CheckAndConvertReplaces`: in the order of the emoji in the panel.
        let order: HashMap<u16, usize> = crate::Section::ALL
            .iter()
            .flat_map(|s| crate::section(*s).iter().copied())
            .enumerate()
            .map(|(k, e)| (e, k))
            .collect();
        list.sort_by_key(|r| order.get(&r.emoji).copied().unwrap_or(usize::MAX));
        let mut by_first: HashMap<char, Vec<usize>> = HashMap::new();
        for (k, r) in list.iter().enumerate() {
            for w in &r.words {
                if let Some(ch) = w.chars().next() {
                    let l = by_first.entry(ch).or_default();
                    if l.last() != Some(&k) {
                        l.push(k);
                    }
                }
            }
        }
        let max_length = list
            .iter()
            .map(|r| r.replacement.encode_utf16().count())
            .max()
            .unwrap_or(0);

        // `InstantReplaces::Default`.
        let mut instant: HashMap<Vec<u16>, String> = HashMap::new();
        let mut put = |what: &str, with: String| {
            instant.insert(units(what), with);
        };
        put("--", "\u{2014}".into());
        put("<<", "\u{AB}".into());
        put(">>", "\u{BB}".into());
        put(":shrug:", "\u{AF}\\_(\u{30C4})_/\u{AF}".into());
        put(":o ", "\u{1F628}".into());
        put("xD ", "\u{1F606}".into());
        for r in &list {
            if let Some(one) = crate::get(r.emoji) {
                put(&r.replacement, one.text());
            }
        }
        for (what, emoji) in crate::replaces() {
            if let Some(one) = crate::get(*emoji) {
                put(what, one.text());
            }
        }
        let instant_max = instant.keys().map(Vec::len).max().unwrap_or(0);
        Data {
            list,
            by_first,
            max_length,
            instant,
            instant_max,
        }
    })
}

/// The emoji panel's tooltip (`EmojiListWidget::tooltipText`): the first short
/// code of the emoji's original.
pub fn code_of(emoji: u16) -> Option<&'static str> {
    let original = crate::get(emoji).map_or(emoji, |e| e.original());
    data()
        .list
        .iter()
        .find(|r| r.emoji == original)
        .map(|r| r.replacement.as_str())
}

/// `GetSuggestionMaxLength`.
pub fn max_length() -> usize {
    data().max_length
}

/// `processInstantReplaces`: after typing one character, the shortest
/// replacement that the text before the caret (`before`, ending with that
/// character) ends with: (its length in bytes, what it becomes).
pub fn instant_replace(before: &str) -> Option<(usize, &'static str)> {
    let d = data();
    let all: Vec<u16> = before.encode_utf16().collect();
    let tail: Vec<u16> = all.iter().rev().take(d.instant_max).copied().collect();
    let mut key: Vec<u16> = Vec::with_capacity(tail.len());
    for unit in tail {
        key.insert(0, unit);
        if let Some(with) = d.instant.get(&key) {
            let what = String::from_utf16(&key).ok()?;
            if !before.ends_with(&what) {
                return None;
            }
            return Some((what.len(), with.as_str()));
        }
    }
    None
}

/// What a suggestion gives: the emoji and its code.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub emoji: u16,
    pub replacement: String,
}

fn is_letter_or_number(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit()
}

/// `Completer::NormalizeQuery`: only letters, numbers and `-`/`+` before a number.
fn normalize_query(query: &str) -> Vec<char> {
    let chars: Vec<char> = query.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    for (i, &ch) in chars.iter().enumerate() {
        if is_letter_or_number(ch)
            || ((ch == '-' || ch == '+') && (i + 1 == chars.len() || chars[i + 1].is_ascii_digit()))
        {
            out.push(ch);
        }
    }
    out
}

struct Completer<'a> {
    query: Vec<char>,
    initial: Vec<char>,
    words: &'a [String],
    used: Vec<bool>,
    used_count: usize,
}

impl Completer<'_> {
    fn starts_with_query(&self, word: &str) -> bool {
        let w: Vec<char> = word.chars().collect();
        w.len() >= self.query.len() && w[..self.query.len()] == self.query[..]
    }

    fn equal_chars(&self, position: usize, word: &[char]) -> usize {
        let left = self.query.len() - position;
        let possible = left.min(word.len());
        (1..possible)
            .find(|&k| word[k] != self.query[position + k])
            .unwrap_or(possible)
    }

    fn match_tail(&mut self, position: usize) -> bool {
        if position == self.query.len() {
            return true;
        }
        let first = self.query[position];
        for index in 0..self.words.len() {
            let word: Vec<char> = self.words[index].chars().collect();
            if word.first() != Some(&first) || self.used[index] {
                continue;
            }
            self.used[index] = true;
            self.used_count += 1;
            let equal = self.equal_chars(position, &word);
            for check in (1..=equal).rev() {
                if self.match_tail(position + check) {
                    self.used[index] = false;
                    return true;
                }
            }
            self.used_count -= 1;
            self.used[index] = false;
        }
        false
    }

    fn is_exact(&self, replacement: &str) -> bool {
        let r: Vec<char> = replacement.chars().collect();
        r.len() == self.initial.len() + 1 && r[..self.initial.len()] == self.initial[..]
    }
}

/// `GetSuggestions`: emoji whose codes match `query` (typed after `:`, with it).
pub fn suggestions(query: &str) -> Vec<Suggestion> {
    let d = data();
    let normalized = normalize_query(query);
    let Some(&first) = normalized.first() else {
        return Vec::new();
    };
    let Some(initial_list) = d.by_first.get(&first) else {
        return Vec::new();
    };
    // (replacement index, words used)
    let mut result: Vec<(usize, usize)> = Vec::new();
    let add = |k: usize, used: usize, result: &mut Vec<(usize, usize)>| {
        let item = &d.list[k];
        match result.last_mut() {
            Some(last) if d.list[last.0].emoji == item.emoji => {
                // `isBetterThanLastResult`.
                let good = |r: &Replacement| r.replacement[1..].starts_with(first);
                if used < last.1 || (!good(&d.list[last.0]) && good(item)) {
                    *last = (k, used);
                }
            }
            _ => result.push((k, used)),
        }
    };
    let initial: Vec<char> = query.chars().collect();
    if normalized.len() > 1 {
        for &k in initial_list {
            let item = &d.list[k];
            let mut c = Completer {
                query: normalized.clone(),
                initial: initial.clone(),
                words: &item.words,
                used: vec![false; item.words.len()],
                used_count: 1,
            };
            let matched = if item.words.len() < 2 {
                c.starts_with_query(&item.words[0])
            } else {
                c.match_tail(0)
            };
            if matched {
                add(k, c.used_count, &mut result);
            }
        }
    } else {
        for &k in initial_list {
            add(k, 1, &mut result);
        }
    }
    let probe = Completer {
        query: normalized.clone(),
        initial,
        words: &[],
        used: Vec::new(),
        used_count: 0,
    };
    // `prepareResult`: stable partitions, the last one wins.
    let partition = |result: &mut Vec<(usize, usize)>, f: &dyn Fn(&(usize, usize)) -> bool| {
        let (mut yes, no): (Vec<_>, Vec<_>) = result.iter().partition(|r| f(r));
        yes.extend(no);
        *result = yes;
    };
    partition(&mut result, &|r| {
        d.list[r.0].replacement[1..].starts_with(first)
    });
    partition(&mut result, &|r| r.1 < 2);
    partition(&mut result, &|r| r.1 < 3);
    partition(&mut result, &|r| probe.is_exact(&d.list[r.0].replacement));
    result
        .into_iter()
        .map(|(k, _)| Suggestion {
            emoji: d.list[k].emoji,
            replacement: d.list[k].replacement.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(e: u16) -> String {
        crate::get(e).unwrap().text()
    }

    #[test]
    fn codes_are_suggested() {
        let list = suggestions(":smile");
        assert!(!list.is_empty());
        assert_eq!(list[0].replacement, ":smile:");
        assert_eq!(text(list[0].emoji), "\u{1F604}");
        // Words in any order: "heart eyes".
        let list = suggestions(":eyesheart");
        assert!(
            list.iter().any(|s| s.replacement.contains("heart_eyes")),
            "{list:?}"
        );
        assert!(suggestions(":").is_empty());
    }

    #[test]
    fn instant_replaces() {
        assert_eq!(instant_replace("a --"), Some((2, "\u{2014}")));
        assert_eq!(instant_replace("hi :-)").map(|r| r.0), Some(3));
        let (len, with) = instant_replace("so :smile:").unwrap();
        assert_eq!((len, with), (7, "\u{1F604}"));
        assert_eq!(instant_replace("plain"), None);
        assert_eq!(instant_replace("xD ").map(|r| r.1), Some("\u{1F606}"));
    }
}
