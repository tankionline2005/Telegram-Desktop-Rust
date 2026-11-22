//! A tiny TrueType font built in memory that only reserves room for emoji images.
//!
//! GPUI lays text out by itself, with no inline objects. Emoji are drawn as images
//! (tdesktop's sprites), so the text shaped for layout gets, in place of each emoji,
//! a Private Use Area character of this font whose advance is the emoji's width,
//! followed by zero-width filler characters up to the emoji's length in bytes: byte
//! offsets stay those of the real text, so wrapping, selection and the caret work
//! unchanged. All glyphs are empty.

/// The family name to use in text runs.
pub const FAMILY: &str = "TdrEmojiSpace";
/// The zero-width filler character.
pub const FILLER: char = 'x';
/// Units per em; the advance of `WIDTH_BASE + k` is `k + 1` units.
const UNITS_PER_EM: u16 = 2048;
const WIDTH_BASE: u32 = 0xE000;
/// Advances from 1 unit to three ems.
const WIDTHS: u32 = 3 * UNITS_PER_EM as u32;

/// The character whose advance is `width` at font size `size` (both in the same
/// pixels).
pub fn space_char(width: f32, size: f32) -> char {
    let units = (width / size * f32::from(UNITS_PER_EM)).round() as u32;
    let units = units.clamp(1, WIDTHS);
    char::from_u32(WIDTH_BASE + units - 1).unwrap()
}

#[cfg(test)]
/// Whether `ch` is one of this font's spaces.
pub fn is_space(ch: char) -> bool {
    (WIDTH_BASE..WIDTH_BASE + WIDTHS).contains(&u32::from(ch))
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

/// The font file.
pub fn build() -> Vec<u8> {
    // Glyph 0: .notdef, 1: the filler, 2..: the spaces.
    let num_glyphs = (2 + WIDTHS) as u16;
    let ascender: i16 = 1600;
    let descender: i16 = -400;

    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000); // version
    be32(&mut head, 0x0001_0000); // fontRevision
    be32(&mut head, 0); // checkSumAdjustment, set below
    be32(&mut head, 0x5F0F_3CF5); // magicNumber
    be16(&mut head, 0x000B); // flags
    be16(&mut head, UNITS_PER_EM);
    head.extend_from_slice(&[0; 16]); // created, modified
    for v in [0i16, descender, UNITS_PER_EM as i16, ascender] {
        be16(&mut head, v as u16); // xMin, yMin, xMax, yMax
    }
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16(&mut head, 2); // fontDirectionHint
    be16(&mut head, 0); // indexToLocFormat: short
    be16(&mut head, 0); // glyphDataFormat

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    be16(&mut hhea, ascender as u16);
    be16(&mut hhea, descender as u16);
    be16(&mut hhea, 0); // lineGap
    be16(&mut hhea, WIDTHS as u16); // advanceWidthMax
    be16(&mut hhea, 0); // minLeftSideBearing
    be16(&mut hhea, 0); // minRightSideBearing
    be16(&mut hhea, 0); // xMaxExtent
    be16(&mut hhea, 1); // caretSlopeRise
    be16(&mut hhea, 0); // caretSlopeRun
    be16(&mut hhea, 0); // caretOffset
    hhea.extend_from_slice(&[0; 8]);
    be16(&mut hhea, 0); // metricDataFormat
    be16(&mut hhea, num_glyphs); // numberOfHMetrics

    let mut hmtx = Vec::new();
    for glyph in 0..u32::from(num_glyphs) {
        let advance = if glyph < 2 { 0 } else { glyph - 1 };
        be16(&mut hmtx, advance as u16);
        be16(&mut hmtx, 0);
    }

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, num_glyphs);
    for v in [0u16, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut maxp, v);
    }

    // All glyphs are empty: every offset is 0.
    let glyf = vec![0u8; 4];
    let loca = vec![0u8; (usize::from(num_glyphs) + 1) * 2];

    // cmap: format 4, Windows Unicode BMP.
    let segments: [(u16, u16, i16); 3] = [
        (
            FILLER as u16,
            FILLER as u16,
            1i16.wrapping_sub(FILLER as i16),
        ),
        (
            WIDTH_BASE as u16,
            (WIDTH_BASE + WIDTHS - 1) as u16,
            (2i32 - WIDTH_BASE as i32) as i16,
        ),
        (0xFFFF, 0xFFFF, 1),
    ];
    let seg_x2 = (segments.len() * 2) as u16;
    let mut sub = Vec::new();
    be16(&mut sub, 4);
    be16(&mut sub, 16 + 8 * segments.len() as u16); // length
    be16(&mut sub, 0); // language
    be16(&mut sub, seg_x2);
    let search = 2 * (1u16 << (segments.len() as f32).log2().floor() as u16);
    be16(&mut sub, search);
    be16(&mut sub, (search / 2).trailing_zeros() as u16);
    be16(&mut sub, seg_x2 - search);
    for (_, end, _) in segments {
        be16(&mut sub, end);
    }
    be16(&mut sub, 0); // reservedPad
    for (start, _, _) in segments {
        be16(&mut sub, start);
    }
    for (_, _, delta) in segments {
        be16(&mut sub, delta as u16);
    }
    for _ in segments {
        be16(&mut sub, 0); // idRangeOffset
    }
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 1);
    be16(&mut cmap, 3);
    be16(&mut cmap, 1);
    be32(&mut cmap, 12);
    cmap.extend_from_slice(&sub);

    let names: [(u16, &str); 4] = [(1, FAMILY), (2, "Regular"), (4, FAMILY), (6, FAMILY)];
    let mut name = Vec::new();
    be16(&mut name, 0);
    be16(&mut name, names.len() as u16);
    be16(&mut name, 6 + 12 * names.len() as u16);
    let mut strings = Vec::new();
    for (id, text) in names {
        let encoded: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        be16(&mut name, 3);
        be16(&mut name, 1);
        be16(&mut name, 0x0409);
        be16(&mut name, id);
        be16(&mut name, encoded.len() as u16);
        be16(&mut name, strings.len() as u16);
        strings.extend_from_slice(&encoded);
    }
    name.extend_from_slice(&strings);

    let mut os2 = Vec::new();
    be16(&mut os2, 4); // version
    be16(&mut os2, UNITS_PER_EM / 2); // xAvgCharWidth
    be16(&mut os2, 400); // usWeightClass
    be16(&mut os2, 5); // usWidthClass
    be16(&mut os2, 0); // fsType
    for _ in 0..10 {
        be16(&mut os2, 0); // subscript, superscript, strikeout metrics
    }
    be16(&mut os2, 0); // sFamilyClass
    os2.extend_from_slice(&[0; 10]); // panose
    os2.extend_from_slice(&[0; 16]); // ulUnicodeRange
    os2.extend_from_slice(b"NONE"); // achVendID
    be16(&mut os2, 0x0040); // fsSelection: regular
    be16(&mut os2, FILLER as u16); // usFirstCharIndex
    be16(&mut os2, 0xFFFF); // usLastCharIndex
    be16(&mut os2, ascender as u16); // sTypoAscender
    be16(&mut os2, descender as u16); // sTypoDescender
    be16(&mut os2, 0); // sTypoLineGap
    be16(&mut os2, ascender as u16); // usWinAscent
    be16(&mut os2, (-descender) as u16); // usWinDescent
    be32(&mut os2, 1); // ulCodePageRange1: Latin 1
    be32(&mut os2, 0);
    be16(&mut os2, 0); // sxHeight
    be16(&mut os2, 0); // sCapHeight
    be16(&mut os2, 0); // usDefaultChar
    be16(&mut os2, FILLER as u16); // usBreakChar
    be16(&mut os2, 0); // usMaxContext

    let mut post = Vec::new();
    be32(&mut post, 0x0003_0000);
    post.extend_from_slice(&[0; 28]);

    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"OS/2", os2),
        (*b"cmap", cmap),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"name", name),
        (*b"post", post),
    ];
    tables.sort_by_key(|t| t.0);
    let count = tables.len() as u16;
    let mut font = Vec::new();
    be32(&mut font, 0x0001_0000);
    be16(&mut font, count);
    let entry = 1u16 << (f32::from(count).log2().floor() as u16);
    be16(&mut font, entry * 16);
    be16(&mut font, entry.trailing_zeros() as u16);
    be16(&mut font, count * 16 - entry * 16);
    let mut offset = 12 + 16 * tables.len();
    let mut head_offset = 0;
    let mut body = Vec::new();
    for (tag, data) in &tables {
        font.extend_from_slice(tag);
        be32(&mut font, checksum(data));
        be32(&mut font, offset as u32);
        be32(&mut font, data.len() as u32);
        if tag == b"head" {
            head_offset = offset;
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    font.extend_from_slice(&body);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_round_trip() {
        let ch = space_char(20., 13.);
        assert!(is_space(ch));
        assert_eq!(ch.len_utf8(), 3);
        let units = u32::from(ch) - WIDTH_BASE + 1;
        assert_eq!(units, (20. / 13. * 2048.0f32).round() as u32);
    }

    #[test]
    fn font_has_tables() {
        let font = build();
        assert_eq!(&font[..4], &[0, 1, 0, 0]);
        assert_eq!(font.len() % 4, 0);
    }
}
