#[cfg(windows)]
fn main() {
    use windows::Win32::Media::MediaFoundation::*;
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap();
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_VP90,
        };
        for (name, flags) in [("sync", MFT_ENUM_FLAG_SYNCMFT), ("all", MFT_ENUM_FLAG_ALL)] {
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            let r = MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                flags,
                Some(&input),
                None,
                &mut list,
                &mut count,
            );
            println!("{name}: {r:?} count {count}");
            for i in 0..count as usize {
                if let Some(a) = &*list.add(i) {
                    let mut buf = [0u16; 256];
                    let mut len = 0u32;
                    let _ = a.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len));
                    println!("  {}", String::from_utf16_lossy(&buf[..len as usize]));
                }
            }
        }
    }
}
#[cfg(not(windows))]
fn main() {}
