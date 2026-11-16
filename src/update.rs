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

#[cfg(test)]
mod tests {
    #[test]
    fn compares_versions() {
        assert!(super::newer("v0.2.0", "0.1.0"));
        assert!(super::newer("v0.10.0", "0.9.9"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
    }
}
