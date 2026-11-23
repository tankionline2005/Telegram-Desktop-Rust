//! Recent emoji and chosen skin tones (`Core::Settings`: `recentEmoji`,
//! `incrementRecentEmoji`, `hideRecentEmoji`, `resetRecentEmoji`, `emojiVariants`).
//! Custom emoji are kept among them by document id (`RecentEmojiDocument`).

use std::collections::{HashMap, HashSet};

/// `kRecentEmojiLimit`.
pub const LIMIT: usize = 54;

/// A recent emoji: one of Telegram's or a custom one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Emoji(u16),
    Custom(i64),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recent {
    /// Emoji and their ratings, the most used first.
    list: Vec<(Entry, u32)>,
    /// Default recent emoji the user removed (`_recentEmojiSkip`, by id).
    skip: HashSet<String>,
    /// Chosen variants by the uncolored emoji's id; "" for all emoji.
    variants: HashMap<String, usize>,
    resolved: bool,
}

impl Recent {
    /// `resolveRecentEmoji`: the stored list completed with the default one.
    fn resolve(&mut self) {
        if self.resolved {
            return;
        }
        self.resolved = true;
        for emoji in crate::default_recent() {
            if self.list.len() >= LIMIT {
                break;
            }
            let id = crate::get(emoji)
                .map(|e| e.id().to_string())
                .unwrap_or_default();
            if self.skip.contains(&id) || self.list.iter().any(|(e, _)| *e == Entry::Emoji(emoji)) {
                continue;
            }
            self.list.push((Entry::Emoji(emoji), 1));
        }
    }

    /// The recent emoji of Telegram's (custom ones left out).
    pub fn list(&mut self) -> Vec<u16> {
        self.entries()
            .into_iter()
            .filter_map(|e| match e {
                Entry::Emoji(e) => Some(e),
                Entry::Custom(_) => None,
            })
            .collect()
    }

    /// All recent emoji, custom ones too.
    pub fn entries(&mut self) -> Vec<Entry> {
        self.resolve();
        self.list.iter().map(|(e, _)| *e).take(LIMIT).collect()
    }

    /// `incrementRecentEmoji` for a custom emoji.
    pub fn increment_custom(&mut self, id: i64) {
        self.resolve();
        self.bump(Entry::Custom(id));
    }

    /// `hideRecentEmoji` for a custom emoji.
    pub fn hide_custom(&mut self, id: i64) {
        self.resolve();
        self.list.retain(|(e, _)| *e != Entry::Custom(id));
    }

    fn bump(&mut self, entry: Entry) {
        if let Some(mut i) = self.list.iter().position(|(e, _)| *e == entry) {
            self.list[i].1 += 1;
            if self.list[i].1 > 0x8000 {
                for (_, rating) in &mut self.list {
                    *rating = if *rating > 1 { *rating / 2 } else { 1 };
                }
            }
            while i > 0 && self.list[i - 1].1 <= self.list[i].1 {
                self.list.swap(i, i - 1);
                i -= 1;
            }
        } else {
            self.list.push((entry, 1));
            let mut i = self.list.len() - 1;
            while i > 0 && self.list[i - 1].1 <= self.list[i].1 {
                self.list.swap(i, i - 1);
                i -= 1;
            }
            while self.list.len() >= LIMIT {
                self.list.pop();
            }
        }
    }

    /// `incrementRecentEmoji`.
    pub fn increment(&mut self, emoji: u16) {
        self.resolve();
        if let Some(one) = crate::get(emoji) {
            self.skip.remove(one.id());
        }
        self.bump(Entry::Emoji(emoji));
    }

    /// `hideRecentEmoji`: removed, and a default one is not added back.
    pub fn hide(&mut self, emoji: u16) {
        self.resolve();
        self.list.retain(|(e, _)| *e != Entry::Emoji(emoji));
        if crate::default_recent().contains(&emoji)
            && let Some(one) = crate::get(emoji)
        {
            self.skip.insert(one.id().to_string());
        }
    }

    /// `resetRecentEmoji`: back to the default list.
    pub fn reset(&mut self) {
        self.list.clear();
        self.skip.clear();
        self.resolved = false;
    }

    /// `lookupEmojiVariant`: the chosen skin tone of an emoji with variants.
    pub fn variant_of(&self, emoji: u16) -> u16 {
        let Some(one) = crate::get(emoji) else {
            return emoji;
        };
        if !one.has_variants() {
            return emoji;
        }
        let original = crate::get(one.original())
            .map(|o| o.id())
            .unwrap_or_default();
        match self
            .variants
            .get(original)
            .or_else(|| self.variants.get(""))
        {
            Some(index) => one.variant(*index),
            None => emoji,
        }
    }

    /// `hasChosenEmojiVariant`.
    pub fn has_chosen_variant(&self, emoji: u16) -> bool {
        let original = crate::get(emoji)
            .and_then(|one| crate::get(one.original()))
            .map(|o| o.id())
            .unwrap_or_default();
        self.variants.contains_key("") || self.variants.contains_key(original)
    }

    /// `saveEmojiVariant`.
    pub fn save_variant(&mut self, emoji: u16) {
        if let Some(one) = crate::get(emoji) {
            let original = one.original();
            if let Some(o) = crate::get(original) {
                self.variants
                    .insert(o.id().to_string(), usize::from(emoji - original));
            }
        }
    }

    /// `saveAllEmojiVariants`.
    pub fn save_all_variants(&mut self, emoji: u16) {
        if let Some(one) = crate::get(emoji) {
            self.variants.clear();
            self.variants
                .insert(String::new(), usize::from(emoji - one.original()));
        }
    }

    /// The stored form: one entry per line.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for (entry, rating) in &self.list {
            match entry {
                Entry::Emoji(emoji) => {
                    if let Some(one) = crate::get(*emoji) {
                        out.push_str(&format!("recent\t{}\t{rating}\n", one.text()));
                    }
                }
                Entry::Custom(id) => out.push_str(&format!("custom\t{id}\t{rating}\n")),
            }
        }
        for id in &self.skip {
            out.push_str(&format!("skip\t{id}\n"));
        }
        for (id, index) in &self.variants {
            out.push_str(&format!("variant\t{id}\t{index}\n"));
        }
        out
    }

    pub fn parse(text: &str) -> Self {
        let mut result = Self::default();
        for line in text.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                ["recent", emoji, rating] => {
                    if let (Some(e), Ok(r)) = (crate::by_text(emoji), rating.parse())
                        && !result.list.iter().any(|(x, _)| *x == Entry::Emoji(e))
                    {
                        result.list.push((Entry::Emoji(e), r));
                    }
                }
                ["custom", id, rating] => {
                    if let (Ok(id), Ok(r)) = (id.parse::<i64>(), rating.parse())
                        && !result.list.iter().any(|(x, _)| *x == Entry::Custom(id))
                    {
                        result.list.push((Entry::Custom(id), r));
                    }
                }
                ["skip", id] => {
                    result.skip.insert((*id).to_string());
                }
                ["variant", id, index] => {
                    if let Ok(i) = index.parse::<usize>()
                        && i <= 5
                    {
                        result.variants.insert((*id).to_string(), i);
                    }
                }
                _ => {}
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_increment() {
        let mut r = Recent::default();
        let list = r.list();
        assert_eq!(list.len(), 34);
        let rocket = crate::by_text("🚀").unwrap();
        r.increment(rocket);
        r.increment(rocket);
        assert_eq!(r.list()[0], rocket);
        let first = list[0];
        r.hide(first);
        assert!(!r.list().contains(&first));
        let back = Recent::parse(&r.serialize());
        let mut back = back;
        assert_eq!(back.list()[0], rocket);
        assert!(!back.list().contains(&first));
    }

    #[test]
    fn variants() {
        let mut r = Recent::default();
        let thumbs = crate::by_text("👍").unwrap();
        assert!(!r.has_chosen_variant(thumbs));
        let dark = crate::get(thumbs).unwrap().variant(5);
        r.save_variant(dark);
        assert!(r.has_chosen_variant(thumbs));
        assert_eq!(r.variant_of(thumbs), dark);
        r.save_all_variants(crate::get(thumbs).unwrap().variant(2));
        let wave = crate::by_text("👋").unwrap();
        assert_eq!(r.variant_of(wave), crate::get(wave).unwrap().variant(2));
    }

    #[test]
    fn custom_emoji_are_recent_too() {
        let mut r = Recent::default();
        r.increment_custom(42);
        r.increment_custom(42);
        assert_eq!(r.entries()[0], Entry::Custom(42));
        assert!(!r.list().is_empty());
        let mut back = Recent::parse(&r.serialize());
        assert_eq!(back.entries()[0], Entry::Custom(42));
        back.hide_custom(42);
        assert!(!back.entries().contains(&Entry::Custom(42)));
    }
}
