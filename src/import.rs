use crate::{config::Category, formats};
use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, MAX_PATH},
        System::{
            Com::{
                CoCreateInstance, CoTaskMemFree, IDataObject, IServiceProvider, CLSCTX_ALL,
                DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL, TYMED_ISTREAM,
            },
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{OleGetClipboard, ReleaseStgMedium, CF_DIB},
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4},
        },
        UI::{
            Shell::{
                FOLDERID_Pictures, IFolderView, IShellBrowser, IShellItemArray, IShellWindows,
                SHGetKnownFolderPath, SID_STopLevelBrowser, ShellWindows, CFSTR_FILECONTENTS,
                CFSTR_FILEDESCRIPTORW, CFSTR_INETURLW, FILEDESCRIPTORW, KF_FLAG_DEFAULT,
                SIGDN_FILESYSPATH, SVGIO_SELECTION, SWC_DESKTOP, SWFO_NEEDDISPATCH,
            },
            WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, IsWindowVisible},
        },
    },
};

const MAX_BYTES: usize = 100 * 1024 * 1024;
const NO_WINDOW: u32 = 0x0800_0000;
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub fn staging_dir() -> PathBuf {
    env::temp_dir().join("crystal-converter-import")
}

pub fn clear_staging() {
    let _ = fs::remove_dir_all(staging_dir());
}

pub fn is_staged(path: &Path) -> bool {
    path.starts_with(staging_dir())
}

pub fn pictures_dir() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out: String = trimmed.chars().take(100).collect();
    let stem = out.split('.').next().unwrap_or("").to_ascii_lowercase();
    if out.is_empty() {
        out = "download".into();
    } else if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
}

pub fn http_url(text: &str) -> Option<&str> {
    let url = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let lower = url.to_ascii_lowercase();
    let ok = (lower.starts_with("http://") || lower.starts_with("https://"))
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace());
    ok.then_some(url)
}

fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let after_scheme = path.split_once("://").map_or(path, |(_, r)| r);
    let tail = after_scheme.split_once('/').map_or("", |(_, p)| p);
    safe_name(tail)
}

fn staged_path(name: &str) -> Option<PathBuf> {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = staging_dir().join(N.fetch_add(1, Ordering::Relaxed).to_string());
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join(safe_name(name)))
}

fn keep(path: PathBuf, allowed: impl Fn(Category) -> bool) -> Option<PathBuf> {
    if formats::detect(&path).is_some_and(allowed) {
        Some(path)
    } else {
        let _ = fs::remove_file(&path);
        None
    }
}

fn media(c: Category) -> bool {
    c != Category::Document
}

fn variant_i32(n: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                Anonymous: VARIANT_0_0_0 { lVal: n },
                ..Default::default()
            }),
        },
    }
}

fn shell_view(dispatch: &windows::Win32::System::Com::IDispatch) -> Option<(HWND, IFolderView)> {
    unsafe {
        let sp: IServiceProvider = windows_core::Interface::cast(dispatch).ok()?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser).ok()?;
        let frame = browser.GetWindow().ok()?;
        let view = browser.QueryActiveShellView().ok()?;
        if !IsWindowVisible(view.GetWindow().ok()?).as_bool() {
            return None;
        }
        Some((frame, windows_core::Interface::cast(&view).ok()?))
    }
}

fn view_paths(view: &IFolderView) -> Vec<PathBuf> {
    unsafe {
        let Ok(items) = view.Items::<IShellItemArray>(SVGIO_SELECTION) else {
            return Vec::new();
        };
        let count = items.GetCount().unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let name = items
                    .GetItemAt(i)
                    .ok()?
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .ok()?;
                let path = name.to_string().ok().map(PathBuf::from);
                CoTaskMemFree(Some(name.0 as _));
                path
            })
            .collect()
    }
}

pub fn explorer_selection() -> Vec<PathBuf> {
    unsafe {
        let fg = GetForegroundWindow();
        let Ok(windows) = CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL)
        else {
            return Vec::new();
        };
        let count = windows.Count().unwrap_or(0);
        for i in 0..count {
            let Ok(d) = windows.Item(&variant_i32(i)) else {
                continue;
            };
            if let Some((frame, view)) = shell_view(&d) {
                if frame == fg {
                    return view_paths(&view);
                }
            }
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class) as usize;
        if matches!(
            String::from_utf16_lossy(&class[..n]).as_str(),
            "Progman" | "WorkerW"
        ) {
            let mut hwnd = 0;
            if let Ok(d) = windows.FindWindowSW(
                &variant_i32(0),
                &VARIANT::default(),
                SWC_DESKTOP,
                &mut hwnd,
                SWFO_NEEDDISPATCH,
            ) {
                if let Some((_, view)) = shell_view(&d) {
                    return view_paths(&view);
                }
            }
        }
        Vec::new()
    }
}

fn format_of(cf: u32, index: i32, tymed: i32) -> FORMATETC {
    FORMATETC {
        cfFormat: cf as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index,
        tymed: tymed as u32,
    }
}

fn read_medium(data: &IDataObject, cf: u32, index: i32) -> Option<Vec<u8>> {
    unsafe {
        let format = format_of(cf, index, TYMED_HGLOBAL.0 | TYMED_ISTREAM.0);
        let mut medium = data.GetData(&format).ok()?;
        let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            let h = medium.u.hGlobal;
            let size = GlobalSize(h);
            let ptr = GlobalLock(h) as *const u8;
            let out = (!ptr.is_null() && size <= MAX_BYTES)
                .then(|| std::slice::from_raw_parts(ptr, size).to_vec());
            if !ptr.is_null() {
                let _ = GlobalUnlock(h);
            }
            out
        } else if medium.tymed == TYMED_ISTREAM.0 as u32 {
            medium.u.pstm.as_ref().and_then(|stream| {
                let mut out = Vec::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let mut read = 0u32;
                    if stream
                        .Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read))
                        .is_err()
                        || out.len() + read as usize > MAX_BYTES
                    {
                        return None;
                    }
                    if read == 0 {
                        return Some(out);
                    }
                    out.extend_from_slice(&buf[..read as usize]);
                }
            })
        } else {
            None
        };
        ReleaseStgMedium(&mut medium);
        bytes
    }
}

fn registered(name: windows::core::PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

fn descriptors(data: &IDataObject) -> Vec<(String, bool)> {
    let Some(bytes) = read_medium(data, registered(CFSTR_FILEDESCRIPTORW), -1) else {
        return Vec::new();
    };
    let Some(count) = bytes
        .get(0..4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    else {
        return Vec::new();
    };
    let size = std::mem::size_of::<FILEDESCRIPTORW>();
    (0..count as usize)
        .filter_map(|i| {
            let start = 4 + i * size;
            let chunk = bytes.get(start..start + size)?;
            let fd = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<FILEDESCRIPTORW>()) };
            let name = fd.cFileName;
            let len = name
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(MAX_PATH as usize);
            let is_dir = fd.dwFileAttributes & 0x10 != 0;
            Some((String::from_utf16_lossy(&name[..len]), is_dir))
        })
        .collect()
}

fn url_of(data: &IDataObject) -> Option<String> {
    let bytes = read_medium(data, registered(CFSTR_INETURLW), -1)?;
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|c| *c != 0)
        .collect();
    http_url(&String::from_utf16_lossy(&wide)).map(str::to_string)
}

fn download(url: &str) -> Option<PathBuf> {
    let path = staged_path(&name_from_url(url))?;
    let ok = Command::new("curl.exe")
        .args([
            "-sSL",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "-m",
            "30",
            "--max-filesize",
            &MAX_BYTES.to_string(),
            "-o",
        ])
        .arg(&path)
        .arg(url)
        .stdin(Stdio::null())
        .creation_flags(NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success());
    let size = fs::metadata(&path).map_or(usize::MAX, |m| m.len() as usize);
    if ok && size <= MAX_BYTES {
        keep(path, |c| c != Category::Document)
    } else {
        let _ = fs::remove_file(&path);
        None
    }
}

pub fn from_browser(data: &IDataObject) -> Vec<PathBuf> {
    let contents = registered(CFSTR_FILECONTENTS);
    let files: Vec<PathBuf> = descriptors(data)
        .into_iter()
        .enumerate()
        .filter(|(_, (_, dir))| !dir)
        .filter_map(|(i, (name, _))| {
            let bytes = read_medium(data, contents, i as i32)?;
            let path = staged_path(&name)?;
            fs::write(&path, bytes).ok()?;
            keep(path, media)
        })
        .collect();
    if !files.is_empty() {
        return files;
    }
    url_of(data)
        .and_then(|u| download(&u))
        .into_iter()
        .collect()
}

pub fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u32_at = |o: usize| {
        dib.get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let header = u32_at(0)? as usize;
    let (width, height) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let bpp = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?) as usize;
    let compression = u32_at(16)?;
    if header < 40
        || !(1..=32768).contains(&width)
        || height == 0
        || height.unsigned_abs() > 32768
        || !matches!(bpp, 24 | 32)
        || !matches!(compression, 0 | 3)
    {
        return None;
    }
    let offset = header
        + if compression == 3 && header == 40 {
            12
        } else {
            0
        };
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let stride = (w * bpp).div_ceil(32) * 4;
    let pixels = dib.get(offset..offset + stride * h)?;
    let step = bpp / 8;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = if height > 0 { h - 1 - y } else { y };
        for px in pixels[row * stride..][..w * step].chunks_exact(step) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Some((w as u32, h as u32, out))
}

fn clipboard_image(data: &IDataObject) -> Option<PathBuf> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let path = staged_path(&format!("clipboard-{secs}.png"))?;
    if let Some(png) = read_medium(data, registered(w!("PNG")), -1) {
        fs::write(&path, png).ok()?;
    } else {
        let dib = read_medium(data, u32::from(CF_DIB.0), -1)?;
        let (w, h, rgba) = dib_to_rgba(&dib)?;
        tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)?
            .save_png(&path)
            .ok()?;
    }
    keep(path, |c| c == Category::Image)
}

pub fn clipboard() -> Vec<PathBuf> {
    let Ok(data) = (unsafe { OleGetClipboard() }) else {
        return Vec::new();
    };
    let files = crate::wheel::read_files(&data);
    if !files.is_empty() {
        return files;
    }
    clipboard_image(&data).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_names() {
        assert_eq!(safe_name("../../evil.png"), "evil.png");
        assert_eq!(safe_name(r"C:\Windows\x.jpg"), "x.jpg");
        assert_eq!(safe_name("a<b>:c?.png"), "a_b__c_.png");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name("  . "), "download");
        assert_eq!(safe_name("CON.png"), "_CON.png");
        assert_eq!(safe_name("photo. "), "photo");
        assert!(safe_name(&"x".repeat(500)).len() <= 100);
    }

    #[test]
    fn filters_urls() {
        assert_eq!(
            http_url("https://a.com/x.png\0"),
            Some("https://a.com/x.png")
        );
        assert_eq!(http_url("HTTP://a.com/x"), Some("HTTP://a.com/x"));
        assert_eq!(http_url("file:///c:/x.png"), None);
        assert_eq!(http_url("javascript:alert(1)"), None);
        assert_eq!(http_url("https://a.com/a b"), None);
        assert_eq!(http_url("ftp://a.com/x"), None);
    }

    #[test]
    fn names_from_urls() {
        assert_eq!(name_from_url("https://a.com/p/cat.png?x=1#y"), "cat.png");
        assert_eq!(name_from_url("https://a.com"), "download");
        assert_eq!(name_from_url("https://a.com/p/%2e%2e"), "%2e%2e");
    }

    #[test]
    fn decodes_bottom_up_dib() {
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 2;
        dib[8] = 2;
        dib[14] = 24;
        dib.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]);
        dib.extend_from_slice(&[7, 8, 9, 10, 11, 12, 0, 0]);
        let (w, h, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgba[..4], &[9, 8, 7, 255]);
        assert_eq!(&rgba[8..12], &[3, 2, 1, 255]);
        assert!(dib_to_rgba(&dib[..50]).is_none());
    }
}
