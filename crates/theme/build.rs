//! Generates `Palette` from tdesktop's `colors.palette` (day colors) and the built-in
//! night theme (`night.tdesktop-theme`, a zip with `colors.tdesktop-theme`).
//!
//! Palette values are `#rrggbb[aa]`, a reference to another key, or `#rrggbb | key`
//! (a default plus a fallback key: a theme that defines `key` but not this one gets
//! `key`'s value). A theme overrides some keys; everything else derives from the palette.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;
use std::{env, fs};

#[derive(Debug, Clone)]
enum Def {
    Color(u32),
    Alias(String),
    WithFallback(u32, String),
}

fn parse_color(v: &str) -> Option<u32> {
    let hex = v.strip_prefix('#')?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some((n << 8) | 0xff),
        8 => Some(n),
        _ => None,
    }
}

/// `name: value; // comment` lines, in file order.
fn parse(text: &str) -> Vec<(String, Def)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches(';').trim();
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let def = match value.split_once('|') {
            Some((color, fallback)) => Def::WithFallback(
                parse_color(color.trim()).unwrap_or_else(|| panic!("bad color in `{line}`")),
                fallback.trim().to_string(),
            ),
            None => match parse_color(value) {
                Some(c) => Def::Color(c),
                None => Def::Alias(value.to_string()),
            },
        };
        out.push((name.to_string(), def));
    }
    out
}

struct Resolver<'a> {
    palette: &'a HashMap<String, Def>,
    theme: &'a HashMap<String, Def>,
    cache: HashMap<String, u32>,
}

impl Resolver<'_> {
    fn get(&mut self, name: &str, depth: usize) -> u32 {
        assert!(depth < 64, "alias cycle at {name}");
        if let Some(c) = self.cache.get(name) {
            return *c;
        }
        let value = match self.theme.get(name).or_else(|| self.palette.get(name)) {
            Some(Def::Color(c)) => *c,
            Some(Def::Alias(a)) => self.get(&a.clone(), depth + 1),
            Some(Def::WithFallback(c, fallback)) => {
                let (c, fallback) = (*c, fallback.clone());
                if self.theme.contains_key(&fallback) && !self.theme.contains_key(name) {
                    self.get(&fallback, depth + 1)
                } else {
                    c
                }
            }
            None => panic!("unknown palette key `{name}`"),
        };
        self.cache.insert(name.to_string(), value);
        value
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Average color of the theme's `background.png` (the built-in night theme ships a 1x1
/// tile: a solid chat background).
fn background_color(path: &Path) -> u32 {
    let file = fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut zip = zip::ZipArchive::new(file).expect("theme is a zip archive");
    let mut bytes = Vec::new();
    zip.by_name("background.png")
        .expect("theme has background.png")
        .read_to_end(&mut bytes)
        .unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().expect("background.png");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("background.png frame");
    let channels = info.color_type.samples();
    let pixels = &buf[..info.buffer_size()];
    let count = (pixels.len() / channels) as u64;
    let mut sum = [0u64; 3];
    for px in pixels.chunks(channels) {
        for (c, total) in sum.iter_mut().enumerate() {
            *total += u64::from(px[c.min(channels - 1)]);
        }
    }
    let avg = |c: usize| (sum[c] / count) as u32;
    (avg(0) << 24) | (avg(1) << 16) | (avg(2) << 8) | 0xff
}

fn read_zip_entry(path: &Path, entry: &str) -> String {
    let file = fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut zip = zip::ZipArchive::new(file).expect("theme is a zip archive");
    let mut f = zip.by_name(entry).expect("theme has colors.tdesktop-theme");
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/theme");
    let palette_path = assets.join("colors.palette");
    let night_path = assets.join("night.tdesktop-theme");
    println!("cargo:rerun-if-changed={}", palette_path.display());
    println!("cargo:rerun-if-changed={}", night_path.display());

    let palette_list =
        parse(&fs::read_to_string(&palette_path).expect("assets/theme/colors.palette"));
    let palette: HashMap<String, Def> = palette_list.iter().cloned().collect();
    let night: HashMap<String, Def> = parse(&read_zip_entry(&night_path, "colors.tdesktop-theme"))
        .into_iter()
        .filter(|(k, _)| palette.contains_key(k))
        .collect();
    let empty = HashMap::new();

    let mut out = String::from(
        "// @generated by tdr-theme/build.rs from tdesktop's palette. Do not edit.\n\n",
    );
    out.push_str("/// All colors of tdesktop's palette (`lib_ui/ui/colors.palette`).\n");
    out.push_str("#[derive(Debug, Clone, Copy)]\npub struct Palette {\n");
    for (name, _) in &palette_list {
        let _ = writeln!(out, "    /// `{name}`\n    pub {}: Color,", snake(name));
    }
    out.push_str("}\n\n");
    for (const_name, doc, theme) in [
        ("DAY", "Default day theme.", &empty),
        (
            "NIGHT",
            "Built-in night theme (`night.tdesktop-theme`).",
            &night,
        ),
    ] {
        let mut r = Resolver {
            palette: &palette,
            theme,
            cache: HashMap::new(),
        };
        let _ = writeln!(
            out,
            "/// {doc}\npub static {const_name}: Palette = Palette {{"
        );
        for (name, _) in &palette_list {
            let _ = writeln!(out, "    {}: Color({:#010x}),", snake(name), r.get(name, 0));
        }
        out.push_str("};\n\n");
    }
    let _ = writeln!(
        out,
        "/// Chat background of the night theme (its `background.png`).\n\
         pub const NIGHT_CHAT_BACKGROUND: Color = Color({:#010x});\n",
        background_color(&night_path)
    );
    let _ = writeln!(
        out,
        "/// Number of colors in the palette.\npub const COLOR_COUNT: usize = {};",
        palette_list.len()
    );
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("palette.rs"),
        out,
    )
    .unwrap();
}
