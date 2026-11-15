//! One running copy per data folder, as in tdesktop (`Core::Sandbox`: a local server
//! named after the data path). A second start finds the first one's pipe, asks it to
//! show its window ("CMD:show;") and exits.

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::{HSTRING, PCWSTR};

const SHOW: &[u8] = b"CMD:show;";

/// The pipe of the copy that uses `data_dir` (`SingleInstanceLocalServerName`: a
/// hash of the path).
fn pipe_name(data_dir: &Path) -> HSTRING {
    let path = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let text = path.to_string_lossy().to_lowercase();
    // FNV-1a: stable across runs and builds.
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    });
    HSTRING::from(format!(
        r"\\.\pipe\unofficial-telegram-desktop-rust-{hash:016x}"
    ))
}

/// Becomes the running copy (and calls `show` whenever another start asks for the
/// window), or hands over to the running one: then returns `false` and this process
/// should exit.
pub fn claim(data_dir: &Path, show: impl Fn() + Send + 'static) -> bool {
    let name = pipe_name(data_dir);
    let server = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            512,
            0,
            None,
        )
    };
    if server == INVALID_HANDLE_VALUE {
        let error = unsafe { GetLastError() };
        if error == ERROR_ACCESS_DENIED {
            // Another copy owns the pipe.
            if hand_over(&name) {
                return false;
            }
            tracing::warn!("the running copy did not answer; starting anyway");
        } else {
            tracing::warn!(?error, "single instance pipe");
        }
        return true;
    }
    let server = SendHandle(server);
    std::thread::Builder::new()
        .name("instance".into())
        .spawn(move || serve(server, show))
        .expect("spawn instance thread");
    true
}

struct SendHandle(HANDLE);
// The handle is only used by the thread it is moved to.
unsafe impl Send for SendHandle {}

fn serve(server: SendHandle, show: impl Fn()) {
    let server = server.0;
    loop {
        // A client may have connected (and even written and left) before this call:
        // what it wrote is still there to read.
        let ready = match unsafe { ConnectNamedPipe(server, None) } {
            Ok(()) => true,
            Err(e) => {
                e.code() == ERROR_PIPE_CONNECTED.to_hresult()
                    || e.code() == ERROR_NO_DATA.to_hresult()
            }
        };
        if !ready {
            let _ = unsafe { DisconnectNamedPipe(server) };
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        let mut buf = [0u8; 512];
        let mut read = 0u32;
        let mut got = Vec::new();
        while unsafe { ReadFile(server, Some(&mut buf), Some(&mut read), None) }.is_ok() && read > 0
        {
            got.extend_from_slice(&buf[..read as usize]);
            if got.len() > 4096 {
                break;
            }
        }
        if got.windows(SHOW.len()).any(|w| w == SHOW) {
            tracing::info!("another start asked for the window");
            show();
        }
        let _ = unsafe { DisconnectNamedPipe(server) };
    }
}

/// Tells the running copy to show its window. `false` when it could not be reached.
fn hand_over(name: &HSTRING) -> bool {
    for _ in 0..10 {
        let file = unsafe {
            CreateFileW(
                name,
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        };
        match file {
            Ok(h) => {
                // Let it take the foreground: this process just got it from the user.
                let mut pid = 0u32;
                if unsafe { GetNamedPipeServerProcessId(h, &mut pid) }.is_ok() {
                    let _ = unsafe { AllowSetForegroundWindow(pid) };
                }
                let mut written = 0u32;
                let ok = unsafe { WriteFile(h, Some(SHOW), Some(&mut written), None) }.is_ok();
                let _ = unsafe { CloseHandle(h) };
                return ok;
            }
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 1000) };
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn second_start_hands_over_and_shows_the_first() {
        let dir = std::env::temp_dir().join(format!("tdr-instance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shown = Arc::new(AtomicUsize::new(0));
        let counter = shown.clone();
        assert!(claim(&dir, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        // The second start finds the pipe, asks for the window and steps back.
        assert!(!claim(&dir, || panic!("the second copy must not serve")));
        for _ in 0..50 {
            if shown.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        // Another folder is another copy.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert!(claim(&other, || {}));
    }
}
