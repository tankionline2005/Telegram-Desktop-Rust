//! Generates Rust code for a TL schema. Used from `tdr-tl`'s build script.
//!
//! Output layout (inside a module chosen by the caller, `crate_path` points at it):
//! * `types::[ns::]Name`     — one struct per constructor (bare + boxed (de)serialization);
//! * `enums::[ns::]Name`     — one enum per boxed type;
//! * `functions::[ns::]Name` — one struct per RPC method, implementing `RemoteCall`.
//!
//! Performance-relevant choices:
//! * small constructors made only of primitives (≤ 16 bytes) are stored inline in their
//!   enum; everything else is boxed, so every enum stays small and cheap to move;
//! * `bytes` fields are `Bytes` slices of the received buffer (no copy).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use tdr_tl_parser::{Category, Definition, Schema};

/// Types mapped to Rust primitives instead of generated enums.
const BUILTIN_TYPES: &[&str] = &["Bool", "True"];

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "static", "struct", "trait", "true", "type", "unsafe", "use", "where",
    "while", "abstract", "become", "box", "do", "final", "macro", "override", "priv", "try",
    "typeof", "unsized", "virtual", "yield",
];

pub struct Config<'a> {
    /// Path of the module the output is included into, e.g. `crate` or `crate::mtproto`.
    pub module_path: &'a str,
    /// Path of the runtime (traits, `Reader`...), e.g. `crate`.
    pub runtime_path: &'a str,
    /// Map TL `string` to `Bytes`. Needed for `mtproto.tl`, where binary data such as
    /// `pq` or `dh_prime` is declared as `string`.
    pub string_as_bytes: bool,
}

pub fn generate(schema: &Schema, cfg: &Config<'_>) -> String {
    Generator::new(schema, cfg).run()
}

fn pascal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Rust field name: lowercase (`srp_B` → `srp_b`, `M1` → `m1`), keywords escaped.
fn field_ident(name: &str) -> String {
    let name = name.to_lowercase();
    match name.as_str() {
        "self" | "super" | "crate" => format!("{name}_"),
        n if KEYWORDS.contains(&n) => format!("r#{n}"),
        _ => name,
    }
}

/// `messages.sendMessage` → (Some("messages"), "sendMessage").
fn split_ns(name: &str) -> (Option<&str>, &str) {
    match name.rsplit_once('.') {
        Some((ns, n)) => (Some(ns), n),
        None => (None, name),
    }
}

fn mod_path(ns: Option<&str>) -> String {
    ns.map(|ns| format!("::{ns}")).unwrap_or_default()
}

struct Generator<'a> {
    defs: Vec<&'a Definition>,
    cfg: &'a Config<'a>,
    rt: String,
    /// Boxed type name → constructors.
    enums: BTreeMap<&'a str, Vec<&'a Definition>>,
    /// Constructor name → stored inline in its enum (not boxed).
    inline: HashMap<&'a str, bool>,
}

impl<'a> Generator<'a> {
    fn new(schema: &'a Schema, cfg: &'a Config<'a>) -> Self {
        let mut enums: BTreeMap<&str, Vec<&Definition>> = BTreeMap::new();
        for d in &schema.definitions {
            if d.category == Category::Type && !BUILTIN_TYPES.contains(&d.result.as_str()) {
                enums.entry(d.result.as_str()).or_default().push(d);
            }
        }
        let mut g = Generator {
            defs: Vec::new(),
            cfg,
            rt: cfg.runtime_path.to_string(),
            enums,
            inline: HashMap::new(),
        };
        // Drop definitions referencing types we can't represent (e.g. `Object`), repeatedly,
        // since dropping a constructor can make its type disappear.
        let mut defs: Vec<&Definition> = schema
            .definitions
            .iter()
            // Constructors of built-in types (`boolTrue`, `true`) are not generated;
            // functions returning them are.
            .filter(|d| {
                d.category == Category::Function || !BUILTIN_TYPES.contains(&d.result.as_str())
            })
            .collect();
        loop {
            let before = defs.len();
            defs.retain(|d| {
                d.params.iter().all(|p| g.representable(d, &p.ty))
                    && (d.category == Category::Type || g.representable(d, &d.result))
            });
            g.enums = BTreeMap::new();
            for d in &defs {
                if d.category == Category::Type {
                    g.enums.entry(d.result.as_str()).or_default().push(d);
                }
            }
            if defs.len() == before {
                break;
            }
        }
        g.defs = defs;
        for d in &g.defs {
            if d.category == Category::Type {
                let inline = g.inline_size(d).is_some_and(|s| s <= 16);
                g.inline.insert(&d.name, inline);
            }
        }
        g
    }

    fn representable(&self, def: &Definition, ty: &str) -> bool {
        let ty = ty.split_once('?').map_or(ty, |(_, t)| t);
        let ty = ty.strip_prefix('!').unwrap_or(ty);
        if let Some(inner) = ty
            .strip_prefix("Vector<")
            .or_else(|| ty.strip_prefix("vector<"))
            .and_then(|t| t.strip_suffix('>'))
        {
            return self.representable(def, inner);
        }
        match ty {
            "#" | "int" | "long" | "double" | "int128" | "int256" | "string" | "bytes" | "Bool"
            | "true" => true,
            t if def.generics.iter().any(|g| g == t) => true,
            t => self.enums.contains_key(t),
        }
    }

    /// Size in bytes if the constructor contains only fixed-size primitives.
    fn inline_size(&self, d: &Definition) -> Option<usize> {
        let mut size = 0;
        for p in &d.params {
            if p.ty == "#" {
                continue;
            }
            size += match p.ty.as_str() {
                "int" => 4,
                "long" | "double" => 8,
                "int128" => 16,
                "int256" => 32,
                "Bool" => 1,
                t if t.ends_with("?true") => 1,
                _ => return None,
            };
        }
        Some(size)
    }

    fn run(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "// @generated by tdr-tl-gen. Do not edit.\n#[allow(clippy::all, unused_imports, unused_variables, unused_mut, non_camel_case_types, non_snake_case)]\npub mod types {{"
        );
        self.emit_grouped(&mut out, Category::Type, |g, o, d| g.emit_struct(o, d));
        out.push_str("}\n\n#[allow(clippy::all, unused_imports, unused_variables, unused_mut, non_camel_case_types, non_snake_case)]\npub mod enums {\n");
        let by_ns = self.group_enums();
        for (ns, list) in &by_ns {
            if let Some(ns) = ns {
                let _ = writeln!(out, "pub mod {ns} {{");
            }
            for (name, ctors) in list {
                self.emit_enum(&mut out, name, ctors);
            }
            if ns.is_some() {
                out.push_str("}\n");
            }
        }
        out.push_str("}\n\n#[allow(clippy::all, unused_imports, unused_variables, unused_mut, non_camel_case_types, non_snake_case)]\npub mod functions {\n");
        self.emit_grouped(&mut out, Category::Function, |g, o, d| {
            g.emit_function(o, d)
        });
        out.push_str("}\n\n");
        self.emit_name_for_id(&mut out);
        out
    }

    fn emit_grouped(
        &self,
        out: &mut String,
        cat: Category,
        emit: impl Fn(&Self, &mut String, &Definition),
    ) {
        let mut by_ns: BTreeMap<Option<&str>, Vec<&Definition>> = BTreeMap::new();
        for d in self.defs.iter().filter(|d| d.category == cat) {
            by_ns.entry(split_ns(&d.name).0).or_default().push(d);
        }
        for (ns, defs) in by_ns {
            if let Some(ns) = ns {
                let _ = writeln!(out, "pub mod {ns} {{");
            }
            for d in defs {
                emit(self, out, d);
            }
            if ns.is_some() {
                out.push_str("}\n");
            }
        }
    }

    fn group_enums(&self) -> BTreeMap<Option<&'a str>, Vec<(&'a str, &Vec<&'a Definition>)>> {
        let mut by_ns: BTreeMap<Option<&str>, Vec<(&str, &Vec<&Definition>)>> = BTreeMap::new();
        for (name, ctors) in &self.enums {
            by_ns
                .entry(split_ns(name).0)
                .or_default()
                .push((name, ctors));
        }
        by_ns
    }

    fn type_path(&self, name: &str) -> String {
        let (ns, n) = split_ns(name);
        format!(
            "{}::types{}::{}",
            self.cfg.module_path,
            mod_path(ns),
            pascal(n)
        )
    }

    fn enum_path(&self, name: &str) -> String {
        let (ns, n) = split_ns(name);
        format!(
            "{}::enums{}::{}",
            self.cfg.module_path,
            mod_path(ns),
            pascal(n)
        )
    }

    /// Rust type for a TL type (without the `flags.N?` prefix).
    fn rust_type(&self, ty: &str, generics: &[String]) -> String {
        let ty = ty.strip_prefix('!').unwrap_or(ty);
        if let Some(inner) = ty.strip_prefix("Vector<").and_then(|t| t.strip_suffix('>')) {
            return format!("::std::vec::Vec<{}>", self.rust_type(inner, generics));
        }
        if let Some(inner) = ty.strip_prefix("vector<").and_then(|t| t.strip_suffix('>')) {
            return format!("{}::RawVec<{}>", self.rt, self.rust_type(inner, generics));
        }
        match ty {
            "#" => "u32".into(),
            "int" => "i32".into(),
            "long" => "i64".into(),
            "double" => "f64".into(),
            "int128" => "[u8; 16]".into(),
            "int256" => "[u8; 32]".into(),
            "string" if self.cfg.string_as_bytes => format!("{}::Bytes", self.rt),
            "string" => "::std::string::String".into(),
            "bytes" => format!("{}::Bytes", self.rt),
            "Bool" | "true" => "bool".into(),
            t if generics.iter().any(|g| g == t) => t.to_string(),
            t => self.enum_path(t),
        }
    }

    fn field_type(&self, p: &tdr_tl_parser::Param, generics: &[String]) -> String {
        match p.ty.split_once('?') {
            Some((_, "true")) => "bool".into(),
            Some((_, t)) => format!("::core::option::Option<{}>", self.rust_type(t, generics)),
            None => self.rust_type(&p.ty, generics),
        }
    }

    fn emit_doc(&self, out: &mut String, d: &Definition) {
        let _ = writeln!(out, "/// `{d}`");
    }

    /// Fields, bare serialization and bare deserialization bodies for a definition.
    fn emit_fields_and_bodies(&self, d: &Definition) -> (String, String, String) {
        let rt = &self.rt;
        let mut fields = String::new();
        let mut ser = String::new();
        let mut de = String::new();
        let mut ctor = String::new();
        for p in &d.params {
            let ident = field_ident(&p.name);
            let local = format!("f_{}", p.name);
            if p.ty == "#" {
                // Flags word: computed from the fields that depend on it.
                let _ = writeln!(ser, "        let mut {local}: u32 = 0;");
                for q in &d.params {
                    if let Some((field, bit)) = q.flag()
                        && field == p.name
                    {
                        let qi = field_ident(&q.name);
                        let cond = if q.ty.ends_with("?true") {
                            format!("self.{qi}")
                        } else {
                            format!("self.{qi}.is_some()")
                        };
                        let _ = writeln!(ser, "        if {cond} {{ {local} |= 1 << {bit}; }}");
                    }
                }
                let _ = writeln!(ser, "        {rt}::Serializable::serialize(&{local}, out);");
                let _ = writeln!(
                    de,
                    "        let {local} = <u32 as {rt}::Deserializable>::deserialize(r)?;"
                );
                continue;
            }
            let ty = self.field_type(p, &d.generics);
            let _ = writeln!(fields, "    pub {ident}: {ty},");
            let _ = writeln!(ctor, "            {ident}: {local},");
            match p.flag() {
                Some((field, bit)) if p.ty.ends_with("?true") => {
                    let _ = writeln!(de, "        let {local} = f_{field} & (1 << {bit}) != 0;");
                }
                Some((field, bit)) => {
                    let _ = writeln!(
                        ser,
                        "        if let ::core::option::Option::Some(v) = &self.{ident} {{ {rt}::Serializable::serialize(v, out); }}"
                    );
                    let _ = writeln!(
                        de,
                        "        let {local} = if f_{field} & (1 << {bit}) != 0 {{ ::core::option::Option::Some({rt}::Deserializable::deserialize(r)?) }} else {{ ::core::option::Option::None }};"
                    );
                }
                None => {
                    let _ = writeln!(
                        ser,
                        "        {rt}::Serializable::serialize(&self.{ident}, out);"
                    );
                    let _ = writeln!(
                        de,
                        "        let {local} = {rt}::Deserializable::deserialize(r)?;"
                    );
                }
            }
        }
        let _ = write!(
            de,
            "        ::core::result::Result::Ok(Self {{\n{ctor}        }})"
        );
        (fields, ser, de)
    }

    fn emit_struct(&self, out: &mut String, d: &Definition) {
        let rt = &self.rt;
        let name = pascal(split_ns(&d.name).1);
        let (fields, ser, de) = self.emit_fields_and_bodies(d);
        let id = d.id;
        self.emit_doc(out, d);
        let _ = write!(
            out,
            "#[derive(Debug, Clone, PartialEq)]\npub struct {name} {{\n{fields}}}\n\
impl {rt}::Identifiable for {name} {{ const CONSTRUCTOR_ID: u32 = {id:#010x}; }}\n\
impl {rt}::Bare for {name} {{\n\
    fn serialize_bare(&self, out: &mut ::std::vec::Vec<u8>) {{\n{ser}    }}\n\
    fn deserialize_bare(r: &mut {rt}::Reader<'_>) -> ::core::result::Result<Self, {rt}::DeserializeError> {{\n{de}\n    }}\n}}\n\
impl {rt}::Serializable for {name} {{\n\
    fn serialize(&self, out: &mut ::std::vec::Vec<u8>) {{\n\
        out.extend_from_slice(&{id:#010x}u32.to_le_bytes());\n\
        {rt}::Bare::serialize_bare(self, out);\n    }}\n}}\n\
impl {rt}::Deserializable for {name} {{\n\
    fn deserialize(r: &mut {rt}::Reader<'_>) -> ::core::result::Result<Self, {rt}::DeserializeError> {{\n\
        let id = r.read_u32()?;\n\
        if id != {id:#010x} {{ return ::core::result::Result::Err({rt}::DeserializeError::UnexpectedConstructor {{ id }}); }}\n\
        {rt}::Bare::deserialize_bare(r)\n    }}\n}}\n"
        );
    }

    /// Variant names: constructor name with the type name stripped (`inputPeerUser` → `User`).
    fn variant_names(&self, type_name: &str, ctors: &[&Definition]) -> Vec<String> {
        let tn = pascal(split_ns(type_name).1).to_lowercase();
        let mut names: Vec<String> = ctors
            .iter()
            .map(|c| {
                let cn = pascal(split_ns(&c.name).1);
                let stripped = if cn.to_lowercase().starts_with(&tn) {
                    &cn[tn.len()..]
                } else {
                    ""
                };
                if stripped.is_empty()
                    || stripped == "Self"
                    || stripped.starts_with(|ch: char| ch.is_ascii_digit())
                {
                    cn
                } else {
                    stripped.to_string()
                }
            })
            .collect();
        let mut seen = BTreeSet::new();
        let dups: BTreeSet<String> = names
            .iter()
            .filter(|n| !seen.insert((*n).clone()))
            .cloned()
            .collect();
        for (n, c) in names.iter_mut().zip(ctors) {
            if dups.contains(n) {
                *n = pascal(split_ns(&c.name).1);
            }
        }
        names
    }

    fn emit_enum(&self, out: &mut String, type_name: &str, ctors: &[&Definition]) {
        let rt = &self.rt;
        let name = pascal(split_ns(type_name).1);
        let variants = self.variant_names(type_name, ctors);
        let mut decl = String::new();
        let mut ser = String::new();
        let mut de = String::new();
        let mut ids = String::new();
        let mut froms = String::new();
        for (c, v) in ctors.iter().zip(&variants) {
            let path = self.type_path(&c.name);
            let id = c.id;
            let unit = c.params.is_empty();
            if unit {
                let _ = writeln!(decl, "    {v},");
                let _ = writeln!(
                    ser,
                    "            Self::{v} => out.extend_from_slice(&{id:#010x}u32.to_le_bytes()),"
                );
                let _ = writeln!(de, "            {id:#010x} => Self::{v},");
                let _ = writeln!(ids, "            Self::{v} => {id:#010x},");
                let _ = writeln!(
                    froms,
                    "impl ::core::convert::From<{path}> for {name} {{ fn from(_: {path}) -> Self {{ Self::{v} }} }}"
                );
                continue;
            }
            let inline = self.inline.get(c.name.as_str()).copied().unwrap_or(false);
            let (ty, wrap) = if inline {
                (path.clone(), String::from("x"))
            } else {
                (
                    format!("::std::boxed::Box<{path}>"),
                    String::from("::std::boxed::Box::new(x)"),
                )
            };
            let _ = writeln!(decl, "    {v}({ty}),");
            let arg = if inline { "x" } else { "&**x" };
            let _ = writeln!(
                ser,
                "            Self::{v}(x) => {rt}::Serializable::serialize({arg}, out),"
            );
            let _ = writeln!(
                de,
                "            {id:#010x} => {{ let x = <{path} as {rt}::Bare>::deserialize_bare(r)?; Self::{v}({wrap}) }}"
            );
            let _ = writeln!(ids, "            Self::{v}(_) => {id:#010x},");
            let _ = writeln!(
                froms,
                "impl ::core::convert::From<{path}> for {name} {{ fn from(x: {path}) -> Self {{ Self::{v}({wrap}) }} }}"
            );
        }
        let _ = write!(
            out,
            "/// Boxed type `{type_name}`.\n#[derive(Debug, Clone, PartialEq)]\npub enum {name} {{\n{decl}}}\n\
impl {name} {{\n    pub fn constructor_id(&self) -> u32 {{\n        match self {{\n{ids}        }}\n    }}\n}}\n\
impl {rt}::Serializable for {name} {{\n    fn serialize(&self, out: &mut ::std::vec::Vec<u8>) {{\n        match self {{\n{ser}        }}\n    }}\n}}\n\
impl {rt}::Deserializable for {name} {{\n    fn deserialize(r: &mut {rt}::Reader<'_>) -> ::core::result::Result<Self, {rt}::DeserializeError> {{\n\
        let id = r.read_u32()?;\n        ::core::result::Result::Ok(match id {{\n{de}            _ => return ::core::result::Result::Err({rt}::DeserializeError::UnexpectedConstructor {{ id }}),\n        }})\n    }}\n}}\n{froms}"
        );
    }

    fn emit_function(&self, out: &mut String, d: &Definition) {
        let rt = &self.rt;
        let name = pascal(split_ns(&d.name).1);
        let (fields, ser, _) = self.emit_fields_and_bodies(d);
        let id = d.id;
        let (gdecl, gbound, guse) = if d.generics.is_empty() {
            (String::new(), String::new(), String::new())
        } else {
            let list = d.generics.join(", ");
            let bounds: Vec<String> = d
                .generics
                .iter()
                .map(|g| format!("{g}: {rt}::RemoteCall"))
                .collect();
            (
                format!("<{list}>"),
                format!("<{}>", bounds.join(", ")),
                format!("<{list}>"),
            )
        };
        let ret = self.rust_type(&d.result, &[]);
        let ret = if d.generics.contains(&d.result) {
            format!("{}::Return", d.result)
        } else {
            ret
        };
        self.emit_doc(out, d);
        let _ = write!(
            out,
            "#[derive(Debug, Clone, PartialEq)]\npub struct {name}{gdecl} {{\n{fields}}}\n\
impl{gbound} {rt}::Identifiable for {name}{guse} {{ const CONSTRUCTOR_ID: u32 = {id:#010x}; }}\n\
impl{gbound} {rt}::Serializable for {name}{guse} {{\n\
    fn serialize(&self, out: &mut ::std::vec::Vec<u8>) {{\n\
        out.extend_from_slice(&{id:#010x}u32.to_le_bytes());\n{ser}    }}\n}}\n\
impl{gbound} {rt}::RemoteCall for {name}{guse} {{ type Return = {ret}; }}\n"
        );
    }

    fn emit_name_for_id(&self, out: &mut String) {
        out.push_str("/// TL name of a constructor or method id, for logs and debugging.\npub fn name_for_id(id: u32) -> ::core::option::Option<&'static str> {\n    ::core::option::Option::Some(match id {\n");
        let mut seen = BTreeSet::new();
        for d in &self.defs {
            if seen.insert(d.id) {
                let _ = writeln!(out, "        {:#010x} => \"{}\",", d.id, d.name);
            }
        }
        out.push_str("        _ => return ::core::option::Option::None,\n    })\n}\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming() {
        assert_eq!(pascal("inputPeerUser"), "InputPeerUser");
        assert_eq!(pascal("p_q_inner_data"), "PQInnerData");
        assert_eq!(field_ident("type"), "r#type");
        assert_eq!(field_ident("self"), "self_");
    }

    #[test]
    fn generates_api() {
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/api.tl"))
                .unwrap();
        let schema = Schema::parse(&src).unwrap();
        let code = generate(
            &schema,
            &Config {
                module_path: "crate",
                runtime_path: "crate",
                string_as_bytes: false,
            },
        );
        assert!(code.contains("pub struct SendMessage"));
        assert!(code.contains("pub enum InputPeer"));
        // Small primitive-only constructors are inline, big ones boxed.
        assert!(code.contains("    User(crate::types::InputPeerUser),"));
        assert!(code.contains("Box<crate::types::Message>"));
        // Methods returning Bool are kept.
        assert!(code.contains("pub struct BindTempAuthKey"));
        assert!(code.contains("impl crate::RemoteCall for LogOut"));
        // Every function in the schema is generated.
        let functions = schema
            .definitions
            .iter()
            .filter(|d| d.category == Category::Function)
            .count();
        assert_eq!(
            code.matches("impl crate::RemoteCall for").count()
                + code.matches("> crate::RemoteCall for").count(),
            functions
        );
    }
}
