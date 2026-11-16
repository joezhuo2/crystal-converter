use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use windows::{
    core::HSTRING,
    Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

pub const RELEASES: &str = "https://github.com/joezhuo2/crystal-converter/releases/latest";
const API: &str = "https://api.github.com/repos/joezhuo2/crystal-converter/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    UpToDate,
    Available(String),
    Failed,
}

pub static RESULT: Mutex<Option<Status>> = Mutex::new(None);
static RESTART: Mutex<Option<PathBuf>> = Mutex::new(None);
static INSTALLING: AtomicBool = AtomicBool::new(false);

const DOWNLOAD_PREFIX: &str = "https://github.com/joezhuo2/crystal-converter/releases/download/";
const HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

struct Asset {
    url: String,
    size: u64,
    sha256: Option<String>,
}

pub fn spawn_check(delay: Duration, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        *RESULT.lock().unwrap() = Some(check());
        done();
    });
}

fn check() -> Status {
    let out = Command::new("curl.exe")
        .args([
            "-sL",
            "-m",
            "15",
            "-H",
            "User-Agent: crystal-converter",
            "-w",
            "\n%{http_code}",
            API,
        ])
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output();
    let Some(text) = out
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    else {
        return Status::Failed;
    };
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", ""));
    if code.trim() == "404" {
        return Status::UpToDate;
    }
    let tag = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["tag_name"].as_str().map(str::to_string));
    match tag {
        Some(tag) if newer(&tag, env!("CARGO_PKG_VERSION")) => {
            Status::Available(tag.trim_start_matches('v').into())
        }
        Some(_) => Status::UpToDate,
        None => Status::Failed,
    }
}

fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['.', '-'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub fn newer(tag: &str, current: &str) -> bool {
    parse(tag) > parse(current)
}

fn curl(args: &[&str], url: &str) -> Option<Output> {
    Command::new("curl.exe")
        .args(["-s", "-H", "User-Agent: crystal-converter"])
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output()
        .ok()
        .filter(|o| o.status.success())
}

fn url_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?']).next().unwrap_or("");
    HOSTS.contains(&host)
}

fn pick_asset(release: &serde_json::Value) -> Option<Asset> {
    release["assets"].as_array()?.iter().find_map(|a| {
        let url = a["browser_download_url"].as_str()?;
        if !a["name"].as_str()?.ends_with("_x64.exe") || !url.starts_with(DOWNLOAD_PREFIX) {
            return None;
        }
        Some(Asset {
            url: url.to_string(),
            size: a["size"].as_u64()?,
            sha256: a["digest"]
                .as_str()
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(str::to_lowercase),
        })
    })
}

fn is_pe(bytes: &[u8]) -> bool {
    let Some(off) = bytes
        .get(0x3c..0x40)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    else {
        return false;
    };
    bytes.starts_with(b"MZ") && bytes.get(off..off + 4) == Some(b"PE\0\0")
}

fn parse_certutil(text: &str) -> Option<String> {
    let line = text.lines().nth(1)?;
    Some(line.replace(' ', "").to_lowercase())
}

fn verify(file: &Path, asset: &Asset) -> Result<(), String> {
    let bytes = fs::read(file).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != asset.size {
        return Err("the download is incomplete".into());
    }
    if !is_pe(&bytes) {
        return Err("the download is not a Windows program".into());
    }
    if let Some(expected) = &asset.sha256 {
        let out = Command::new("certutil")
            .arg("-hashfile")
            .arg(file)
            .arg("SHA256")
            .stdin(Stdio::null())
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| e.to_string())?;
        let actual = parse_certutil(&String::from_utf8_lossy(&out.stdout));
        if actual.as_deref() != Some(expected) {
            return Err("the download's checksum does not match".into());
        }
    }
    Ok(())
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    let old = sibling(exe, ".old");
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old)?;
    if let Err(e) = fs::rename(new, exe) {
        let _ = fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

fn download(asset: &Asset, tmp: &Path) -> Result<(), String> {
    fs::File::create(tmp).map_err(|_| "the app's folder is not writable".to_string())?;
    let tmp = tmp.to_string_lossy();
    let mut url = asset.url.clone();
    for _ in 0..4 {
        let out = curl(
            &[
                "-f",
                "-m",
                "600",
                "--proto",
                "=https",
                "-o",
                &tmp,
                "-w",
                "%{redirect_url}",
            ],
            &url,
        )
        .ok_or("the download failed")?;
        let next = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if next.is_empty() {
            return Ok(());
        }
        if !url_allowed(&next) {
            return Err("the download was redirected to an untrusted address".into());
        }
        url = next;
    }
    Err("the download was redirected too many times".into())
}

fn install() -> Result<(), String> {
    let out = curl(&["-f", "-m", "15"], API).ok_or("couldn't reach GitHub")?;
    let release: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "GitHub sent an unexpected reply")?;
    let tag = release["tag_name"].as_str().unwrap_or("");
    if !newer(tag, env!("CARGO_PKG_VERSION")) {
        return Err("there is no newer version".into());
    }
    let asset = pick_asset(&release).ok_or("the release has no downloadable program")?;
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let tmp = sibling(&exe, ".new");
    let result = download(&asset, &tmp)
        .and_then(|_| verify(&tmp, &asset))
        .and_then(|_| swap(&exe, &tmp).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    *RESTART.lock().unwrap() = Some(exe);
    Ok(())
}

pub fn spawn_install(done: impl FnOnce() + Send + 'static) {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = install() {
            let _ = Command::new("explorer").arg(RELEASES).spawn();
            let text = format!(
                "Couldn't install the update: {e}.\n\nThe releases page is open so you can update by hand."
            );
            unsafe {
                MessageBoxW(
                    None,
                    &HSTRING::from(text),
                    &HSTRING::from("Crystal Converter"),
                    MB_OK | MB_ICONERROR,
                );
            }
            INSTALLING.store(false, Ordering::SeqCst);
        }
        done();
    });
}

pub fn take_restart() -> Option<PathBuf> {
    RESTART.lock().unwrap().take()
}

pub fn restart_pending() -> bool {
    RESTART.lock().unwrap().is_some()
}

pub fn cleanup() {
    if let Ok(exe) = env::current_exe() {
        let _ = fs::remove_file(sibling(&exe, ".old"));
        let _ = fs::remove_file(sibling(&exe, ".new"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("cc-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn release(url: &str, name: &str, digest: Option<&str>) -> serde_json::Value {
        serde_json::json!({"assets": [
            {"name": "crystal-converter_0.2.0_x64.zip", "browser_download_url": format!("{DOWNLOAD_PREFIX}v0.2.0/crystal-converter_0.2.0_x64.zip"), "size": 9},
            {"name": name, "browser_download_url": url, "size": 5, "digest": digest}
        ]})
    }

    #[test]
    fn picks_exe_asset() {
        let url = format!("{DOWNLOAD_PREFIX}v0.2.0/crystal-converter_0.2.0_x64.exe");
        let name = "crystal-converter_0.2.0_x64.exe";
        let a = pick_asset(&release(&url, name, Some("sha256:AB"))).unwrap();
        assert_eq!(
            (a.url, a.size, a.sha256),
            (url.clone(), 5, Some("ab".into()))
        );
        let other = "https://github.com/evil/repo/releases/download/v1/crystal-converter_1_x64.exe";
        assert!(pick_asset(&release(other, name, None)).is_none());
        assert!(pick_asset(&release(&url, "crystal-converter_0.2.0_x64.zip", None)).is_none());
    }

    #[test]
    fn allows_only_github_hosts() {
        assert!(url_allowed("https://github.com/a/b"));
        assert!(url_allowed("https://objects.githubusercontent.com/x?y=1"));
        assert!(!url_allowed("http://github.com/a"));
        assert!(!url_allowed("https://github.com.evil.com/a"));
        assert!(!url_allowed("https://evil.com/github.com/"));
        assert!(!url_allowed("https://github.com@evil.com/a"));
        assert!(!url_allowed("https://github.com:444/a"));
    }

    #[test]
    fn checks_pe_and_size() {
        let mut pe = vec![0u8; 0x48];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c] = 0x44;
        pe[0x44..0x48].copy_from_slice(b"PE\0\0");
        assert!(is_pe(&pe));
        assert!(!is_pe(b"MZ"));
        assert!(!is_pe(&[0u8; 0x48]));
        assert_eq!(
            parse_certutil("SHA256 hash of f:\nAB cd\nCertUtil: done\n").as_deref(),
            Some("abcd")
        );
        let dir = scratch("verify");
        let file = dir.join("a.exe");
        fs::write(&file, &pe).unwrap();
        let asset = |size, sha256| Asset {
            url: String::new(),
            size,
            sha256,
        };
        assert!(verify(&file, &asset(pe.len() as u64, None)).is_ok());
        assert!(verify(&file, &asset(1, None)).is_err());
        assert!(verify(&file, &asset(pe.len() as u64, Some("00".into()))).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn swaps_and_rolls_back() {
        let dir = scratch("swap");
        let exe = dir.join("app.exe");
        let new = dir.join("app.exe.new");
        fs::write(&exe, "old").unwrap();
        fs::write(&new, "new").unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(fs::read_to_string(dir.join("app.exe.old")).unwrap(), "old");
        fs::write(&exe, "cur").unwrap();
        assert!(swap(&exe, &dir.join("missing")).is_err());
        assert_eq!(fs::read_to_string(&exe).unwrap(), "cur");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn compares_versions() {
        assert!(super::newer("v0.2.0", "0.1.0"));
        assert!(super::newer("v0.10.0", "0.9.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
    }
}
