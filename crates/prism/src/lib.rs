//! Syntax highlighting of code blocks, as tdesktop does it: libprisma
//! (`Telegram/ThirdParty/libprisma`, a C++ port of Prism.js, MIT) runs Prism's
//! grammars (`grammars.dat`, generated from Prism's own sources) over the code, and
//! lib_spellcheck's `spellcheck_highlight_syntax.cpp` turns the token types into one
//! of eight colors.
//!
//! The grammars are JavaScript regular expressions; [`translate`] rewrites the few
//! JavaScript-only forms for `fancy-regex`. A pattern that still does not compile
//! never matches (its tokens stay plain), as one that throws in libprisma.
//!
//! Not ported: embedded languages (a diff naming the language of its lines).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use fancy_regex::Regex;

struct PatternRaw {
    source: String,
    icase: bool,
    multiline: bool,
    /// Prism's `lookbehind`: the first group is context, not part of the match.
    lookbehind: bool,
    greedy: bool,
    alias: String,
    inside: Option<usize>,
}

struct Grammar {
    /// Token name and its patterns, in order.
    tokens: Vec<(String, Vec<usize>)>,
}

pub struct Highlighter {
    patterns: Vec<PatternRaw>,
    compiled: Vec<OnceLock<Option<Regex>>>,
    grammars: Vec<Grammar>,
    languages: HashMap<String, usize>,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }

    fn u16(&mut self) -> u16 {
        u16::from(self.u8()) | (u16::from(self.u8()) << 8)
    }

    fn string(&mut self) -> String {
        let mut len = usize::from(self.u8());
        if len >= 254 {
            let (a, b, c) = (self.u8(), self.u8(), self.u8());
            len = usize::from(a) | (usize::from(b) << 8) | (usize::from(c) << 16);
        }
        let end = (self.at + len).min(self.data.len());
        let s = String::from_utf8_lossy(&self.data[self.at.min(end)..end]).into_owned();
        self.at = end;
        s
    }
}

/// tdesktop's colors (`QueuedHighlighter::process`): by token type, then by alias.
fn color_of(name: &str) -> Option<u8> {
    Some(match name {
        "comment" | "block-comment" | "prolog" | "doctype" | "cdata" => 1,
        "punctuation" => 2,
        "property" | "tag" | "boolean" | "number" | "constant" | "symbol" | "deleted" => 3,
        "selector" | "attr-name" | "string" | "char" | "builtin" => 4,
        "operator" | "entity" | "url" => 5,
        "atrule" | "attr-value" | "keyword" | "function" => 6,
        "class-name" => 7,
        "inserted" => 8,
        _ => return None,
    })
}

impl Highlighter {
    /// Reads `grammars.dat` (`LanguageTree::load`).
    pub fn load(data: &[u8]) -> Self {
        let mut r = Reader { data, at: 0 };
        let count = r.u16();
        let mut patterns = Vec::with_capacity(count.into());
        for _ in 0..count {
            let item = r.string();
            let (Some(beg), Some(end)) = (item.find('/'), item.rfind('/')) else {
                patterns.push(PatternRaw {
                    source: String::new(),
                    icase: false,
                    multiline: false,
                    lookbehind: false,
                    greedy: false,
                    alias: String::new(),
                    inside: None,
                });
                continue;
            };
            let source = item[beg + 1..end.max(beg + 1)].to_string();
            let options = &item[end + 1..];
            let alias_beg = options.find(',').unwrap_or(options.len());
            let alias_end = options.rfind(',').unwrap_or(options.len());
            let flags = &options[..alias_beg];
            let alias = options
                .get(alias_beg + 1..alias_end.max(alias_beg + 1))
                .unwrap_or("")
                .to_string();
            let inside = options
                .get(alias_end + 1..)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse().ok());
            patterns.push(PatternRaw {
                source,
                icase: flags.contains('i'),
                multiline: flags.contains('m'),
                lookbehind: flags.contains('l'),
                greedy: flags.contains('y'),
                alias,
                inside,
            });
        }
        let count = r.u16();
        let mut grammars = Vec::with_capacity(count.into());
        for _ in 0..count {
            let keys = r.u8();
            let mut tokens = Vec::with_capacity(keys.into());
            for _ in 0..keys {
                let name = r.string();
                let ids = r.u8();
                let list = (0..ids).map(|_| usize::from(r.u16())).collect();
                tokens.push((name, list));
            }
            grammars.push(Grammar { tokens });
        }
        let count = r.u16();
        let mut languages = HashMap::new();
        for _ in 0..count {
            let name = r.string();
            let _display = r.string();
            let index = usize::from(r.u16());
            languages.entry(name).or_insert(index);
        }
        let compiled = (0..patterns.len()).map(|_| OnceLock::new()).collect();
        Self {
            patterns,
            compiled,
            grammars,
            languages,
        }
    }

    fn regex(&self, pattern: usize) -> Option<&Regex> {
        self.compiled[pattern]
            .get_or_init(|| {
                let raw = &self.patterns[pattern];
                let mut src = String::new();
                if raw.icase {
                    src.push_str("(?i)");
                }
                if raw.multiline {
                    src.push_str("(?m)");
                }
                src.push_str(&translate(&raw.source));
                fancy_regex::RegexBuilder::new(&src)
                    .backtrack_limit(100_000)
                    .build()
                    .ok()
            })
            .as_ref()
    }

    /// The grammar of a language name (`patch` is `diff`, `host-language` falls back
    /// to `host`).
    fn grammar(&self, language: &str) -> Option<usize> {
        let language = language.to_lowercase();
        let language = if language == "patch" {
            "diff".to_string()
        } else {
            language
        };
        if let Some(g) = self.languages.get(&language) {
            return Some(*g);
        }
        let mut split = language.find('-');
        while let Some(at) = split {
            if let Some(g) = self.languages.get(&language[..at]) {
                return Some(*g);
            }
            split = language[at + 1..].find('-').map(|p| at + 1 + p);
        }
        None
    }

    /// Colored runs of `text` (byte ranges, color 1..=8), empty for an unknown
    /// language.
    pub fn highlight(&self, text: &str, language: &str) -> Vec<(Range<usize>, u8)> {
        let Some(grammar) = self.grammar(language) else {
            return Vec::new();
        };
        let mut state = State {
            depth: 0,
            searches_left: 64 * text.len() + 1024,
            searches: Vec::new(),
            searches_from: 0,
        };
        let list = self.tokenize(&mut state, text, grammar);
        let mut out: Vec<(Range<usize>, u8)> = Vec::new();
        flatten(&list, 0, 0, &mut out);
        out
    }

    fn tokenize(&self, state: &mut State, text: &str, grammar: usize) -> TokenList {
        if state.depth >= 32 {
            return TokenList::new(text.len());
        }
        state.depth += 1;
        let searches_from = state.searches_from;
        state.searches_from = state.searches.len();
        let mut list = TokenList::new(text.len());
        self.match_grammar(state, text, &mut list, grammar, HEAD, 0, None);
        state.searches.truncate(state.searches_from);
        state.searches_from = searches_from;
        state.depth -= 1;
        list
    }

    /// The first match of `pattern` in `text` from `pos` on, seeing the text before
    /// it: (start, length, lookbehind length).
    fn search(&self, pattern: usize, text: &str, pos: usize) -> Option<(usize, usize, usize)> {
        let regex = self.regex(pattern)?;
        let captures = regex.captures_from_pos(text, pos).ok()??;
        let m = captures.get(0)?;
        let lookbehind = if self.patterns[pattern].lookbehind {
            captures.get(1).map_or(0, |g| g.end() - g.start())
        } else {
            0
        };
        Some((m.start(), m.end() - m.start(), lookbehind))
    }

    /// `SyntaxHighlighter::matchGreedy`: greedy patterns search the whole text, and a
    /// search is reused while it is still the first match from the position.
    fn match_greedy(
        &self,
        state: &mut State,
        pattern: usize,
        pos: usize,
        text: &str,
    ) -> Option<(usize, usize)> {
        let k = state.searches[state.searches_from..]
            .iter()
            .position(|s| s.pattern == pattern)
            .map(|k| k + state.searches_from);
        let stale = match k {
            None => true,
            Some(k) => {
                let s = &state.searches[k];
                pos < s.from || (s.found && pos > s.start)
            }
        };
        let k = match k {
            Some(k) => k,
            None => {
                state.searches.push(GreedySearch {
                    pattern,
                    from: 0,
                    found: false,
                    start: 0,
                    length: 0,
                    lookbehind: 0,
                });
                state.searches.len() - 1
            }
        };
        if stale {
            state.searches_left = state.searches_left.saturating_sub(1);
            let found = self.search(pattern, text, pos);
            let s = &mut state.searches[k];
            s.from = pos;
            s.found = found.is_some();
            if let Some((start, length, lookbehind)) = found {
                s.start = start;
                s.length = length;
                s.lookbehind = lookbehind;
            }
        }
        let s = &state.searches[k];
        s.found
            .then(|| (s.start + s.lookbehind, s.length - s.lookbehind))
    }

    /// Prism's `matchGrammar` (libprisma's port), over a token list of `text`.
    #[allow(clippy::too_many_arguments)]
    fn match_grammar(
        &self,
        state: &mut State,
        text: &str,
        list: &mut TokenList,
        grammar: usize,
        start_node: usize,
        start_pos: usize,
        mut rematch: Option<&mut Rematch>,
    ) {
        let grammar_ref = &self.grammars[grammar];
        for (token_name, patterns) in &grammar_ref.tokens {
            for (x, &pattern) in patterns.iter().enumerate() {
                if let Some(r) = rematch.as_deref()
                    && r.j == x
                    && &r.token == token_name
                {
                    return;
                }
                let raw = &self.patterns[pattern];
                let greedy = raw.greedy;
                let mut pos = start_pos;
                let mut current = list.nodes[start_node].next;
                while current != HEAD {
                    if let Some(r) = rematch.as_deref()
                        && pos >= r.reach
                    {
                        break;
                    }
                    if state.searches_left == 0 || list.count > text.len() {
                        return;
                    }
                    if list.is_syntax(current) {
                        pos += list.nodes[current].len;
                        current = list.nodes[current].next;
                        continue;
                    }
                    let mut str_start = pos;
                    let mut str_len = list.nodes[current].len;
                    let mut remove_count = 1usize;
                    let (match_index, match_len);
                    if greedy {
                        let Some((m_start, m_len)) = self.match_greedy(state, pattern, pos, text)
                        else {
                            break;
                        };
                        if m_start >= text.len() {
                            break;
                        }
                        let from = m_start;
                        let to = m_start + m_len;
                        let mut p = pos;
                        p += list.nodes[current].len;
                        while from >= p {
                            current = list.nodes[current].next;
                            if current == HEAD {
                                return;
                            }
                            p += list.nodes[current].len;
                        }
                        p -= list.nodes[current].len;
                        pos = p;
                        if list.is_syntax(current) {
                            pos += list.nodes[current].len;
                            current = list.nodes[current].next;
                            continue;
                        }
                        let mut k = current;
                        while k != HEAD && (p < to || !list.is_syntax(k)) {
                            remove_count += 1;
                            p += list.nodes[k].len;
                            k = list.nodes[k].next;
                        }
                        remove_count -= 1;
                        str_start = pos;
                        str_len = p - pos;
                        match_index = m_start - pos;
                        match_len = m_len;
                    } else {
                        state.searches_left = state.searches_left.saturating_sub(1);
                        let s = &text[str_start..str_start + str_len];
                        let Some((m_start, m_len, lookbehind)) = self.search(pattern, s, 0) else {
                            pos += list.nodes[current].len;
                            current = list.nodes[current].next;
                            continue;
                        };
                        match_index = m_start + lookbehind;
                        match_len = m_len - lookbehind;
                    }
                    let before_len = match_index;
                    let after_len = str_len - match_index - match_len;
                    let reach = pos + str_len;
                    if let Some(r) = rematch.as_deref_mut()
                        && reach > r.reach
                    {
                        r.reach = reach;
                    }
                    let mut remove_from = list.nodes[current].prev;
                    if before_len > 0 {
                        remove_from = list.add_after(remove_from, before_len, None);
                        pos += before_len;
                    }
                    list.remove_range(remove_from, remove_count);
                    let match_text_start = str_start + match_index;
                    let children = match raw.inside {
                        Some(inside) => self.tokenize(
                            state,
                            &text[match_text_start..match_text_start + match_len],
                            inside,
                        ),
                        None => TokenList::new(match_len),
                    };
                    current = list.add_after(
                        remove_from,
                        match_len,
                        Some(Box::new(Syntax {
                            kind: token_name.clone(),
                            alias: raw.alias.clone(),
                            children,
                        })),
                    );
                    if after_len > 0 {
                        list.add_after(current, after_len, None);
                    }
                    if remove_count > 1 {
                        let mut nested = Rematch {
                            token: token_name.clone(),
                            reach,
                            j: x,
                        };
                        let prev = list.nodes[current].prev;
                        self.match_grammar(
                            state,
                            text,
                            list,
                            grammar,
                            prev,
                            pos,
                            Some(&mut nested),
                        );
                        if let Some(r) = rematch.as_deref_mut()
                            && nested.reach > r.reach
                        {
                            r.reach = nested.reach;
                        }
                    }
                    pos += list.nodes[current].len;
                    current = list.nodes[current].next;
                }
            }
        }
    }
}

struct State {
    depth: usize,
    searches_left: usize,
    searches: Vec<GreedySearch>,
    searches_from: usize,
}

struct GreedySearch {
    pattern: usize,
    from: usize,
    found: bool,
    start: usize,
    length: usize,
    lookbehind: usize,
}

struct Rematch {
    token: String,
    reach: usize,
    j: usize,
}

struct Syntax {
    kind: String,
    alias: String,
    children: TokenList,
}

struct Node {
    prev: usize,
    next: usize,
    len: usize,
    syntax: Option<Box<Syntax>>,
}

/// libprisma's `TokenList`: a circular list with a head node; removed nodes stay in
/// the arena.
struct TokenList {
    nodes: Vec<Node>,
    /// Nodes in the list (libprisma's `length`).
    count: usize,
}

const HEAD: usize = 0;

impl TokenList {
    fn new(text_len: usize) -> Self {
        let mut list = Self {
            nodes: vec![Node {
                prev: HEAD,
                next: HEAD,
                len: 0,
                syntax: None,
            }],
            count: 0,
        };
        list.add_after(HEAD, text_len, None);
        list
    }

    fn is_syntax(&self, node: usize) -> bool {
        node == HEAD || self.nodes[node].syntax.is_some()
    }

    fn add_after(&mut self, node: usize, len: usize, syntax: Option<Box<Syntax>>) -> usize {
        let next = self.nodes[node].next;
        let id = self.nodes.len();
        self.nodes.push(Node {
            prev: node,
            next,
            len,
            syntax,
        });
        self.nodes[node].next = id;
        self.nodes[next].prev = id;
        self.count += 1;
        id
    }

    fn remove_range(&mut self, node: usize, count: usize) {
        for _ in 0..count {
            let item = self.nodes[node].next;
            if item == HEAD {
                break;
            }
            let next = self.nodes[item].next;
            self.nodes[node].next = next;
            self.nodes[next].prev = node;
            self.count -= 1;
        }
    }
}

/// Colored runs of a token tree (`enumerate` in `QueuedHighlighter::process`).
fn flatten(list: &TokenList, offset: usize, color: u8, out: &mut Vec<(Range<usize>, u8)>) {
    let mut pos = offset;
    let mut node = list.nodes[HEAD].next;
    while node != HEAD {
        let n = &list.nodes[node];
        match &n.syntax {
            Some(s) => {
                let c = color_of(&s.kind)
                    .or_else(|| color_of(&s.alias))
                    .unwrap_or(color);
                flatten(&s.children, pos, c, out);
            }
            None if color != 0 && n.len > 0 => match out.last_mut() {
                Some((r, c)) if *c == color && r.end == pos => r.end = pos + n.len,
                _ => out.push((pos..pos + n.len, color)),
            },
            None => {}
        }
        pos += n.len;
        node = n.next;
    }
}

/// A JavaScript regular expression in `fancy-regex`'s syntax: `\uXXXX`, `\0`,
/// `\cX`, ASCII `\d` and `\w`, `[^]` and `[]`, literal braces and brackets.
pub fn translate(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len() + 8);
    let mut i = 0;
    let mut in_class = false;
    let hex = |s: &[char]| -> Option<u32> {
        let t: String = s.iter().collect();
        u32::from_str_radix(&t, 16).ok()
    };
    let push_code = |out: &mut String, code: u32| {
        out.push_str(&format!("\\x{{{code:X}}}"));
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            match n {
                'u' if chars.len() >= i + 6 => {
                    if let Some(code) = hex(&chars[i + 2..i + 6]) {
                        push_code(&mut out, code);
                        i += 6;
                        continue;
                    }
                    out.push('u');
                    i += 2;
                }
                'x' if chars.len() >= i + 4 => {
                    if let Some(code) = hex(&chars[i + 2..i + 4]) {
                        push_code(&mut out, code);
                        i += 4;
                        continue;
                    }
                    out.push('x');
                    i += 2;
                }
                'c' if i + 2 < chars.len() && chars[i + 2].is_ascii_alphabetic() => {
                    push_code(&mut out, (chars[i + 2] as u32) % 32);
                    i += 3;
                }
                '0' if !chars.get(i + 2).is_some_and(|d| d.is_ascii_digit()) => {
                    out.push_str("\\x{0}");
                    i += 2;
                }
                'd' => {
                    out.push_str(if in_class { "0-9" } else { "[0-9]" });
                    i += 2;
                }
                'w' => {
                    out.push_str(if in_class {
                        "A-Za-z0-9_"
                    } else {
                        "[A-Za-z0-9_]"
                    });
                    i += 2;
                }
                'D' if !in_class => {
                    out.push_str("[^0-9]");
                    i += 2;
                }
                'W' if !in_class => {
                    out.push_str("[^A-Za-z0-9_]");
                    i += 2;
                }
                'b' if in_class => {
                    // Backspace inside a class.
                    out.push_str("\\x{8}");
                    i += 2;
                }
                'D' | 'W' | 's' | 'S' | 'b' | 'B' | 'n' | 'r' | 't' | 'f' | 'v' | 'k' | 'p'
                | 'P' => {
                    out.push('\\');
                    out.push(n);
                    i += 2;
                }
                '1'..='9' if !in_class => {
                    out.push('\\');
                    out.push(n);
                    i += 2;
                }
                _ if n.is_ascii_alphanumeric() => {
                    // An identity escape of a letter is the letter.
                    out.push(n);
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    out.push(n);
                    i += 2;
                }
            }
            continue;
        }
        if in_class {
            match c {
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '[' | '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                '-' if out.ends_with('-') && !out.ends_with("\\-") => out.push_str("\\-"),
                _ => out.push(c),
            }
            i += 1;
            continue;
        }
        match c {
            '[' => {
                // `[]` matches nothing, `[^]` anything.
                if chars.get(i + 1) == Some(&']') {
                    out.push_str("(?!)");
                    i += 2;
                    continue;
                }
                if chars.get(i + 1) == Some(&'^') && chars.get(i + 2) == Some(&']') {
                    out.push_str("[\\s\\S]");
                    i += 3;
                    continue;
                }
                in_class = true;
                out.push('[');
                if chars.get(i + 1) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                // A `]` first in a class is literal in neither, escape it for both.
                i += 1;
                continue;
            }
            '{' => {
                // A quantifier, or a literal brace.
                let rest: String = chars[i + 1..].iter().take(24).collect();
                let quantifier = rest.find('}').map(|end| &rest[..end]).is_some_and(|q| {
                    let mut parts = q.splitn(2, ',');
                    let a = parts.next().unwrap_or("");
                    let b = parts.next();
                    !a.is_empty()
                        && a.chars().all(|d| d.is_ascii_digit())
                        && b.is_none_or(|b| b.chars().all(|d| d.is_ascii_digit()))
                });
                if quantifier {
                    out.push('{');
                } else {
                    out.push_str("\\{");
                }
            }
            '}' => {
                // Closes a quantifier only when one is open.
                let open = out
                    .rfind('{')
                    .is_some_and(|o| !out[..o].ends_with('\\') && !out[o..].contains('}'));
                if open {
                    out.push('}');
                } else {
                    out.push_str("\\}");
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighter() -> &'static Highlighter {
        static H: OnceLock<Highlighter> = OnceLock::new();
        H.get_or_init(|| {
            Highlighter::load(include_bytes!("../../../assets/libprisma/grammars.dat"))
        })
    }

    fn colored<'a>(text: &'a str, runs: &[(Range<usize>, u8)], color: u8) -> Vec<&'a str> {
        runs.iter()
            .filter(|(_, c)| *c == color)
            .map(|(r, _)| &text[r.clone()])
            .collect()
    }

    #[test]
    fn python_like_tdesktop() {
        let code = "# Simple print statement\ngreeting = \"Hello, Python!\"\nprint(greeting)\nresult = 17 / 3\n";
        let runs = highlighter().highlight(code, "python");
        assert_eq!(colored(code, &runs, 1), ["# Simple print statement"]);
        assert_eq!(colored(code, &runs, 4), ["\"Hello, Python!\""]);
        assert!(colored(code, &runs, 6).contains(&"print"));
        assert!(colored(code, &runs, 3).contains(&"17"));
        assert!(colored(code, &runs, 5).contains(&"="));
    }

    #[test]
    fn rust_and_js() {
        let code = "fn main() {\n    let x = \"hi\"; // note\n}";
        let runs = highlighter().highlight(code, "rust");
        assert!(colored(code, &runs, 6).contains(&"fn"));
        assert!(colored(code, &runs, 1).contains(&"// note"));
        let js = "const a = `t${b}`; /* c */ if (a === 1) {}";
        let runs = highlighter().highlight(js, "js");
        assert!(colored(js, &runs, 6).contains(&"const"));
        assert!(colored(js, &runs, 1).contains(&"/* c */"));
    }

    #[test]
    fn unknown_language_is_plain() {
        assert!(highlighter().highlight("x = 1", "no-such").is_empty());
        assert!(highlighter().highlight("x = 1", "").is_empty());
    }

    #[test]
    fn most_patterns_compile() {
        let h = highlighter();
        let failed = (0..h.patterns.len())
            .filter(|&p| h.regex(p).is_none())
            .count();
        // A few JavaScript-only forms may stay unsupported; they never match.
        assert!(
            failed * 100 < h.patterns.len(),
            "{failed} of {} patterns do not compile",
            h.patterns.len()
        );
    }

    #[test]
    fn translates_javascript_forms() {
        assert_eq!(translate(r"[^]"), r"[\s\S]");
        assert_eq!(translate(r"a{2}b{"), r"a{2}b\{");
        assert_eq!(translate(r"[[\]]"), r"[\[\]]");
        let escaped = format!("{}u00e9", '\\');
        assert_eq!(translate(&escaped), r"\x{E9}");
        assert_eq!(translate(r"\w+"), "[A-Za-z0-9_]+");
    }
}

#[cfg(test)]
mod timing {
    #[test]
    #[ignore]
    fn first_and_second_highlight() {
        let t = std::time::Instant::now();
        let h = super::Highlighter::load(include_bytes!("../../../assets/libprisma/grammars.dat"));
        eprintln!("load {:?}", t.elapsed());
        let code =
            "# c\nimport os\ndef f(x):\n    return x * 2  # twice\nprint(f(\"a\"))\n".repeat(20);
        for lang in ["python", "rust", "js", "cpp", "python"] {
            let t = std::time::Instant::now();
            let runs = h.highlight(&code, lang);
            eprintln!("{lang}: {:?} ({} runs)", t.elapsed(), runs.len());
        }
    }
}
