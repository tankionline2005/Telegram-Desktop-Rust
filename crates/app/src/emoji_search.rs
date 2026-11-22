//! The emoji panel's search on this computer: the English names, short codes and
//! aliases of lib_ui's `emoji_autocomplete.json` (tdesktop asks the server for
//! keywords in the interface language, `EmojiKeywords`; that comes later).

use std::sync::OnceLock;

struct Named {
    emoji: u16,
    /// Lowercase words of the name, the short code and the aliases.
    words: Vec<String>,
}

fn names() -> &'static [Named] {
    static NAMES: OnceLock<Vec<Named>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../../../assets/emoji/emoji_autocomplete.json"
        ))
        .unwrap_or_default();
        let mut out = Vec::new();
        let Some(map) = json.as_object() else {
            return out;
        };
        for value in map.values() {
            let Some(output) = value.get("output").and_then(|v| v.as_str()) else {
                continue;
            };
            let text: String = output
                .split('-')
                .filter_map(|h| u32::from_str_radix(h, 16).ok())
                .filter_map(char::from_u32)
                .collect();
            let Some(emoji) = tdr_emoji::by_text(&text) else {
                continue;
            };
            let mut words = Vec::new();
            for key in ["name", "alpha_code", "aliases"] {
                if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
                    words.extend(
                        v.to_lowercase()
                            .split(|c: char| !c.is_alphanumeric())
                            .filter(|w| !w.is_empty())
                            .map(str::to_string),
                    );
                }
            }
            out.push(Named { emoji, words });
        }
        // In the picker's order.
        out.sort_by_key(|n| n.emoji);
        out
    })
}

/// Emoji whose words start with every word of `query`.
pub fn search(query: &str) -> Vec<u16> {
    let query: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    if query.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<u16> = Vec::new();
    for named in names() {
        if query
            .iter()
            .all(|q| named.words.iter().any(|w| w.starts_with(q.as_str())))
            && !out.contains(&named.emoji)
        {
            out.push(named.emoji);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_by_name() {
        let found = super::search("heart");
        assert!(found.contains(&tdr_emoji::by_text("❤️").unwrap()));
        assert!(super::search("zzzzqq").is_empty());
    }
}
