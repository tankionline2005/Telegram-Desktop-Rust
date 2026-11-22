//! Recent emoji and chosen skin tones (`Core::Settings`: `recentEmoji`,
//! `incrementRecentEmoji`, `hideRecentEmoji`, `resetRecentEmoji`, `emojiVariants`).
//! Custom emoji are not kept here yet.

use std::collections::{HashMap, HashSet};

/// `kRecentEmojiLimit`.
pub const LIMIT: usize = 54;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recent {
    /// Emoji and their ratings, the most used first.
    list: Vec<(u16, u32)>,
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
            if self.skip.contains(&id) || self.list.iter().any(|(e, _)| *e == emoji) {
                continue;
            }
            self.list.push((emoji, 1));
        }
    }

    pub fn list(&mut self) -> Vec<u16> {
        self.resolve();
        self.list.iter().map(|(e, _)| *e).take(LIMIT).collect()
    }

    /// `incrementRecentEmoji`.
    pub fn increment(&mut self, emoji: u16) {
        self.resolve();
        if let Some(one) = crate::get(emoji) {
            self.skip.remove(one.id());
        }
        if let Some(mut i) = self.list.iter().position(|(e, _)| *e == emoji) {
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
            self.list.push((emoji, 1));
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

    /// `hideRecentEmoji`: removed, and a default one is not added back.
    pub fn hide(&mut self, emoji: u16) {
        self.resolve();
        self.list.retain(|(e, _)| *e != emoji);
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
        for (emoji, rating) in &self.list {
            if let Some(one) = crate::get(*emoji) {
                out.push_str(&format!("recent\t{}\t{rating}\n", one.text()));
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
                        && !result.list.iter().any(|(x, _)| *x == e)
                    {
                        result.list.push((e, r));
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
}
