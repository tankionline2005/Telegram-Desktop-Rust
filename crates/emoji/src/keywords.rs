//! Emoji keywords of languages from the server (tdesktop's
//! `ChatHelpers::EmojiKeywords`): a lang pack per language with its version,
//! updated by differences (`messages.getEmojiKeywordsDifference`), asked by
//! prefix or exactly, merged with the short codes (`AppendLegacySuggestions`),
//! the recent emoji first (`PrioritizeRecent`). The network and the file live
//! in the data crate; this is the state and the queries.

use std::collections::BTreeMap;

/// `kRefreshEach`: a lang pack is asked for its difference at most this often.
pub const REFRESH_EACH_SECS: u64 = 60 * 60;

/// One language's keywords (`LangPackData`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LangPack {
    pub version: i32,
    /// Lowercased keyword -> emoji texts (as the server sent them).
    pub emoji: BTreeMap<String, Vec<String>>,
}

/// A keyword's change from the server (`emojiKeyword` / `emojiKeywordDeleted`).
pub enum Change {
    Add {
        keyword: String,
        emoticons: Vec<String>,
    },
    Delete {
        keyword: String,
        emoticons: Vec<String>,
    },
}

/// `MustAddPostfix`: ™ © ® are emoji only with FE0F.
fn emoji_of(text: &str) -> Option<u16> {
    let postfixed;
    let text = if matches!(text, "\u{2122}" | "\u{A9}" | "\u{AE}") {
        postfixed = format!("{text}\u{FE0F}");
        postfixed.as_str()
    } else {
        text
    };
    crate::by_text(text)
}

/// `NormalizeKey`.
fn normalize_key(key: &str) -> String {
    key.to_lowercase().trim().to_string()
}

impl LangPack {
    /// `ApplyDifference`.
    pub fn apply(&mut self, version: i32, changes: Vec<Change>) {
        self.version = version;
        for change in changes {
            match change {
                Change::Add { keyword, emoticons } => {
                    let word = normalize_key(&keyword);
                    if word.is_empty() {
                        continue;
                    }
                    let list = self.emoji.entry(word).or_default();
                    list.extend(emoticons.into_iter().filter(|e| emoji_of(e).is_some()));
                }
                Change::Delete { keyword, emoticons } => {
                    let word = normalize_key(&keyword);
                    if let Some(list) = self.emoji.get_mut(&word) {
                        list.retain(|e| !emoticons.contains(e));
                        if list.is_empty() {
                            self.emoji.remove(&word);
                        }
                    }
                }
            }
        }
    }

    /// `maxKeyLength` (UTF-16 units, as Qt counts).
    pub fn max_key_length(&self) -> usize {
        self.emoji
            .keys()
            .map(|k| k.encode_utf16().count())
            .max()
            .unwrap_or(0)
    }

    /// `LangPack::query`: emoji of keywords starting with `normalized` (or equal
    /// to it), without repeats, with the keyword they were found by.
    fn query(&self, language: &str, normalized: &str, exact: bool) -> Vec<(u16, String)> {
        if normalized.encode_utf16().count() > self.max_key_length()
            || self.emoji.is_empty()
            || (exact && skip_exact(language, normalized))
        {
            return Vec::new();
        }
        let mut out: Vec<(u16, String)> = Vec::new();
        for (key, list) in self.emoji.range(normalized.to_string()..) {
            let fits = if exact {
                key == normalized
            } else {
                key.starts_with(normalized)
            };
            if !fits {
                break;
            }
            for text in list {
                if let Some(e) = emoji_of(text)
                    && !out.iter().any(|(x, _)| *x == e)
                {
                    out.push((e, key.clone()));
                }
            }
        }
        out
    }

    /// The file format: the version, then a keyword per line with its emoji
    /// separated by tabs.
    pub fn serialize(&self) -> String {
        let mut out = format!("{}\n", self.version);
        for (key, list) in &self.emoji {
            out.push_str(&key.replace(['\t', '\n'], " "));
            for e in list {
                out.push('\t');
                out.push_str(e);
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let version = lines.next()?.trim().parse().ok()?;
        let mut emoji = BTreeMap::new();
        for line in lines {
            let mut parts = line.split('\t');
            let Some(key) = parts.next().filter(|k| !k.is_empty()) else {
                continue;
            };
            emoji.insert(key.to_string(), parts.map(str::to_string).collect());
        }
        Some(Self { version, emoji })
    }
}

/// `SkipExactKeyword`: words too short or too common to suggest on their own.
fn skip_exact(language: &str, word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    if (chars.len() == 1 && !chars[0].is_alphabetic()) || word == "10" {
        return true;
    } else if language != "en" {
        return false;
    }
    (chars.len() == 1 && chars[0] != '$' && chars[0] != '\u{20AC}')
        || (chars.len() == 2 && !matches!(word, "us" | "uk" | "hi" | "ok"))
}

/// One found emoji: the emoji, the keyword or code it was found by.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub emoji: u16,
    pub label: String,
}

/// `EmojiKeywords::query`: every language's keywords (in the order given), then
/// the short codes for a prefix query of letters, numbers, `_`, `-` and `+`.
pub fn query(packs: &[(String, LangPack)], query: &str, exact: bool) -> Vec<Found> {
    let normalized = query.to_lowercase();
    if normalized.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Found> = Vec::new();
    for (language, pack) in packs {
        for (emoji, label) in pack.query(language, &normalized, exact) {
            if !out.iter().any(|f| f.emoji == emoji) {
                out.push(Found { emoji, label });
            }
        }
    }
    if !exact
        && query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
    {
        for s in crate::suggestions::suggestions(query) {
            if !out.iter().any(|f| f.emoji == s.emoji) {
                out.push(Found {
                    emoji: s.emoji,
                    label: s.replacement,
                });
            }
        }
    }
    out
}

/// `PrioritizeRecent`: the recent emoji (their uncolored originals) first, in
/// the recent order.
pub fn prioritize_recent(mut list: Vec<Found>, recent: &[u16]) -> Vec<Found> {
    let mut last = 0;
    for &item in recent {
        let original = crate::get(item).map_or(item, |e| e.original());
        // Only one found after the mark moves (`it > lastRecent`).
        if let Some(at) = list.iter().position(|f| f.emoji == original)
            && at > last
        {
            let found = list.remove(at);
            list.insert(last, found);
            last += 1;
        }
    }
    list
}

/// `maxQueryLength`: the longest keyword of all languages.
pub fn max_query_length(packs: &[(String, LangPack)]) -> usize {
    packs
        .iter()
        .map(|(_, p)| p.max_key_length())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> LangPack {
        let mut p = LangPack::default();
        p.apply(
            3,
            vec![
                Change::Add {
                    keyword: "Кот".into(),
                    emoticons: vec!["\u{1F431}".into(), "\u{1F408}".into()],
                },
                Change::Add {
                    keyword: "котик".into(),
                    emoticons: vec!["\u{1F431}".into(), "\u{1F63A}".into()],
                },
                Change::Add {
                    keyword: "bad".into(),
                    emoticons: vec!["not an emoji".into()],
                },
            ],
        );
        p
    }

    #[test]
    fn keywords_by_prefix_and_exactly() {
        let packs = vec![("ru".to_string(), pack())];
        let found = query(&packs, "кот", false);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].label, "кот");
        assert_eq!(query(&packs, "кот", true).len(), 2);
        assert!(packs[0].1.emoji["bad"].is_empty());
        let back = LangPack::parse(&packs[0].1.serialize()).unwrap();
        assert_eq!(back, packs[0].1);
    }

    #[test]
    fn deleted_keywords_and_recent_first() {
        let mut p = pack();
        p.apply(
            4,
            vec![Change::Delete {
                keyword: "кот".into(),
                emoticons: vec!["\u{1F431}".into(), "\u{1F408}".into()],
            }],
        );
        assert!(!p.emoji.contains_key("кот"));
        let packs = vec![("ru".to_string(), p)];
        let found = query(&packs, "кот", false);
        let last = found.last().unwrap().emoji;
        let sorted = prioritize_recent(found, &[last]);
        assert_eq!(sorted[0].emoji, last);
    }

    #[test]
    fn short_english_words_are_not_exact() {
        assert!(skip_exact("en", "ab"));
        assert!(!skip_exact("en", "ok"));
        assert!(!skip_exact("ru", "ко"));
    }
}
