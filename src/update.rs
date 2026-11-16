use std::{
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    sync::Mutex,
    time::Duration,
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

pub fn spawn_check(delay: Duration, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        *RESULT.lock().unwrap() = Some(check());
        done();
    });
}

fn check() -> Status {
    let out = Command::new("curl.exe")
        .args(["-sfL", "-m", "15", "-H", "User-Agent: crystal-converter", API])
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output();
    let tag = out
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["tag_name"].as_str().map(str::to_string));
    match tag {
        Some(tag) if newer(&tag, env!("CARGO_PKG_VERSION")) => Status::Available(tag.trim_start_matches('v').into()),
        Some(_) => Status::UpToDate,
        None => Status::Failed,
    }
}

fn parse(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn newer(tag: &str, current: &str) -> bool {
    parse(tag) > parse(current)
}

#[cfg(test)]
mod tests {
    #[test]
    fn compares_versions() {
        assert!(super::newer("v0.2.0", "0.1.0"));
        assert!(super::newer("v0.10.0", "0.9.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
    }
}
