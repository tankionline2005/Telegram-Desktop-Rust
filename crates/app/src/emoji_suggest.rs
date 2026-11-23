//! Emoji suggestions while typing (tdesktop's `Ui::Emoji::SuggestionsController`
//! and `SuggestionsWidget`): the query found in the field (`getEmojiQuery`), the
//! row of emoji over it with its selection and scroll, and the languages'
//! keywords (`ChatHelpers::EmojiKeywords`, kept in files beside the recent emoji).
//! The field (`text_input`) finds queries and takes the keys; `MainView` fills the
//! rows (`rows_for`) and draws them (`ui/emoji_suggest_box.rs`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tdr_emoji::keywords::{self, LangPack};

/// `kShowExactDelay`: a whole-field word waits this long before showing.
pub const SHOW_EXACT_DELAY: Duration = Duration::from_millis(300);
/// `kMaxNonScrolledEmoji`.
pub const MAX_NON_SCROLLED: usize = 7;

/// What the field asks for (`SuggestionsQuery`).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Query {
    #[default]
    None,
    /// The text after and with `:` (`:smi`), or the whole field's one word.
    Text(String),
    /// The caret stands after `count` copies of an emoji: custom emoji for it.
    Emoji { emoji: u16, count: usize },
}

/// One suggestion: an emoji, or a custom emoji over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub emoji: u16,
    pub custom: Option<i64>,
    pub replacement: String,
}

/// What a key did.
pub enum KeyResult {
    /// Not for the suggestions: the field handles it.
    Ignored,
    Consumed,
    /// Replace the query with this row.
    Chosen(Row),
}

/// The suggestions' state, shared by the field and the view.
#[derive(Default)]
pub struct Suggest {
    pub query: Query,
    /// Where the query starts in the field (bytes) and where the box points.
    pub query_start: usize,
    pub anchor: usize,
    pub rows: Vec<Row>,
    pub selected: Option<usize>,
    /// The selection's background moves from here (rows, -1 for none).
    pub selected_from: (f32, Option<Instant>),
    pub scroll: f32,
    pub scroll_from: (f32, Option<Instant>),
    pub scroll_max: f32,
    pub pressed: Option<usize>,
    /// Shown (with the moment it was toggled, for the fade).
    pub shown: bool,
    pub toggled: Option<Instant>,
    /// The field has the focus (hidden at once without it, `updateForceHidden`).
    pub focused: bool,
    /// The rows must be filled for `query` (by the view).
    pub dirty: bool,
    /// A whole-field word is shown at this moment.
    pub exact_due: Option<Instant>,
    /// The keywords changed: show the last query again.
    pub refreshed: bool,
    /// A whole-field word waiting for `exact_due`.
    pending: Option<(Query, usize, usize)>,
    /// A `:` query selects its first row (`selectFirstResult`).
    select_first: bool,
}

/// `universalDuration`.
pub const ANIM_MS: f32 = 120.;

impl Suggest {
    /// `handleTextChange`: the field's text changed (`by_key`: typed).
    pub fn text_changed(&mut self, query: Query, query_start: usize, anchor: usize, by_key: bool) {
        match &query {
            Query::Emoji { .. } => self.show_query(query, query_start, anchor),
            Query::Text(text) if by_key => {
                if text.starts_with(':') {
                    self.show_query(query, query_start, anchor);
                    self.select_first = true;
                } else {
                    let hidden = !self.shown;
                    self.pending = Some((query, query_start, anchor));
                    self.exact_due = Some(
                        Instant::now()
                            + if hidden {
                                SHOW_EXACT_DELAY
                            } else {
                                Duration::ZERO
                            },
                    );
                }
            }
            Query::None => self.show_query(Query::None, 0, 0),
            Query::Text(_) => {}
        }
    }

    /// `handleCursorPositionChange`: the caret moved without typing.
    pub fn cursor_moved(&mut self) {
        self.show_query(Query::None, 0, 0);
    }

    fn show_query(&mut self, query: Query, query_start: usize, anchor: usize) {
        self.exact_due = None;
        self.pending = None;
        if self.query == query && !self.refreshed {
            return;
        }
        self.query = query;
        self.query_start = query_start;
        self.anchor = anchor;
        self.dirty = true;
    }

    /// The delayed whole-field word, when it is due.
    pub fn take_due(&mut self) -> bool {
        if self.exact_due.is_some_and(|d| Instant::now() >= d)
            && let Some((q, s, a)) = self.pending.take()
        {
            self.exact_due = None;
            self.query = Query::None;
            self.show_query(q, s, a);
            return true;
        }
        false
    }

    /// `showWithQuery`'s end: the rows for the query came.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.dirty = false;
        self.refreshed = false;
        self.selected = None;
        self.selected_from = (-1., None);
        self.pressed = None;
        self.rows = rows;
        let count = self.rows.len();
        self.scroll_max = if count > MAX_NON_SCROLLED {
            (count as f32 - (SCROLLED_WIDTH / SIZE)).max(0.)
        } else {
            0.
        };
        self.scroll = self.scroll.min(self.scroll_max);
        let shown = !self.rows.is_empty();
        if shown != self.shown {
            self.shown = shown;
            self.toggled = Some(Instant::now());
        }
        if std::mem::take(&mut self.select_first) && shown {
            self.set_selected(Some(0), false);
        }
    }

    /// Hides (Escape).
    pub fn hide(&mut self) {
        self.show_query(Query::None, 0, 0);
    }

    pub fn visible(&self) -> bool {
        self.shown && self.focused && !self.rows.is_empty()
    }

    /// `setSelected`: moving by keys animates the selection and scrolls.
    pub fn set_selected(&mut self, selected: Option<usize>, animated: bool) {
        let selected = selected.filter(|s| *s < self.rows.len());
        if animated {
            let from = self.selected_now();
            self.selected_from = (from, Some(Instant::now()));
            if self.scroll_max > 0. {
                let max = self.rows.len() as f32 - 3.;
                let s = selected.map_or(-1., |s| s as f32);
                let for_scroll = (s.max(1.) - 1.).min(max);
                let to = self.scroll_max * for_scroll / max;
                self.scroll_from = (self.scroll_now(), Some(Instant::now()));
                self.scroll = to;
            }
        } else {
            self.selected_from = (selected.map_or(-1., |s| s as f32), None);
        }
        self.selected = selected;
    }

    fn progress(at: Option<Instant>) -> Option<f32> {
        let t = at?.elapsed().as_secs_f32() * 1000. / ANIM_MS;
        (t < 1.).then(|| crate::anim::sine_in_out(t))
    }

    /// The selection's place now (rows), -1 for none.
    pub fn selected_now(&self) -> f32 {
        let to = self.selected.map_or(-1., |s| s as f32);
        match Self::progress(self.selected_from.1) {
            Some(t) => self.selected_from.0 + (to - self.selected_from.0) * t,
            None => to,
        }
    }

    /// The scroll now (rows).
    pub fn scroll_now(&self) -> f32 {
        match Self::progress(self.scroll_from.1) {
            Some(t) => self.scroll_from.0 + (self.scroll - self.scroll_from.0) * t,
            None => self.scroll,
        }
    }

    pub fn animating(&self) -> bool {
        Self::progress(self.selected_from.1).is_some()
            || Self::progress(self.scroll_from.1).is_some()
    }

    /// `handleKeyEvent` for Enter, Tab and the arrows (`delta` -1 or 1).
    pub fn key(&mut self, key: Key) -> KeyResult {
        if !self.visible() {
            return KeyResult::Ignored;
        }
        let chosen = |s: &Self| {
            s.selected
                .and_then(|k| s.rows.get(k).cloned())
                .map_or(KeyResult::Ignored, KeyResult::Chosen)
        };
        match key {
            Key::Enter => chosen(self),
            Key::Tab => {
                if self.selected.is_none() {
                    self.set_selected(Some(0), false);
                }
                chosen(self)
            }
            Key::Arrow(delta) => {
                if delta < 0 && self.selected.is_none() {
                    return KeyResult::Ignored;
                }
                let n = self.rows.len() as i32;
                let start = self
                    .selected
                    .map_or(if delta > 0 { n - 1 } else { 0 }, |s| s as i32);
                let mut next = start + delta;
                if next < 0 {
                    next = -1;
                } else if next >= n {
                    next -= n;
                }
                self.set_selected((next >= 0).then_some(next as usize), true);
                KeyResult::Consumed
            }
        }
    }
}

/// Keys the suggestions take.
#[derive(Clone, Copy)]
pub enum Key {
    Enter,
    Tab,
    Arrow(i32),
}

/// `emojiSuggestionSize`, `emojiSuggestionsScrolledWidth` (style pixels).
pub const SIZE: f32 = 40.;
pub const SCROLLED_WIDTH: f32 = 240.;

/// The languages' keywords (`EmojiKeywords::_data`) and when they were asked.
#[derive(Default)]
pub struct Keywords {
    pub packs: Vec<(String, LangPack)>,
    /// The languages the server has (`_remoteList`).
    pub remote: Vec<String>,
    pub asked: Option<Instant>,
    pub dir: Option<std::path::PathBuf>,
}

pub static KEYWORDS: Mutex<Option<Keywords>> = Mutex::new(None);

pub fn with_keywords<R>(f: impl FnOnce(&mut Keywords) -> R) -> R {
    let mut guard = KEYWORDS.lock().unwrap();
    f(guard.get_or_insert_with(Keywords::default))
}

/// Reads the kept lang packs from `dir` (`ReadLocalCache`).
pub fn load_keywords(dir: std::path::PathBuf) {
    let mut packs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let path = e.path();
            let Some(lang) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Some(pack) = LangPack::parse(&text)
            {
                packs.push((lang, pack));
            }
        }
    }
    packs.sort_by(|a, b| a.0.cmp(&b.0));
    with_keywords(|k| {
        k.remote = packs.iter().map(|(l, _)| l.clone()).collect();
        k.packs = packs;
        k.dir = Some(dir);
    });
}

/// The languages to look for and the kept packs' versions.
pub type RefreshRequest = (Vec<String>, Vec<(String, i32)>);

/// `EmojiKeywords::refresh`: what to ask the server for, at most once an hour:
/// the languages to look for and the versions of the packs kept.
pub fn refresh_request() -> Option<RefreshRequest> {
    with_keywords(|k| {
        k.dir.as_ref()?;
        if k.asked
            .is_some_and(|a| a.elapsed().as_secs() < keywords::REFRESH_EACH_SECS)
        {
            return None;
        }
        k.asked = Some(Instant::now());
        let versions = k
            .packs
            .iter()
            .map(|(l, p)| (l.clone(), p.version))
            .collect();
        Some((languages(), versions))
    })
}

/// A lang pack's difference came (`applyDifference`); the server's languages.
pub fn apply_difference(
    remote: Vec<String>,
    lang: &str,
    version: i32,
    changes: Vec<tdr_emoji::keywords::Change>,
) {
    let save = with_keywords(|k| {
        k.remote = remote;
        let at = match k.packs.iter().position(|(l, _)| l == lang) {
            Some(at) => at,
            None => {
                k.packs.push((lang.to_string(), LangPack::default()));
                k.packs.sort_by(|a, b| a.0.cmp(&b.0));
                k.packs.iter().position(|(l, _)| l == lang)?
            }
        };
        let pack = &mut k.packs[at].1;
        if changes.is_empty() && pack.version >= version {
            return None;
        }
        pack.apply(version, changes);
        let dir = k.dir.clone()?;
        Some((dir.join(format!("{lang}.txt")), pack.serialize()))
    });
    if let Some((path, text)) = save {
        std::thread::spawn(move || {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = tdr_api::write_atomic(&path, text.as_bytes());
        });
    }
}

/// `queryMine` without the variants: keywords of the server's languages, then
/// short codes; the recent emoji first.
pub fn query(text: &str, exact: bool, recent: &[u16]) -> Vec<keywords::Found> {
    with_keywords(|k| {
        let packs: Vec<(String, LangPack)> = k
            .packs
            .iter()
            .filter(|(l, _)| k.remote.contains(l))
            .cloned()
            .collect();
        keywords::prioritize_recent(keywords::query(&packs, text, exact), recent)
    })
}

/// `maxQueryLength` of the kept lang packs.
pub fn max_query_length() -> usize {
    with_keywords(|k| keywords::max_query_length(&k.packs))
}

/// `EmojiKeywords::languages`: the interface's (English), the system's and the
/// keyboard layouts' languages, as `ru-RU` and `ru`.
pub fn languages() -> Vec<String> {
    let mut out = vec!["en".to_string()];
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::{
            GetUserPreferredUILanguages, LCIDToLocaleName, MUI_LANGUAGE_NAME,
        };
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
        let mut count = 0u32;
        let mut len = 0u32;
        // SAFETY: sizes first, then a buffer of that size.
        unsafe {
            if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len).is_ok() {
                let mut buf = vec![0u16; len as usize];
                if GetUserPreferredUILanguages(
                    MUI_LANGUAGE_NAME,
                    &mut count,
                    Some(windows::core::PWSTR(buf.as_mut_ptr())),
                    &mut len,
                )
                .is_ok()
                {
                    for name in buf.split(|c| *c == 0).filter(|s| !s.is_empty()) {
                        out.push(String::from_utf16_lossy(name));
                    }
                }
            }
            let n = GetKeyboardLayoutList(None);
            if n > 0 {
                let mut layouts = vec![Default::default(); n as usize];
                let n = GetKeyboardLayoutList(Some(&mut layouts));
                for hkl in layouts.iter().take(n.max(0) as usize) {
                    let langid = (hkl.0 as usize & 0xFFFF) as u32;
                    let mut name = [0u16; 85];
                    let len = LCIDToLocaleName(langid, Some(&mut name), 0);
                    if len > 1 {
                        out.push(String::from_utf16_lossy(&name[..len as usize - 1]));
                    }
                }
            }
        }
    }
    let mut all = Vec::new();
    for l in out {
        if let Some((base, _)) = l.split_once('-') {
            all.push(base.to_string());
        }
        all.push(l);
    }
    all.sort();
    all.dedup();
    all
}

/// `SearchEmoji` of the panel: each word of `query` is an emoji itself or is
/// looked up (`queryMine`, not exactly), 32 emoji at most; also the set of found
/// emoji with their originals (for custom emoji).
pub fn search_emoji(query: &str) -> (Vec<u16>, std::collections::HashSet<u16>) {
    const LIMIT: usize = 32;
    let recent = crate::emoji::recent_list();
    let mut result = Vec::new();
    let mut set = std::collections::HashSet::new();
    let mut push = |emoji: u16, result: &mut Vec<u16>| {
        if result.len() < LIMIT && set.insert(emoji) {
            result.push(emoji);
        }
        if let Some(one) = tdr_emoji::get(emoji) {
            set.insert(one.original());
        }
    };
    for word in query.split_whitespace() {
        if let Some(emoji) = tdr_emoji::by_text(word) {
            push(emoji, &mut result);
        } else {
            for found in self::query(&word.to_lowercase(), false, &recent) {
                push(crate::emoji::variant_of(found.emoji), &mut result);
                if result.len() >= LIMIT {
                    break;
                }
            }
        }
        if result.len() >= LIMIT {
            break;
        }
    }
    drop(push);
    (result, set)
}
