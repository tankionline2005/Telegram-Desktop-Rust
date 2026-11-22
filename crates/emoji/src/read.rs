//! `emoji.txt` reading, ported from codegen's `emoji/data_read.cpp`: the same
//! splitting into sections, parts, lines and quoted strings (the order of the list
//! depends on it).

/// One emoji as UTF-16 units, a surrogate pair packed into one value
/// (`high << 16 | low`), as codegen's `InputId`.
pub type InputId = Vec<u32>;

pub struct DoubleColored {
    pub original: InputId,
    pub same: InputId,
    pub different: InputId,
}

#[derive(Default)]
pub struct InputData {
    pub colored: Vec<InputId>,
    pub double_colored: Vec<DoubleColored>,
    pub categories: [Vec<InputId>; 7],
    pub other: Vec<InputId>,
}

type Line = Vec<String>;
type Part = Vec<Line>;
type Section = Vec<Part>;

/// `Skip`: the rest after the separator's first character (none if not found).
fn skip(data: &str, end: Option<usize>) -> &str {
    match end {
        Some(i) => {
            let ch = data[i..].chars().next().map_or(0, char::len_utf8);
            &data[i + ch..]
        }
        None => "",
    }
}

fn head(data: &str, end: Option<usize>) -> &str {
    end.map_or(data, |i| &data[..i])
}

fn read_string(data: &str) -> (Option<String>, &str) {
    let end = data.find(',');
    let parse = head(data, end);
    let start = parse.find('"');
    let quote_end = start.and_then(|s| parse[s + 1..].find('"').map(|e| s + 1 + e));
    if let Some(s) = start {
        let bad = match quote_end {
            None => true,
            Some(e) => parse[e + 1..].contains('"'),
        };
        let _ = s;
        if bad {
            return (None, parse);
        }
    }
    let result = match (start, quote_end) {
        (Some(s), Some(e)) if e > s => parse[s + 1..e].to_string(),
        _ => String::new(),
    };
    (Some(result), skip(data, end))
}

fn read_line(data: &str) -> (Option<Line>, &str) {
    let end = data.find('\n');
    let mut parse = head(data, end);
    let mut result = Line::new();
    loop {
        let (string, updated) = read_string(parse);
        let Some(string) = string else {
            return (None, updated);
        };
        if !string.is_empty() {
            result.push(string);
        }
        if updated.is_empty() {
            break;
        }
        parse = updated;
    }
    (Some(result), skip(data, end))
}

fn read_part(data: &str) -> (Option<Part>, &str) {
    let end = data.find("\n\n").or_else(|| data.find("\r\n\r\n"));
    let mut parse = head(data, end);
    let mut result = Part::new();
    loop {
        let (line, updated) = read_line(parse);
        let Some(line) = line else {
            return (None, updated);
        };
        if !line.is_empty() {
            result.push(line);
        }
        if updated.is_empty() {
            break;
        }
        parse = updated;
    }
    (Some(result), skip(data, end))
}

fn read_section(data: &str) -> (Option<Section>, &str) {
    let end = match (data.find("--------"), data.find("========")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let mut parse = head(data, end);
    let mut result = Section::new();
    loop {
        let (part, updated) = read_part(parse);
        let Some(part) = part else {
            return (None, updated);
        };
        if !part.is_empty() {
            result.push(part);
        }
        if updated.is_empty() {
            break;
        }
        parse = updated;
    }
    (Some(result), skip(data, end))
}

fn read_file(text: &str) -> Option<Vec<Section>> {
    let mut parse = text;
    let mut result = Vec::new();
    loop {
        let (section, updated) = read_section(parse);
        let section = section?;
        if !section.is_empty() {
            result.push(section);
        }
        if updated.is_empty() {
            break;
        }
        parse = updated;
    }
    Some(result)
}

/// `InputIdFromString`.
pub fn input_id(emoji: &str) -> InputId {
    let units: Vec<u16> = emoji.encode_utf16().collect();
    let mut result = InputId::new();
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if (0xD800..0xDC00).contains(&unit) && i + 1 < units.len() {
            result.push((u32::from(unit) << 16) | u32::from(units[i + 1]));
            i += 2;
        } else {
            result.push(u32::from(unit));
            i += 1;
        }
    }
    result
}

fn find_colored_line<'a>(file: &'a [Section], colored: &str) -> Option<&'a Line> {
    file[0]
        .iter()
        .flat_map(|part| part.iter())
        .find(|line| line[0] == colored)
}

/// `ReadData`.
pub fn read_data(text: &str) -> Option<InputData> {
    let file = read_file(text)?;
    if file.len() < 3 || file[0].len() != 8 || file[1].len() > 8 {
        return None;
    }
    let mut result = InputData::default();
    for line in &file[2][0] {
        for string in line {
            let found = find_colored_line(&file, string)?;
            if found.len() != 6 {
                return None;
            }
            let id = input_id(&found[1]);
            if id.len() < 2 {
                return None;
            }
            result.colored.push(id);
        }
    }
    if file[2].len() > 1 {
        for line in &file[2][1] {
            for string in line {
                let found = find_colored_line(&file, string)?;
                if found.len() != 26 {
                    return None;
                }
                result.double_colored.push(DoubleColored {
                    original: input_id(&found[0]),
                    same: input_id(&found[1]),
                    different: input_id(&found[2]),
                });
            }
        }
    }
    let mut index = 0;
    let mut replacements_used = 0;
    for section in &file[0] {
        let first = &section[0][0];
        let replaced = file[1].iter().find(|s| &s[0][0] == first);
        if replaced.is_some() {
            replacements_used += 1;
        }
        for line in replaced.unwrap_or(section) {
            for string in line {
                result.categories[index].push(input_id(string));
            }
        }
        if index + 1 < result.categories.len() {
            index += 1;
        }
    }
    if replacements_used != file[1].len() {
        return None;
    }
    if let Some(other) = file.get(3) {
        for part in other {
            for line in part {
                for string in line {
                    result.other.push(input_id(string));
                }
            }
        }
    }
    Some(result)
}
