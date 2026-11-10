//! Parser for Telegram's TL schema files (`api.tl`, `mtproto.tl`).
//!
//! This crate is the foundation of two things:
//! * the code generator that turns the schema into Rust types (planned `tdr-tl-gen`);
//! * the upstream tracker (`cargo xtask upstream`), which diffs schemas between
//!   tdesktop releases.
//!
//! The parser is intentionally strict about what it accepts as a definition and
//! skips only the well-known built-in declarations (`int ? = Int;`, `vector ...`).

use std::collections::BTreeMap;
use std::fmt;

/// Whether a definition is a constructor (type) or an RPC method (function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Type,
    Function,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Category::Type => "constructor",
            Category::Function => "method",
        })
    }
}

/// A single parameter, e.g. `peer:InputPeer` or `silent:flags.5?true`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Param {
    pub name: String,
    /// The raw type as written in the schema (`flags.5?true`, `Vector<long>`, `#`, `!X`).
    pub ty: String,
}

impl Param {
    /// `Some((flags_field, bit))` for conditional parameters like `flags.5?true`.
    pub fn flag(&self) -> Option<(&str, u32)> {
        let (cond, _) = self.ty.split_once('?')?;
        let (field, bit) = cond.split_once('.')?;
        Some((field, bit.parse().ok()?))
    }
}

impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name, self.ty)
    }
}

/// One TL definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Definition {
    /// Full name including namespace, e.g. `messages.sendMessage`.
    pub name: String,
    /// Constructor id (`#xxxxxxxx`). Always present in Telegram's schemas.
    pub id: u32,
    /// Generic type parameters, e.g. `X` in `{X:Type}`.
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    /// Result type, e.g. `Updates` or `Vector<Bool>`.
    pub result: String,
    pub category: Category,
    /// 1-based line number in the source file.
    pub line: usize,
}

impl Definition {
    pub fn namespace(&self) -> Option<&str> {
        self.name.split_once('.').map(|(ns, _)| ns)
    }

    /// Key used to match definitions between schema versions.
    pub fn key(&self) -> (Category, &str) {
        (self.category, &self.name)
    }
}

impl fmt::Display for Definition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{:08x}", self.name, self.id)?;
        for g in &self.generics {
            write!(f, " {{{g}:Type}}")?;
        }
        for p in &self.params {
            write!(f, " {p}")?;
        }
        write!(f, " = {};", self.result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// A parsed schema.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    /// Value of the `// LAYER N` comment, if present.
    pub layer: Option<u32>,
    pub definitions: Vec<Definition>,
}

impl Schema {
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let mut schema = Schema::default();
        let mut category = Category::Type;

        for (idx, raw) in source.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw.trim();

            if let Some(comment) = line.strip_prefix("//") {
                if let Some(layer) = comment.trim().strip_prefix("LAYER") {
                    schema.layer = layer.trim().parse().ok();
                }
                continue;
            }
            // Strip trailing comments.
            let line = match line.find("//") {
                Some(pos) => line[..pos].trim(),
                None => line,
            };
            if line.is_empty() {
                continue;
            }
            match line {
                "---functions---" => {
                    category = Category::Function;
                    continue;
                }
                "---types---" => {
                    category = Category::Type;
                    continue;
                }
                _ => {}
            }
            if is_builtin(line) {
                continue;
            }
            let def = parse_definition(line, category).map_err(|message| ParseError {
                line: line_no,
                message,
            })?;
            schema.definitions.push(Definition {
                line: line_no,
                ..def
            });
        }
        Ok(schema)
    }

    pub fn by_key(&self) -> BTreeMap<(Category, &str), &Definition> {
        self.definitions.iter().map(|d| (d.key(), d)).collect()
    }
}

/// Built-in declarations that describe the TL language itself, not real constructors.
fn is_builtin(line: &str) -> bool {
    line.contains('[') || line.contains(" ? =")
}

fn parse_definition(line: &str, category: Category) -> Result<Definition, String> {
    let body = line
        .strip_suffix(';')
        .ok_or_else(|| "definition does not end with ';'".to_string())?;
    let (lhs, result) = body
        .rsplit_once('=')
        .ok_or_else(|| "definition has no '='".to_string())?;
    let result = result.trim();
    if result.is_empty() {
        return Err("empty result type".into());
    }

    let mut tokens = lhs.split_whitespace();
    let head = tokens.next().ok_or("missing name")?;
    let (name, id) = match head.split_once('#') {
        Some((name, id)) => (
            name,
            u32::from_str_radix(id, 16).map_err(|e| format!("bad constructor id '{id}': {e}"))?,
        ),
        None => (head, infer_id(body)),
    };
    if name.is_empty() {
        return Err("empty name".into());
    }

    let mut generics = Vec::new();
    let mut params = Vec::new();
    for tok in tokens {
        if let Some(inner) = tok.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
            let (g, kind) = inner
                .split_once(':')
                .ok_or_else(|| format!("bad generic '{tok}'"))?;
            if kind != "Type" {
                return Err(format!("unsupported generic kind '{kind}'"));
            }
            generics.push(g.to_string());
            continue;
        }
        let (pname, ty) = tok
            .split_once(':')
            .ok_or_else(|| format!("bad parameter '{tok}'"))?;
        if pname.is_empty() || ty.is_empty() {
            return Err(format!("bad parameter '{tok}'"));
        }
        params.push(Param {
            name: pname.to_string(),
            ty: ty.to_string(),
        });
    }

    Ok(Definition {
        name: name.to_string(),
        id,
        generics,
        params,
        result: result.to_string(),
        category,
        line: 0,
    })
}

/// Computes the constructor id of a definition written without `#id`: CRC32 of its
/// canonical form (no id, `bytes` spelled `string`, no angle/curly brackets, no
/// `flags.N?true` parameters).
pub fn infer_id(definition: &str) -> u32 {
    let def = definition.trim().trim_end_matches(';').trim();
    let mut tokens: Vec<String> = Vec::new();
    for (i, tok) in def.split_whitespace().enumerate() {
        if i == 0 {
            tokens.push(tok.split('#').next().unwrap_or(tok).to_string());
            continue;
        }
        if let Some((_, ty)) = tok.split_once(':')
            && let Some((cond, ty)) = ty.split_once('?')
            && cond.contains('.')
            && ty == "true"
        {
            continue;
        }
        tokens.push(tok.to_string());
    }
    let canonical = tokens
        .join(" ")
        .replace(":bytes", ":string")
        .replace("?bytes", "?string")
        .replace('<', " ")
        .replace(['>', '{', '}'], "");
    crc32(canonical.as_bytes())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// A change to a single definition between two schema versions.
#[derive(Debug, Clone)]
pub enum Change<'a> {
    Added(&'a Definition),
    Removed(&'a Definition),
    Changed {
        old: &'a Definition,
        new: &'a Definition,
        added_params: Vec<&'a Param>,
        removed_params: Vec<&'a Param>,
    },
}

impl Change<'_> {
    pub fn definition(&self) -> &Definition {
        match self {
            Change::Added(d) | Change::Removed(d) => d,
            Change::Changed { new, .. } => new,
        }
    }
}

/// Structural diff of two schemas, ordered by (category, name).
#[derive(Debug, Clone)]
pub struct SchemaDiff<'a> {
    pub old_layer: Option<u32>,
    pub new_layer: Option<u32>,
    pub changes: Vec<Change<'a>>,
}

impl SchemaDiff<'_> {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.old_layer == self.new_layer
    }

    pub fn count(&self, pred: impl Fn(&Change<'_>) -> bool) -> usize {
        self.changes.iter().filter(|c| pred(c)).count()
    }
}

pub fn diff<'a>(old: &'a Schema, new: &'a Schema) -> SchemaDiff<'a> {
    let old_map = old.by_key();
    let new_map = new.by_key();
    let mut changes = Vec::new();

    for (key, &n) in &new_map {
        match old_map.get(key) {
            None => changes.push(Change::Added(n)),
            Some(&o) => {
                if o.id != n.id || o.params != n.params || o.result != n.result {
                    let added_params = n.params.iter().filter(|p| !o.params.contains(p)).collect();
                    let removed_params =
                        o.params.iter().filter(|p| !n.params.contains(p)).collect();
                    changes.push(Change::Changed {
                        old: o,
                        new: n,
                        added_params,
                        removed_params,
                    });
                }
            }
        }
    }
    for (key, &o) in &old_map {
        if !new_map.contains_key(key) {
            changes.push(Change::Removed(o));
        }
    }
    changes.sort_by(|a, b| a.definition().key().cmp(&b.definition().key()));

    SchemaDiff {
        old_layer: old.layer,
        new_layer: new.layer,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
int ? = Int;
vector {t:Type} # [ t ] = Vector t;
boolFalse#bc799737 = Bool;
inputPeerUser#dde8a54c user_id:long access_hash:long = InputPeer;
---functions---
invokeWithLayer#da9b0d0d {X:Type} layer:int query:!X = X;
messages.sendMessage#fe05dc9a flags:# no_webpage:flags.1?true peer:InputPeer message:string = Updates; // comment
// LAYER 233
"#;

    #[test]
    fn parses_sample() {
        let s = Schema::parse(SAMPLE).unwrap();
        assert_eq!(s.layer, Some(233));
        assert_eq!(s.definitions.len(), 4);

        let invoke = &s.definitions[2];
        assert_eq!(invoke.category, Category::Function);
        assert_eq!(invoke.generics, vec!["X"]);
        assert_eq!(invoke.params[1].ty, "!X");

        let send = &s.definitions[3];
        assert_eq!(send.name, "messages.sendMessage");
        assert_eq!(send.namespace(), Some("messages"));
        assert_eq!(send.id, 0xfe05dc9a);
        assert_eq!(send.params[1].flag(), Some(("flags", 1)));
        assert_eq!(send.result, "Updates");
        assert_eq!(
            send.to_string(),
            "messages.sendMessage#fe05dc9a flags:# no_webpage:flags.1?true peer:InputPeer message:string = Updates;"
        );
    }

    /// The inferred id must match the explicit one for (almost) every real definition.
    #[test]
    fn infers_ids_like_telegram() {
        assert_eq!(infer_id("ipPort ipv4:int port:int = IpPort;"), 0xd433ad73);
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/api.tl");
        let src = std::fs::read_to_string(root).unwrap();
        let (mut total, mut matched) = (0, 0);
        for line in src.lines() {
            let line = line.trim();
            if line.starts_with("//") || !line.contains('#') || !line.ends_with(';') {
                continue;
            }
            let Some(def) = parse_definition(line, Category::Type).ok() else {
                continue;
            };
            total += 1;
            if infer_id(line) == def.id {
                matched += 1;
            }
        }
        assert!(total > 2000);
        // Every explicit id in the real schema follows the rule.
        assert_eq!(matched, total, "{matched}/{total} ids matched");
    }

    #[test]
    fn rejects_garbage() {
        assert!(Schema::parse("foo bar:int baz = Baz;").is_err());
        assert!(Schema::parse("foo#zz = Baz;").is_err());
        assert!(Schema::parse("foo#1 = ;").is_err());
        assert!(Schema::parse("foo#12345678 = Baz").is_err());
    }

    #[test]
    fn diffs() {
        let old = Schema::parse(
            "a#00000001 x:int = A;\nb#00000002 = B;\n---functions---\nf#00000003 = A;\n// LAYER 1",
        )
        .unwrap();
        let new = Schema::parse(
            "a#00000011 x:int y:flags.0?int = A;\nc#00000004 = C;\n---functions---\nf#00000003 = A;\n// LAYER 2",
        )
        .unwrap();
        let d = diff(&old, &new);
        assert_eq!((d.old_layer, d.new_layer), (Some(1), Some(2)));
        assert_eq!(d.changes.len(), 3);
        assert!(
            matches!(&d.changes[0], Change::Changed { added_params, removed_params, .. }
            if added_params.len() == 1 && removed_params.is_empty())
        );
        assert!(matches!(d.changes[1], Change::Removed(def) if def.name == "b"));
        assert!(matches!(d.changes[2], Change::Added(def) if def.name == "c"));
    }

    /// Parses the vendored real schemas; guards against upstream format changes.
    #[test]
    fn parses_vendored_schemas() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/");
        for file in ["api.tl", "mtproto.tl"] {
            let src = std::fs::read_to_string(format!("{root}{file}")).unwrap();
            let s = Schema::parse(&src).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(!s.definitions.is_empty(), "{file}: no definitions");
        }
        let api = std::fs::read_to_string(format!("{root}api.tl")).unwrap();
        let api = Schema::parse(&api).unwrap();
        assert!(api.layer.is_some());
        assert!(api.definitions.len() > 2000);
    }
}
