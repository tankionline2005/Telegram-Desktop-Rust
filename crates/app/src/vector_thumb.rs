//! Telegram's vector thumbnails (`photoPathSize`): a compressed SVG path in a
//! 512 x 512 square (tdesktop's `Images::ExpandPathInlineBytes` and
//! `PathFromInlineBytes`), drawn as a silhouette while a sticker or custom emoji
//! loads.

use resvg::tiny_skia::{Path, PathBuilder};

/// `ExpandPathInlineBytes`: the bytes as SVG path text.
pub fn expand(bytes: &[u8]) -> String {
    const LOOKUP: &[u8] = b"AACAAAAHAAALMAAAQASTAVAAAZaacaaaahaaalmaaaqastava.az0123456789-,";
    let mut out = String::with_capacity(3 * (bytes.len() + 1));
    out.push('M');
    for &c in bytes {
        if c >= 128 + 64 {
            if let Some(&ch) = LOOKUP.get(usize::from(c - 128 - 64)) {
                out.push(char::from(ch));
            }
        } else {
            if c >= 128 {
                out.push(',');
            } else if c >= 64 {
                out.push('-');
            }
            out.push_str(&(c & 63).to_string());
        }
    }
    out.push('z');
    out
}

/// `PathFromInlineBytes`: M/m, L/l, C/c and Z/z, numbers split by commas or a
/// minus sign.
pub fn parse(bytes: &[u8]) -> Option<Path> {
    if bytes.is_empty() {
        return None;
    }
    let text = expand(bytes);
    let s = text.as_bytes();
    let mut at = 0usize;
    let skip_commas = |at: &mut usize| {
        while s.get(*at) == Some(&b',') {
            *at += 1;
        }
    };
    let number = |at: &mut usize| -> Option<f32> {
        skip_commas(at);
        let start = *at;
        if s.get(*at) == Some(&b'-') {
            *at += 1;
        }
        while s.get(*at).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            *at += 1;
        }
        let v = std::str::from_utf8(&s[start..*at]).ok()?.parse().ok();
        skip_commas(at);
        v
    };
    let starts_number = |at: usize| {
        s.get(at)
            .is_some_and(|c| c.is_ascii_digit() || *c == b'-' || *c == b'.')
    };
    let mut pb = PathBuilder::new();
    let (mut x, mut y, mut sx, mut sy) = (0f32, 0f32, 0f32, 0f32);
    let mut command = b'M';
    while at < s.len() {
        skip_commas(&mut at);
        let Some(&c) = s.get(at) else {
            break;
        };
        if c.is_ascii_alphabetic() {
            command = c;
            at += 1;
            if command == b'z' || command == b'Z' {
                pb.close();
                x = sx;
                y = sy;
                continue;
            }
        } else if !starts_number(at) {
            at += 1;
            continue;
        }
        match command {
            b'M' | b'm' => {
                let (nx, ny) = (number(&mut at)?, number(&mut at)?);
                if command == b'm' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                sx = x;
                sy = y;
                pb.move_to(x, y);
                // Pairs after a move are lines (SVG).
                command = if command == b'm' { b'l' } else { b'L' };
            }
            b'L' | b'l' => {
                let (nx, ny) = (number(&mut at)?, number(&mut at)?);
                if command == b'l' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                pb.line_to(x, y);
            }
            b'C' | b'c' => {
                let mut v = [0f32; 6];
                for n in &mut v {
                    *n = number(&mut at)?;
                }
                if command == b'c' {
                    for k in 0..3 {
                        v[2 * k] += x;
                        v[2 * k + 1] += y;
                    }
                }
                pb.cubic_to(v[0], v[1], v[2], v[3], v[4], v[5]);
                x = v[4];
                y = v[5];
            }
            _ => {
                // Anything else is not in these thumbnails: skip its number.
                number(&mut at)?;
            }
        }
    }
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_and_parses() {
        // "M" + "10,20" + "l" + "-5,5" + "z"
        let bytes = [10u8, 128 + 20, 192 + 37, 64 + 5, 128 + 5];
        assert_eq!(expand(&bytes), "M10,20l-5,5z");
        let path = parse(&bytes).unwrap();
        let b = path.bounds();
        assert_eq!(
            (b.left(), b.top(), b.right(), b.bottom()),
            (5., 20., 10., 25.)
        );
    }
}
