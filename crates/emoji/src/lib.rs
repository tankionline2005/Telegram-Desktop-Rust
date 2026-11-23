//! Telegram's emoji (lib_ui `Ui::Emoji`): the list in the order of tdesktop's sprite
//! images, the picker's sections, the search of emoji in text and the text
//! replacements (`:-)`). The list is prepared from lib_ui's `emoji.txt` the way
//! codegen's `emoji/data.cpp` does at tdesktop's build time.

mod read;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use read::{InputData, InputId, read_data};

const POSTFIX: u32 = 0xFE0F;
const COLORS: [u32; 5] = [0xD83CDFFB, 0xD83CDFFC, 0xD83CDFFD, 0xD83CDFFE, 0xD83CDFFF];
const COLOR_MASK: u32 = 0xD83CDFFB;

/// Images in a row of a sprite and rows in a sprite (`kImagesPerRow`,
/// `kImageRowsPerSprite`); each image is `UNIVERSAL_SIZE` pixels.
pub const IMAGES_PER_ROW: usize = 32;
pub const ROWS_PER_SPRITE: usize = 16;
pub const UNIVERSAL_SIZE: u32 = 72;
pub const SPRITES: usize = 8;

/// The text replacements (codegen's `Replaces`).
const REPLACES: &[(u32, &str)] = &[
    (0xD83DDE0A, ":-)"),
    (0xD83DDE0D, "8-)"),
    (0x2764, "<3"),
    (0xD83DDE1A, ":-*"),
    (0xD83DDE0E, "B-)"),
    (0xD83DDE03, ":-D"),
    (0xD83DDE09, ";-)"),
    (0xD83DDE1C, ";-P"),
    (0xD83DDE0B, ":-p"),
    (0xD83DDE14, "3("),
    (0xD83DDE1E, ":-("),
    (0xD83DDE0F, ":]"),
    (0xD83DDE22, ":'("),
    (0xD83DDE2D, ":_("),
    (0xD83DDE29, ":(("),
    (0xD83DDE10, ":|"),
    (0xD83DDE0C, "3-)"),
    (0xD83DDE20, ">("),
    (0xD83DDE21, ">(("),
    (0xD83DDE07, "O:)"),
    (0xD83DDE30, ";o"),
    (0xD83DDE33, "8|"),
    (0xD83DDE32, "8o"),
    (0xD83DDE37, ":X"),
    (0xD83DDE08, "}:)"),
];

/// Shown only with the postfix (`PostfixRequired`): ™, ©, ®.
const POSTFIX_REQUIRED: [[u32; 2]; 3] = [[0x2122, POSTFIX], [0xA9, POSTFIX], [0xAE, POSTFIX]];

/// Flags sent as one string but drawn as another (`FlagAliases`).
const FLAG_ALIASES: [([u32; 2], [u32; 2]); 1] =
    [([0xD83CDDE8, 0xD83CDDF5], [0xD83CDDEB, 0xD83CDDF7])];

/// `Ui::Emoji::One`.
#[derive(Debug, Clone)]
pub struct One {
    id: String,
    original: Option<u16>,
    index: u16,
    postfix: bool,
    colorizable: bool,
}

impl One {
    /// The emoji without its trailing variation selector.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The text put into a message: with the postfix where the emoji has it.
    pub fn text(&self) -> String {
        if self.postfix {
            format!("{}\u{FE0F}", self.id)
        } else {
            self.id.clone()
        }
    }

    pub fn index(&self) -> u16 {
        self.index
    }

    pub fn colored(&self) -> bool {
        self.original.is_some()
    }

    /// The uncolored emoji of a colored one (itself otherwise).
    pub fn original(&self) -> u16 {
        self.original.unwrap_or(self.index)
    }

    /// Has skin tone variants (`hasVariants`).
    pub fn has_variants(&self) -> bool {
        self.colorizable || self.colored()
    }

    /// The variant `index` (0 — no color, 1..=5 — the skin tones).
    pub fn variant(&self, index: usize) -> u16 {
        if self.has_variants() && index <= 5 {
            self.original() + index as u16
        } else {
            self.index
        }
    }

    pub fn sprite(&self) -> usize {
        usize::from(self.index >> 9)
    }

    pub fn row(&self) -> usize {
        usize::from((self.index >> 5) & 0x0F)
    }

    pub fn column(&self) -> usize {
        usize::from(self.index & 0x1F)
    }
}

/// The picker's sections (`Ui::Emoji::Section` without Recent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    People,
    Nature,
    Food,
    Activity,
    Travel,
    Objects,
    Symbols,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::People,
        Section::Nature,
        Section::Food,
        Section::Activity,
        Section::Travel,
        Section::Objects,
        Section::Symbols,
    ];
}

#[derive(Default)]
struct Node {
    children: HashMap<u16, u32>,
    value: Option<u16>,
}

struct Data {
    list: Vec<One>,
    trie: Vec<Node>,
    postfix_required: HashSet<u16>,
    sections: Vec<Vec<u16>>,
    replaces: Vec<(&'static str, u16)>,
}

fn append(id: &mut Vec<u16>, code: u32) {
    let first = (code >> 16) as u16;
    if first != 0 {
        id.push(first);
    }
    id.push((code & 0xFFFF) as u16);
}

fn bare_id(input: &[u32]) -> Vec<u16> {
    let mut result = Vec::new();
    for &code in input {
        if code != POSTFIX {
            append(&mut result, code);
        }
    }
    result
}

struct Emoji {
    id: Vec<u16>,
    postfixed: bool,
    variated: bool,
    colored: bool,
}

struct DoubleTemplates {
    same: InputId,
    different: InputId,
}

#[derive(Default)]
struct Prepared {
    list: Vec<Emoji>,
    map: HashMap<Vec<u16>, usize>,
    postfix_required: HashSet<usize>,
    categories: Vec<Vec<usize>>,
    aliases: HashMap<Vec<u16>, Vec<Vec<u16>>>,
}

impl Prepared {
    fn add_alias(&mut self, original: Vec<u16>, aliased: Vec<u16>) {
        let list = self.aliases.entry(original).or_default();
        if !list.contains(&aliased) {
            list.push(aliased);
        }
    }

    /// `addOne`: the index of the emoji, added if new.
    fn add_one(
        &mut self,
        bare: Vec<u16>,
        emoji: Emoji,
        postfix_required: &HashSet<Vec<u16>>,
    ) -> usize {
        if let Some(&index) = self.map.get(&bare) {
            return index;
        }
        let index = self.list.len();
        self.map.insert(bare.clone(), index);
        self.list.push(emoji);
        if let Some(aliases) = self.aliases.get(&bare) {
            for alias in aliases.clone() {
                self.map.entry(alias).or_insert(index);
            }
        }
        if postfix_required.contains(&bare) {
            self.postfix_required.insert(index);
        }
        index
    }

    fn append_category(
        &mut self,
        category: &[InputId],
        variated: &HashSet<Vec<u16>>,
        double_variated: &HashMap<Vec<u16>, DoubleTemplates>,
        postfix_required: &HashSet<Vec<u16>>,
    ) {
        self.categories.push(Vec::new());
        for input in category {
            let bare = bare_id(input);
            let mut emoji = Emoji {
                id: Vec::new(),
                postfixed: false,
                variated: false,
                colored: false,
            };
            let mut to = input.len();
            if to == 2 && input[1] == POSTFIX {
                emoji.postfixed = true;
                to -= 1;
            }
            for &code in &input[..to] {
                append(&mut emoji.id, code);
            }
            let index = self.add_one(bare.clone(), emoji, postfix_required);
            if variated.contains(&bare) {
                self.list[index].variated = true;
                let mut from = 0;
                let mut base = Vec::new();
                append(&mut base, input[from]);
                from += 1;
                // A few uncolored emoji contain two postfixes, the colored ones one.
                if input[from..to].iter().filter(|c| **c == POSTFIX).count() == 2
                    && input[from] == POSTFIX
                {
                    from += 1;
                }
                for color in COLORS {
                    let mut id = base.clone();
                    append(&mut id, color);
                    let mut bare_colored = id.clone();
                    for &code in &input[from..to] {
                        append(&mut id, code);
                        if code != POSTFIX {
                            append(&mut bare_colored, code);
                        }
                    }
                    let colored = Emoji {
                        id,
                        postfixed: false,
                        variated: false,
                        colored: true,
                    };
                    self.add_one(bare_colored, colored, postfix_required);
                }
            } else if let Some(d) = double_variated.get(&bare) {
                for color1 in COLORS {
                    for color2 in COLORS {
                        let pattern = if color1 == color2 {
                            &d.same
                        } else {
                            &d.different
                        };
                        let mut id = Vec::new();
                        for &code in pattern {
                            if code != POSTFIX {
                                let code = if code == COLORS[0] {
                                    color1
                                } else if code == COLORS[1] {
                                    color2
                                } else {
                                    code
                                };
                                append(&mut id, code);
                            }
                        }
                        let colored = Emoji {
                            id: id.clone(),
                            postfixed: pattern.last() == Some(&POSTFIX),
                            variated: false,
                            colored: false,
                        };
                        self.add_one(id, colored, postfix_required);
                    }
                }
            }
            self.categories.last_mut().unwrap().push(index);
        }
    }
}

/// `FillVariatedIds`.
fn variated_ids(data: &InputData) -> HashSet<Vec<u16>> {
    data.colored
        .iter()
        .map(|row| {
            let mut id = Vec::new();
            for (i, &code) in row.iter().enumerate() {
                if i != 1 && code != POSTFIX {
                    append(&mut id, code);
                }
            }
            id
        })
        .collect()
}

/// `FillDoubleVariatedIds` (with its aliases).
fn double_variated_ids(
    data: &InputData,
    prepared: &mut Prepared,
) -> HashMap<Vec<u16>, DoubleTemplates> {
    let mut result = HashMap::new();
    for d in &data.double_colored {
        let (original, same, different) = (&d.original, &d.same, &d.different);
        if same.iter().filter(|c| **c == COLOR_MASK).count() == 1 {
            for color in COLORS {
                let mut copy = same.clone();
                copy[1] = color;
                let same_with = bare_id(&copy);
                let mut copy = different.clone();
                for entry in &mut copy {
                    if *entry == COLORS[0] || *entry == COLORS[1] {
                        *entry = color;
                    }
                }
                prepared.add_alias(same_with, bare_id(&copy));
            }
        } else if original.len() == 1 {
            for color in COLORS {
                let mut copy = original.clone();
                copy.push(color);
                let original_with = bare_id(&copy);
                let mut copy = same.clone();
                for entry in &mut copy {
                    if *entry == COLOR_MASK {
                        *entry = color;
                    }
                }
                prepared.add_alias(original_with, bare_id(&copy));
            }
        }
        let same_template = if original.len() == 1 {
            vec![original[0], COLOR_MASK]
        } else {
            same.clone()
        };
        result.insert(
            bare_id(original),
            DoubleTemplates {
                same: same_template,
                different: different.clone(),
            },
        );
    }
    result
}

fn prepare() -> Data {
    let input = read_data(include_str!("../../../assets/emoji/emoji.txt"))
        .expect("lib_ui's emoji.txt is valid");
    let mut prepared = Prepared::default();
    let variated = variated_ids(&input);
    let double_variated = double_variated_ids(&input, &mut prepared);
    let postfix_required: HashSet<Vec<u16>> = POSTFIX_REQUIRED.iter().map(|r| bare_id(r)).collect();
    for (from, to) in FLAG_ALIASES {
        prepared.add_alias(bare_id(&to), bare_id(&from));
    }
    for category in &input.categories {
        prepared.append_category(category, &variated, &double_variated, &postfix_required);
    }
    prepared.append_category(&input.other, &variated, &double_variated, &postfix_required);

    // `enumerateWholeList`: a colored emoji points to the variated one before it.
    let mut list = Vec::with_capacity(prepared.list.len());
    let mut last_variated = None;
    for (index, e) in prepared.list.iter().enumerate() {
        if e.variated {
            last_variated = Some(index as u16);
        } else if !e.colored {
            last_variated = None;
        }
        list.push(One {
            id: String::from_utf16_lossy(&e.id),
            original: if e.colored { last_variated } else { None },
            index: index as u16,
            postfix: e.postfixed,
            colorizable: e.variated,
        });
    }

    let mut trie = vec![Node::default()];
    for (key, &index) in &prepared.map {
        let mut node = 0usize;
        for &unit in key {
            let next = trie.len() as u32;
            let child = *trie[node].children.entry(unit).or_insert(next);
            if child == next {
                trie.push(Node::default());
            }
            node = child as usize;
        }
        trie[node].value = Some(index as u16);
    }

    let replaces = REPLACES
        .iter()
        .filter_map(|(code, text)| {
            let mut id = Vec::new();
            append(&mut id, *code);
            prepared.map.get(&id).map(|i| (*text, *i as u16))
        })
        .collect();

    Data {
        list,
        trie,
        postfix_required: prepared
            .postfix_required
            .iter()
            .map(|i| *i as u16)
            .collect(),
        sections: prepared
            .categories
            .iter()
            .map(|c| c.iter().map(|i| *i as u16).collect())
            .collect(),
        replaces,
    }
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(prepare)
}

/// Prepares the list now (it is otherwise prepared on first use).
pub fn init() {
    data();
}

/// All emoji, by index.
pub fn all() -> &'static [One] {
    &data().list
}

pub fn get(index: u16) -> Option<&'static One> {
    data().list.get(usize::from(index))
}

/// The emoji of a picker section, in order.
pub fn section(section: Section) -> &'static [u16] {
    let index = Section::ALL.iter().position(|s| *s == section).unwrap();
    &data().sections[index]
}

/// `Ui::Emoji::Find`: the emoji at the start of `text` and its length in bytes (a
/// variation selector after any of its characters is taken with it), the longest
/// one if several match.
pub fn find(text: &str) -> Option<(u16, usize)> {
    let data = data();
    let mut node = 0usize;
    let mut found = None;
    let mut chars = text.char_indices().peekable();
    let mut buf = [0u16; 2];
    while let Some((at, ch)) = chars.next() {
        let mut next = Some(node);
        for unit in ch.encode_utf16(&mut buf).iter() {
            next = next.and_then(|n| data.trie[n].children.get(unit).map(|c| *c as usize));
        }
        let Some(n) = next else {
            break;
        };
        node = n;
        let mut end = at + ch.len_utf8();
        let mut postfixed = false;
        if let Some(&(_, '\u{FE0F}')) = chars.peek() {
            chars.next();
            end += 3;
            postfixed = true;
        }
        if let Some(value) = data.trie[node].value {
            if data.postfix_required.contains(&value) && !postfixed {
                return None;
            }
            found = Some((value, end));
        }
    }
    found
}

/// Splits `text` into runs of text and emoji: `(byte range, Some(emoji))`.
pub fn split(text: &str) -> Vec<(std::ops::Range<usize>, Option<u16>)> {
    let mut out = Vec::new();
    let mut plain = 0;
    let mut at = 0;
    while at < text.len() {
        if let Some((index, len)) = find(&text[at..]) {
            if plain < at {
                out.push((plain..at, None));
            }
            out.push((at..at + len, Some(index)));
            at += len;
            plain = at;
        } else {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if plain < text.len() {
        out.push((plain..text.len(), None));
    }
    out
}

/// `Ui::Emoji::FindReplace`: a text replacement (`:-)`) ending at the end of `text`,
/// as (its length in bytes, the emoji).
pub fn find_replace_at_end(text: &str) -> Option<(usize, u16)> {
    data()
        .replaces
        .iter()
        .filter(|(r, _)| text.ends_with(r))
        .max_by_key(|(r, _)| r.len())
        .map(|(r, i)| (r.len(), *i))
}

/// The text replacements and their emoji.
pub fn replaces() -> &'static [(&'static str, u16)] {
    &data().replaces
}

/// `GetDefaultRecent`.
pub fn default_recent() -> Vec<u16> {
    const KEYS: [u32; 34] = [
        0xD83DDE02, 0xD83DDE18, 0x2764, 0xD83DDE0D, 0xD83DDE0A, 0xD83DDE01, 0xD83DDC4D, 0x263A,
        0xD83DDE14, 0xD83DDE04, 0xD83DDE2D, 0xD83DDC8B, 0xD83DDE12, 0xD83DDE33, 0xD83DDE1C,
        0xD83DDE48, 0xD83DDE09, 0xD83DDE03, 0xD83DDE22, 0xD83DDE1D, 0xD83DDE31, 0xD83DDE21,
        0xD83DDE0F, 0xD83DDE1E, 0xD83DDE05, 0xD83DDE1A, 0xD83DDE4A, 0xD83DDE0C, 0xD83DDE00,
        0xD83DDE0B, 0xD83DDE06, 0xD83DDC4C, 0xD83DDE10, 0xD83DDE15,
    ];
    KEYS.iter()
        .filter_map(|code| {
            let mut id = Vec::new();
            append(&mut id, *code);
            let text = String::from_utf16_lossy(&id);
            find(&text).map(|(i, _)| i)
        })
        .collect()
}

/// The emoji of a text (an exact match of the whole string).
pub fn by_text(text: &str) -> Option<u16> {
    find(text)
        .filter(|(_, len)| *len == text.len())
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_fits_the_sprites() {
        let count = all().len();
        assert!(count > 3000, "{count}");
        assert!(
            count <= SPRITES * IMAGES_PER_ROW * ROWS_PER_SPRITE,
            "{count}"
        );
    }

    #[test]
    fn first_emoji_and_colors() {
        let grinning = get(0).unwrap();
        assert_eq!(grinning.id(), "😀");
        let (thumbs, len) = find("👍🏽 ok").unwrap();
        assert_eq!(len, "👍🏽".len());
        let one = get(thumbs).unwrap();
        assert!(one.colored());
        let original = get(one.original()).unwrap();
        assert_eq!(original.id(), "👍");
        assert_eq!(original.variant(3), thumbs);
    }

    #[test]
    fn postfix_and_sequences() {
        let (heart, len) = find("❤️x").unwrap();
        assert_eq!(len, "❤️".len());
        assert_eq!(get(heart).unwrap().text(), "❤️");
        // Without the postfix it is the same emoji.
        assert_eq!(find("❤").map(|f| f.0), Some(heart));
        // ZWJ sequence: the longest match.
        let family = "👨‍👩‍👧";
        assert_eq!(find(family).map(|f| f.1), Some(family.len()));
        // © only with the postfix.
        assert!(find("©").is_none());
        assert!(find("©\u{FE0F}").is_some());
        assert!(find("a").is_none());
    }

    #[test]
    fn sections_and_recent() {
        assert_eq!(section(Section::People)[0], 0);
        for s in Section::ALL {
            assert!(!section(s).is_empty());
        }
        assert_eq!(default_recent().len(), 34);
        assert_eq!(find_replace_at_end("hi :-)").map(|r| r.0), Some(3));
    }

    #[test]
    fn split_text() {
        let parts = split("a😀b");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].1, Some(0));
    }
}

pub mod keywords;
pub mod recent;
pub mod suggestions;
