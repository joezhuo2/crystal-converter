use crate::convert::{command, file_name};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn tar() -> PathBuf {
    env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join(r"System32\tar.exe"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| "tar".into())
}

fn run(mut cmd: Command) -> Result<String, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("tar.exe could not start: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Err(String::from_utf8_lossy(&out.stderr)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("tar failed")
        .to_string())
}

pub fn zip(files: &[PathBuf], out: &Path) -> Result<(), String> {
    let mut cmd = command(tar());
    cmd.args(["-a", "-c", "-f"]).arg(out);
    for f in files {
        let f = std::path::absolute(f).map_err(|e| e.to_string())?;
        if let Some(dir) = f.parent() {
            cmd.arg("-C").arg(dir).arg(file_name(&f));
        }
    }
    run(cmd).map(|_| ())
}

pub fn is_safe_entry(name: &str) -> bool {
    let n = name.replace('\\', "/");
    !n.starts_with('/') && !n.contains(':') && !n.split('/').any(|part| part == "..")
}

pub fn unique_dir(dir: &Path, stem: &str) -> PathBuf {
    let mut candidate = dir.join(stem);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n})"));
        n += 1;
    }
    candidate
}

pub fn extract(input: &Path, out: &Path) -> Result<(), String> {
    let mut list = command(tar());
    list.arg("-tf").arg(input);
    let names = run(list)?;
    if let Some(bad) = names.lines().find(|l| !is_safe_entry(l)) {
        return Err(format!("archive has an unsafe path: {bad}"));
    }
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut cmd = command(tar());
    cmd.arg("-xf").arg(input).arg("-C").arg(out);
    let result = run(cmd).map(|_| ());
    if result.is_err() {
        let _ = fs::remove_dir_all(out);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_that_escape() {
        assert!(is_safe_entry("a/b.txt"));
        assert!(is_safe_entry("dir/"));
        assert!(!is_safe_entry("../x"));
        assert!(!is_safe_entry("a/../../x"));
        assert!(!is_safe_entry("a\\..\\x"));
        assert!(!is_safe_entry("/etc/x"));
        assert!(!is_safe_entry("C:/x"));
        assert!(!is_safe_entry("C:\\x"));
    }

    #[test]
    fn zips_and_extracts_round_trip() {
        let dir = env::temp_dir().join(format!("cc-zip-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "one").unwrap();
        fs::write(dir.join("sub").join("b.txt"), "two").unwrap();
        let zip_path = dir.join("out.zip");
        zip(&[dir.join("a.txt"), dir.join("sub")], &zip_path).unwrap();
        let out = unique_dir(&dir, "out");
        extract(&zip_path, &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "one");
        assert_eq!(
            fs::read_to_string(out.join("sub").join("b.txt")).unwrap(),
            "two"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
