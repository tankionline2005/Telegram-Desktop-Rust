//! Matroska (EBML) demuxing: the first video track, its blocks with their times
//! and the alpha stream (`BlockAdditional` with `BlockAddID` 1).

/// A video frame as stored.
#[derive(Clone, Debug)]
pub struct Block {
    /// Milliseconds from the start.
    pub time_ms: u64,
    pub data: Vec<u8>,
    pub alpha: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct Video {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub alpha_mode: bool,
    /// Total length, milliseconds (0 when unknown).
    pub duration_ms: u64,
    pub blocks: Vec<Block>,
}

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const ALPHA_MODE: u32 = 0x53C0;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_ADDITIONS: u32 = 0x75A1;
const BLOCK_MORE: u32 = 0xA6;
const BLOCK_ADD_ID: u32 = 0xEE;
const BLOCK_ADDITIONAL: u32 = 0xA5;

/// An element id (marker bits kept) and its length in bytes.
fn read_id(data: &[u8]) -> Option<(u32, usize)> {
    let first = *data.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 4 || data.len() < len {
        return None;
    }
    let mut id = 0u32;
    for b in &data[..len] {
        id = (id << 8) | u32::from(*b);
    }
    Some((id, len))
}

/// A size (marker bit removed; `None` value for "unknown") and its length.
fn read_size(data: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *data.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || data.len() < len {
        return None;
    }
    let mut value = u64::from(first) & (0xFF >> len);
    let mut all_ones = value == (0xFF >> len);
    for b in &data[1..len] {
        value = (value << 8) | u64::from(*b);
        all_ones &= *b == 0xFF;
    }
    Some((if all_ones { None } else { Some(value) }, len))
}

fn uint(data: &[u8]) -> u64 {
    data.iter().fold(0, |v, b| (v << 8) | u64::from(*b))
}

fn float(data: &[u8]) -> f64 {
    match data.len() {
        4 => f64::from(f32::from_be_bytes(data.try_into().unwrap())),
        8 => f64::from_be_bytes(data.try_into().unwrap()),
        _ => 0.,
    }
}

/// The children of an element body: (id, body).
fn children(mut data: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let Some((id, il)) = read_id(data) else {
            break;
        };
        let Some((size, sl)) = read_size(&data[il..]) else {
            break;
        };
        let start = il + sl;
        let end = match size {
            Some(s) => (start + s as usize).min(data.len()),
            // Unknown size (live streams): to the end of the parent.
            None => data.len(),
        };
        if start > end {
            break;
        }
        out.push((id, &data[start..end]));
        data = &data[end..];
    }
    out
}

struct Frame<'a> {
    track: u64,
    timecode: i16,
    data: &'a [u8],
}

fn parse_block(data: &[u8]) -> Option<Frame<'_>> {
    let (track, tl) = read_size(data)?;
    let rest = &data[tl..];
    if rest.len() < 3 {
        return None;
    }
    let timecode = i16::from_be_bytes([rest[0], rest[1]]);
    let flags = rest[2];
    // Lacing is not used for video frames.
    if flags & 0x06 != 0 {
        return None;
    }
    Some(Frame {
        track: track?,
        timecode,
        data: &rest[3..],
    })
}

pub fn demux(bytes: &[u8]) -> Option<Video> {
    let top = children(bytes);
    if top.first().map(|t| t.0) != Some(EBML) {
        return None;
    }
    let (_, segment) = top.iter().find(|t| t.0 == SEGMENT)?;
    let mut video = Video::default();
    let mut scale_ns = 1_000_000u64;
    let mut duration = 0.;
    let mut track_number = None;
    for (id, body) in children(segment) {
        match id {
            INFO => {
                for (id, v) in children(body) {
                    match id {
                        TIMECODE_SCALE => scale_ns = uint(v).max(1),
                        DURATION => duration = float(v),
                        _ => {}
                    }
                }
            }
            TRACKS => {
                for (id, entry) in children(body) {
                    if id != TRACK_ENTRY || track_number.is_some() {
                        continue;
                    }
                    let fields = children(entry);
                    if fields
                        .iter()
                        .find(|f| f.0 == TRACK_TYPE)
                        .is_some_and(|f| uint(f.1) != 1)
                    {
                        continue;
                    }
                    for (id, v) in fields {
                        match id {
                            TRACK_NUMBER => track_number = Some(uint(v)),
                            CODEC_ID => video.codec = String::from_utf8_lossy(v).into(),
                            VIDEO => {
                                for (id, v) in children(v) {
                                    match id {
                                        PIXEL_WIDTH => video.width = uint(v) as u32,
                                        PIXEL_HEIGHT => video.height = uint(v) as u32,
                                        ALPHA_MODE => video.alpha_mode = uint(v) != 0,
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            CLUSTER => {
                let mut cluster_time = 0u64;
                for (id, v) in children(body) {
                    match id {
                        TIMECODE => cluster_time = uint(v),
                        SIMPLE_BLOCK | BLOCK_GROUP => {
                            let (block, alpha) = if id == SIMPLE_BLOCK {
                                (Some(v), None)
                            } else {
                                let mut block = None;
                                let mut alpha = None;
                                for (id, v) in children(v) {
                                    match id {
                                        BLOCK => block = Some(v),
                                        BLOCK_ADDITIONS => {
                                            for (id, more) in children(v) {
                                                if id != BLOCK_MORE {
                                                    continue;
                                                }
                                                let fields = children(more);
                                                let add_id = fields
                                                    .iter()
                                                    .find(|f| f.0 == BLOCK_ADD_ID)
                                                    .map_or(1, |f| uint(f.1));
                                                if add_id == 1
                                                    && let Some(f) = fields
                                                        .iter()
                                                        .find(|f| f.0 == BLOCK_ADDITIONAL)
                                                {
                                                    alpha = Some(f.1.to_vec());
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                (block, alpha)
                            };
                            let Some(frame) = block.and_then(parse_block) else {
                                continue;
                            };
                            if Some(frame.track) != track_number && track_number.is_some() {
                                continue;
                            }
                            let ticks = (cluster_time as i64 + i64::from(frame.timecode)).max(0);
                            video.blocks.push(Block {
                                time_ms: ticks as u64 * scale_ns / 1_000_000,
                                data: frame.data.to_vec(),
                                alpha,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    video.blocks.sort_by_key(|b| b.time_ms);
    video.duration_ms = (duration * scale_ns as f64 / 1_000_000.) as u64;
    Some(video)
}
