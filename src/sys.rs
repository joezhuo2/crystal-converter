use crate::config::Config;
use std::{env, path::Path, sync::Mutex};
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Media::Audio::{PlaySoundW, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT},
        System::{
            Registry::{
                RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
            },
            Threading::{GetCurrentProcess, SetProcessWorkingSetSize},
        },
    },
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Crystal Converter";
const MENU_KEY: &str = r"Software\Classes\*\shell\CrystalConverter";

fn reg_set(key: &str, name: Option<&str>, value: &str) {
    let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let name = name.map(HSTRING::from);
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        );
    }
}

pub fn apply(config: &Config) {
    let exe = env::current_exe().unwrap_or_default();
    let quoted = format!("\"{}\"", exe.display());
    unsafe {
        if config.launch_at_login {
            reg_set(RUN_KEY, Some(RUN_VALUE), &quoted);
        } else {
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN_KEY),
                &HSTRING::from(RUN_VALUE),
            );
        }
        if config.context_menu {
            reg_set(MENU_KEY, None, "Convert with Crystal Converter");
            reg_set(MENU_KEY, Some("Icon"), &exe.display().to_string());
            reg_set(
                &format!(r"{MENU_KEY}\command"),
                None,
                &format!("{quoted} --open \"%1\""),
            );
        } else {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(MENU_KEY));
        }
    }
}

pub fn open(target: &str) {
    let _ = std::process::Command::new("explorer").arg(target).spawn();
}

pub fn reveal(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

pub fn recycle(paths: &[std::path::PathBuf]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    let existing: Vec<_> = paths.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return true;
    }
    let mut from: Vec<u16> = Vec::new();
    for p in existing {
        from.extend(p.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 }
}

pub fn copy_files(owner: windows::Win32::Foundation::HWND, paths: &[std::path::PathBuf]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::{
        Foundation::{GlobalFree, HANDLE, POINT},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_HDROP,
        },
        UI::Shell::DROPFILES,
    };
    let mut wide: Vec<u16> = Vec::new();
    for p in paths {
        wide.extend(p.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);
    let header = std::mem::size_of::<DROPFILES>();
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, header + wide.len() * 2) else {
            return false;
        };
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        let df = DROPFILES {
            pFiles: header as u32,
            pt: POINT::default(),
            fNC: false.into(),
            fWide: true.into(),
        };
        std::ptr::write_unaligned(ptr.cast::<DROPFILES>(), df);
        std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), ptr.add(header), wide.len() * 2);
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(owner)).is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(u32::from(CF_HDROP.0), Some(HANDLE(mem.0))).is_ok();
        let _ = CloseClipboard();
        if !ok {
            let _ = GlobalFree(Some(mem));
        }
        ok
    }
}

pub fn copy_text(owner: windows::Win32::Foundation::HWND, text: &str) -> bool {
    use windows::Win32::{
        Foundation::{GlobalFree, HANDLE},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
        },
    };
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            return false;
        };
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(owner)).is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))).is_ok();
        let _ = CloseClipboard();
        if !ok {
            let _ = GlobalFree(Some(mem));
        }
        ok
    }
}

pub struct KeepAwake;

pub fn keep_awake() -> KeepAwake {
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    unsafe {
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
    }
    KeepAwake
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn toast(title: &str, body: &str) {
    use windows::{
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{ToastNotification, ToastNotificationManager},
    };
    const AUMID: &str = "CrystalConverter";
    reg_set(
        &format!(r"Software\Classes\AppUserModelId\{AUMID}"),
        Some("DisplayName"),
        "Crystal Converter",
    );
    let xml = format!(
        "<toast><visual><binding template=\"ToastText02\"><text id=\"1\">{}</text><text id=\"2\">{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let _ = (|| -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
    })();
}

#[derive(Clone, Copy)]
pub enum Sound {
    Start,
    Done,
    Error,
}

fn synth(notes: &[(f32, f32, f32)]) -> Vec<u8> {
    const RATE: u32 = 44100;
    let total = notes
        .iter()
        .map(|(_, start, dur)| start + dur)
        .fold(0.0, f32::max);
    let n = (total * RATE as f32) as usize;
    let mut samples = vec![0f32; n];
    for &(freq, start, dur) in notes {
        let s0 = (start * RATE as f32) as usize;
        for i in 0..(dur * RATE as f32) as usize {
            let t = i as f32 / RATE as f32;
            let env = (t / 0.005).min(1.0) * (-t * 5.0 / dur).exp();
            let w = std::f32::consts::TAU * freq * t;
            if let Some(s) = samples.get_mut(s0 + i) {
                *s += env * (w.sin() + 0.25 * (2.0 * w).sin());
            }
        }
    }
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-6);
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| ((s / peak * 0.32 * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

static PLAYING: Mutex<Vec<u8>> = Mutex::new(Vec::new());

pub fn play(sound: Sound) {
    let wav = match sound {
        Sound::Start => synth(&[(659.25, 0.0, 0.12), (987.77, 0.07, 0.2)]),
        Sound::Done => synth(&[
            (783.99, 0.0, 0.35),
            (1046.5, 0.09, 0.45),
            (1567.98, 0.18, 0.55),
        ]),
        Sound::Error => synth(&[(311.13, 0.0, 0.2), (233.08, 0.13, 0.32)]),
    };
    let mut playing = PLAYING.lock().unwrap();
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
    *playing = wav;
}

pub fn stop_sound() {
    let mut playing = PLAYING.lock().unwrap();
    if !playing.is_empty() {
        unsafe {
            let _ = PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0));
        }
        *playing = Vec::new();
    }
}

pub fn trim() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub fn pick_folder() -> Option<std::path::PathBuf> {
    use windows::Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS, SIGDN_FILESYSPATH},
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog
            .SetOptions(dialog.GetOptions().ok()? | FOS_PICKFOLDERS)
            .ok()?;
        dialog.Show(None).ok()?;
        let name = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as _));
        path.map(Into::into)
    }
}

fn winget() -> std::path::PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(|d| Path::new(&d).join(r"Microsoft\WindowsApps\winget.exe"))
        .filter(|p| p.symlink_metadata().is_ok())
        .unwrap_or_else(|| "winget".into())
}

fn winget_error(output: &str, code: Option<i32>) -> String {
    output
        .split(['\r', '\n'])
        .map(str::trim)
        .rfind(|l| l.chars().any(char::is_alphabetic))
        .map(str::to_string)
        .unwrap_or_else(|| match code {
            Some(c) => format!("winget failed with code {c:#x}"),
            None => "winget was stopped".into(),
        })
}

pub fn winget_install(package: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(winget())
        .args([
            "install",
            "--id",
            package,
            "-e",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "winget not found. Install App Installer from the Microsoft Store.".to_string()
            } else {
                e.to_string()
            }
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(winget_error(
        &String::from_utf8_lossy(&out.stdout),
        out.status.code(),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn winget_error_takes_last_readable_line() {
        let out = "Found LibreOffice\r\n  \u{2588}\u{2588}  50%\r\nInstaller failed with exit code: 1603\r\n\r\n";
        assert_eq!(
            super::winget_error(out, Some(1)),
            "Installer failed with exit code: 1603"
        );
        assert_eq!(
            super::winget_error(" - / | ", Some(-1978335212)),
            "winget failed with code 0x8a150014"
        );
    }
}
