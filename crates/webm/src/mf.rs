//! VP9 decoding with Media Foundation's VP9 decoder (the "VP9 Video Extensions"
//! that Windows ships), a synchronous MFT fed frame by frame; NV12 out.

use std::mem::ManuallyDrop;
use std::sync::Once;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};

use crate::{Decoded, Error, Frame, Video, nv12_to_rgba};

static STARTUP: Once = Once::new();

fn err(e: windows::core::Error) -> Error {
    Error::Decode(e.message())
}

struct Picture {
    data: Vec<u8>,
    stride: usize,
    rows: usize,
}

struct Decoder {
    mft: IMFTransform,
    provides_samples: bool,
    sample_size: u32,
    width: u32,
    height: u32,
    stride: usize,
}

impl Decoder {
    fn new(width: u32, height: u32) -> Result<Self, Error> {
        unsafe {
            let input = MFT_REGISTER_TYPE_INFO {
                guidMajorType: MFMediaType_Video,
                guidSubtype: MFVideoFormat_VP90,
            };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                None,
                &mut list,
                &mut count,
            )
            .map_err(err)?;
            let mut mft: Option<IMFTransform> = None;
            for i in 0..count as usize {
                let slot = &mut *list.add(i);
                if mft.is_none()
                    && let Some(activate) = slot.as_ref()
                {
                    mft = activate.ActivateObject::<IMFTransform>().ok();
                }
                drop(slot.take());
            }
            if !list.is_null() {
                CoTaskMemFree(Some(list as *const _));
            }
            let mft = mft.ok_or(Error::NoDecoder)?;

            let in_type = MFCreateMediaType().map_err(err)?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(err)?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_VP90)
                .map_err(err)?;
            in_type
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(width) << 32) | u64::from(height),
                )
                .map_err(err)?;
            mft.SetInputType(0, &in_type, 0).map_err(err)?;
            let mut decoder = Decoder {
                mft,
                provides_samples: false,
                sample_size: 0,
                width,
                height,
                stride: width as usize,
            };
            decoder.choose_output()?;
            let _ = decoder
                .mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = decoder
                .mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Ok(decoder)
        }
    }

    /// NV12 output (again after a stream change).
    fn choose_output(&mut self) -> Result<(), Error> {
        unsafe {
            let mut index = 0;
            loop {
                let t = self.mft.GetOutputAvailableType(0, index).map_err(err)?;
                if t.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                    self.mft.SetOutputType(0, &t, 0).map_err(err)?;
                    if let Ok(size) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                        self.width = (size >> 32) as u32;
                        self.height = size as u32;
                    }
                    self.stride = t
                        .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                        .map(|s| s as i32)
                        .ok()
                        .filter(|s| *s > 0)
                        .map_or(self.width as usize, |s| s as usize);
                    break;
                }
                index += 1;
            }
            let info = self.mft.GetOutputStreamInfo(0).map_err(err)?;
            self.provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            self.sample_size = info.cbSize.max(self.stride as u32 * self.height * 3 / 2);
            Ok(())
        }
    }

    fn feed(&mut self, data: &[u8], time_ms: u64, out: &mut Vec<Picture>) -> Result<(), Error> {
        unsafe {
            let sample = MFCreateSample().map_err(err)?;
            let buffer = MFCreateMemoryBuffer(data.len() as u32).map_err(err)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None).map_err(err)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock().map_err(err)?;
            buffer.SetCurrentLength(data.len() as u32).map_err(err)?;
            sample.AddBuffer(&buffer).map_err(err)?;
            sample.SetSampleTime(time_ms as i64 * 10_000).map_err(err)?;
            loop {
                match self.mft.ProcessInput(0, &sample, 0) {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING => self.pull(out)?,
                    Err(e) => return Err(err(e)),
                }
            }
            self.pull(out)
        }
    }

    fn drain(&mut self, out: &mut Vec<Picture>) -> Result<(), Error> {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        self.pull(out)
    }

    fn pull(&mut self, out: &mut Vec<Picture>) -> Result<(), Error> {
        unsafe {
            loop {
                let own = if self.provides_samples {
                    None
                } else {
                    let s = MFCreateSample().map_err(err)?;
                    let b = MFCreateMemoryBuffer(self.sample_size).map_err(err)?;
                    s.AddBuffer(&b).map_err(err)?;
                    Some(s)
                };
                let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(own),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.mft.ProcessOutput(0, &mut buffers, &mut status);
                let sample = ManuallyDrop::take(&mut buffers[0].pSample);
                drop(ManuallyDrop::take(&mut buffers[0].pEvents));
                match result {
                    Ok(()) => {
                        if let Some(sample) = sample {
                            let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
                            let mut ptr = std::ptr::null_mut();
                            let mut len = 0u32;
                            buffer.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
                            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                            buffer.Unlock().map_err(err)?;
                            let rows = (data.len() * 2 / 3) / self.stride.max(1);
                            out.push(Picture {
                                data,
                                stride: self.stride,
                                rows: rows.max(self.height as usize),
                            });
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.choose_output()?,
                    Err(e) => return Err(err(e)),
                }
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

pub fn decode(video: &Video, max_frames: usize) -> Result<Decoded, Error> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        STARTUP.call_once(|| {
            let _ = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        });
    }
    let (w, h) = (video.width.max(1), video.height.max(1));
    let mut color = Decoder::new(w, h)?;
    let with_alpha = video.blocks.iter().any(|b| b.alpha.is_some());
    let mut alpha = if with_alpha {
        Some(Decoder::new(w, h)?)
    } else {
        None
    };
    let blocks = &video.blocks[..video.blocks.len().min(max_frames)];
    let mut pictures = Vec::new();
    let mut alphas = Vec::new();
    for b in blocks {
        color.feed(&b.data, b.time_ms, &mut pictures)?;
        if let (Some(a), Some(data)) = (&mut alpha, &b.alpha) {
            a.feed(data, b.time_ms, &mut alphas)?;
        }
    }
    color.drain(&mut pictures)?;
    if let Some(a) = &mut alpha {
        a.drain(&mut alphas)?;
    }
    let (width, height) = (w as usize, h as usize);
    let mut frames = Vec::with_capacity(pictures.len());
    for (k, p) in pictures.iter().enumerate() {
        let a = alphas.get(k).map(|a| (a.data.as_slice(), a.stride));
        let rgba = nv12_to_rgba(&p.data, p.stride, p.rows, width, height, a);
        let next = blocks.get(k + 1).map(|b| b.time_ms).unwrap_or_else(|| {
            video
                .duration_ms
                .max(blocks[k.min(blocks.len() - 1)].time_ms + 33)
        });
        let this = blocks.get(k).map_or(0, |b| b.time_ms);
        frames.push(Frame {
            rgba,
            duration_ms: next.saturating_sub(this).clamp(1, 1000) as u32,
        });
    }
    Ok(Decoded {
        width: w,
        height: h,
        frames,
    })
}
