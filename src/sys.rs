use crate::config::Config;
use std::{env, path::Path};
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::System::Registry::{RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ},
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
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE));
        }
        if config.context_menu {
            reg_set(MENU_KEY, None, "Convert with Crystal Converter");
            reg_set(MENU_KEY, Some("Icon"), &exe.display().to_string());
            reg_set(&format!(r"{MENU_KEY}\command"), None, &format!("{quoted} --open \"%1\""));
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
    let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

pub fn recycle(paths: &[std::path::PathBuf]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
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
