//! WebM video stickers and emoji (Telegram's `video/webm`: VP9, often with an alpha
//! channel stored as a second VP9 stream in each block's `BlockAdditional`):
//! Matroska demuxing here, VP9 decoding by the system (Media Foundation's VP9
//! decoder on Windows; tdesktop uses FFmpeg's libvpx).

#[cfg(windows)]
mod mf;
mod mkv;

pub use mkv::{Block, Video, demux};

/// A decoded frame: straight (not premultiplied) RGBA.
#[derive(Clone)]
pub struct Frame {
    pub rgba: Vec<u8>,
    /// How long it is shown, milliseconds.
    pub duration_ms: u32,
}

/// All frames of a video, `width` x `height`.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
}

#[derive(Debug)]
pub enum Error {
    Container,
    NoDecoder,
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Container => f.write_str("not a vp9 webm"),
            Error::NoDecoder => f.write_str("no vp9 decoder"),
            Error::Decode(e) => write!(f, "vp9: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Decodes every frame, at most `max_frames`.
pub fn decode(bytes: &[u8], max_frames: usize) -> Result<Decoded, Error> {
    let video = demux(bytes).ok_or(Error::Container)?;
    if video.codec != "V_VP9" || video.blocks.is_empty() {
        return Err(Error::Container);
    }
    #[cfg(windows)]
    {
        mf::decode(&video, max_frames)
    }
    #[cfg(not(windows))]
    {
        let _ = max_frames;
        Err(Error::NoDecoder)
    }
}

/// Planar 4:2:0 (NV12 layout: Y plane, then interleaved UV) to RGB, BT.601 limited
/// range as libvpx streams are tagged.
pub(crate) fn nv12_to_rgba(
    data: &[u8],
    stride: usize,
    rows: usize,
    width: usize,
    height: usize,
    alpha: Option<(&[u8], usize)>,
) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    let uv = rows * stride;
    for y in 0..height {
        for x in 0..width {
            let yy = f32::from(*data.get(y * stride + x).unwrap_or(&16)) - 16.;
            let i = uv + (y / 2) * stride + (x / 2) * 2;
            let u = f32::from(*data.get(i).unwrap_or(&128)) - 128.;
            let v = f32::from(*data.get(i + 1).unwrap_or(&128)) - 128.;
            let c = 1.164 * yy;
            let r = c + 1.596 * v;
            let g = c - 0.392 * u - 0.813 * v;
            let b = c + 2.017 * u;
            let a = match alpha {
                Some((plane, astride)) => {
                    let ay = f32::from(*plane.get(y * astride + x).unwrap_or(&235));
                    ((ay - 16.) * 255. / 219.).clamp(0., 255.)
                }
                None => 255.,
            };
            let o = (y * width + x) * 4;
            out[o] = r.clamp(0., 255.) as u8;
            out[o + 1] = g.clamp(0., 255.) as u8;
            out[o + 2] = b.clamp(0., 255.) as u8;
            out[o + 3] = a as u8;
        }
    }
    out
}
