use crate::{config, convert};
use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Ghostscript,
    Whisper,
    Rembg,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Self::Ghostscript, Self::Whisper, Self::Rembg];

    pub fn name(self) -> &'static str {
        match self {
            Self::Ghostscript => "Ghostscript",
            Self::Whisper => "whisper.cpp",
            Self::Rembg => "rembg",
        }
    }

    pub fn exe(self) -> &'static str {
        match self {
            Self::Ghostscript => "gswin64c.exe",
            Self::Whisper => "whisper-cli.exe",
            Self::Rembg => "rembg.exe",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Ghostscript => "Compress PDFs",
            Self::Whisper => "Speech to subtitles (SRT)",
            Self::Rembg => "Remove image backgrounds",
        }
    }

    pub fn licence(self) -> &'static str {
        match self {
            Self::Ghostscript => "AGPL-3.0",
            Self::Whisper => "MIT",
            Self::Rembg => "MIT",
        }
    }

    pub fn page(self) -> &'static str {
        match self {
            Self::Ghostscript => "https://ghostscript.com/releases/gsdnld.html",
            Self::Whisper => "https://github.com/ggml-org/whisper.cpp/releases",
            Self::Rembg => "https://github.com/danielgatis/rembg",
        }
    }

    pub fn installer(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Ghostscript => Some((
                "winget",
                "winget install --id ArtifexSoftware.GhostScript -e",
            )),
            Self::Whisper => None,
            Self::Rembg => Some(("pip", r#"pip install "rembg[cpu,cli]""#)),
        }
    }

    fn local_dir(self) -> &'static str {
        match self {
            Self::Whisper => r"whisper\Release",
            _ => "",
        }
    }
}

pub struct Download {
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const WHISPER_ZIP: Download = Download {
    name: "whisper.cpp",
    url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip",
    size: 8_928_640,
    sha256: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
};

pub const MODELS: [(&str, Download); 4] = [
    (
        "tiny",
        Download {
            name: "Whisper tiny model",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            size: 77_691_713,
            sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        },
    ),
    (
        "base",
        Download {
            name: "Whisper base model",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            size: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        },
    ),
    (
        "small",
        Download {
            name: "Whisper small model",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            size: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        },
    ),
    (
        "medium",
        Download {
            name: "Whisper medium model",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            size: 1_533_763_059,
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        },
    ),
];

pub fn model(name: &str) -> &'static (&'static str, Download) {
    MODELS.iter().find(|m| m.0 == name).unwrap_or(&MODELS[1])
}

pub fn tools_dir() -> PathBuf {
    config::dir().join("tools")
}

pub fn model_path(name: &str) -> PathBuf {
    tools_dir()
        .join("models")
        .join(format!("ggml-{}.bin", model(name).0))
}

pub fn needs(tool: Tool) -> String {
    format!("needs {} - get it in Settings > Tools", tool.name())
}

pub fn needs_model(name: &str) -> String {
    format!(
        "needs the Whisper {} model - get it in Settings > Tools",
        model(name).0
    )
}

pub fn candidates(
    tool: Tool,
    tools: &Path,
    standard: &[PathBuf],
    path: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = vec![tools.to_path_buf(), tools.join(tool.local_dir())];
    dirs.extend_from_slice(standard);
    dirs.extend_from_slice(path);
    dirs.into_iter().map(|d| d.join(tool.exe())).collect()
}

fn standard_dirs(tool: Tool) -> Vec<PathBuf> {
    let under = |root: PathBuf, tail: &str| -> Vec<PathBuf> {
        fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join(tail))
            .collect()
    };
    let var = |k: &str| env::var_os(k).map(PathBuf::from);
    match tool {
        Tool::Ghostscript => ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(var)
            .flat_map(|p| under(p.join("gs"), "bin"))
            .collect(),
        Tool::Rembg => [
            var("APPDATA").map(|p| p.join("Python")),
            var("LOCALAPPDATA").map(|p| p.join(r"Programs\Python")),
        ]
        .into_iter()
        .flatten()
        .flat_map(|p| under(p, "Scripts"))
        .collect(),
        Tool::Whisper => Vec::new(),
    }
}

pub fn find(tool: Tool) -> Option<PathBuf> {
    candidates(
        tool,
        &tools_dir(),
        &standard_dirs(tool),
        &convert::path_dirs(),
    )
    .into_iter()
    .find(|p| p.exists())
}

pub fn gs_args(input: &Path, out: &Path, quality: u32) -> Vec<String> {
    let setting = match quality {
        1..=72 => "screen",
        0 | 73..=150 => "ebook",
        _ => "printer",
    };
    [
        "-dBATCH".to_string(),
        "-dNOPAUSE".into(),
        "-dQUIET".into(),
        "-dSAFER".into(),
        "-sDEVICE=pdfwrite".into(),
        format!("-dPDFSETTINGS=/{setting}"),
        format!("-sOutputFile={}", out.display()),
        input.display().to_string(),
    ]
    .to_vec()
}

pub fn whisper_args(model: &Path, wav: &Path, out_prefix: &Path) -> Vec<String> {
    [
        "-m",
        &model.display().to_string(),
        "-f",
        &wav.display().to_string(),
        "-osrt",
        "-l",
        "auto",
        "-of",
        &out_prefix.display().to_string(),
    ]
    .map(String::from)
    .to_vec()
}

pub fn safe_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(':')
        && !name.starts_with(['/', '\\'])
        && !name.split(['/', '\\']).any(|c| c == "..")
}

fn parse_certutil(text: &str) -> Option<String> {
    let hex: String = text
        .lines()
        .nth(1)?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    (hex.len() == 64).then(|| hex.to_lowercase())
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("certutil.exe")
        .args(["-hashfile"])
        .arg(path)
        .arg("SHA256")
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    parse_certutil(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "could not hash file".into())
}

fn fetch(dl: &Download, dest: &Path, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    let dir = dest.parent().ok_or("bad path")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = dest.with_extension("part");
    let mut child = Command::new("curl.exe")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-o",
        ])
        .arg(&part)
        .arg(dl.url)
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        progress((have as f32 / dl.size as f32).min(1.0));
        std::thread::sleep(Duration::from_millis(200));
    };
    let checked = if !status.success() {
        Err(format!("download of {} failed", dl.name))
    } else if fs::metadata(&part).map(|m| m.len()).ok() != Some(dl.size) {
        Err(format!("{} has an unexpected size", dl.name))
    } else if sha256(&part)? != dl.sha256 {
        Err(format!("{} failed its checksum", dl.name))
    } else {
        fs::rename(&part, dest).map_err(|e| e.to_string())
    };
    if checked.is_err() {
        let _ = fs::remove_file(&part);
    }
    checked
}

fn extract(zip: &Path, dest: &Path) -> Result<(), String> {
    let tar = PathBuf::from(env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
        .join(r"System32\tar.exe");
    let list = Command::new(&tar)
        .arg("-tf")
        .arg(zip)
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let names = String::from_utf8_lossy(&list.stdout);
    if !list.status.success() || !names.lines().all(safe_entry) {
        return Err("archive is not safe to extract".into());
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let ok = Command::new(&tar)
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(dest)
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        let _ = fs::remove_dir_all(dest);
        Err("could not extract archive".into())
    }
}

pub fn install_whisper(
    model_name: &str,
    progress: &mut dyn FnMut(&str, f32),
) -> Result<(), String> {
    let tools = tools_dir();
    if find(Tool::Whisper).is_none() {
        let zip = tools.join("whisper.zip");
        fetch(&WHISPER_ZIP, &zip, &mut |f| progress(WHISPER_ZIP.name, f))?;
        let result = extract(&zip, &tools.join("whisper"));
        let _ = fs::remove_file(&zip);
        result?;
    }
    let path = model_path(model_name);
    if !path.exists() {
        let dl = &model(model_name).1;
        fetch(dl, &path, &mut |f| progress(dl.name, f))?;
    }
    Ok(())
}

pub fn launch_installer(tool: Tool) {
    if let Some((_, command)) = tool.installer() {
        let _ = Command::new("cmd")
            .raw_arg(format!(
                "/c start \"Install {}\" cmd /k {command}",
                tool.name()
            ))
            .spawn();
    }
}

pub fn on_path(exe: &str) -> bool {
    convert::path_dirs()
        .into_iter()
        .any(|d| d.join(format!("{exe}.exe")).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_order_is_tools_then_standard_then_path() {
        let c = candidates(
            Tool::Whisper,
            Path::new("t"),
            &[PathBuf::from("std")],
            &[PathBuf::from("p")],
        );
        assert_eq!(c[0], Path::new("t").join("whisper-cli.exe"));
        assert_eq!(
            c[1],
            Path::new("t")
                .join(r"whisper\Release")
                .join("whisper-cli.exe")
        );
        assert_eq!(c[2], Path::new("std").join("whisper-cli.exe"));
        assert_eq!(c[3], Path::new("p").join("whisper-cli.exe"));
    }

    #[test]
    fn builds_tool_arguments() {
        let gs = gs_args(Path::new("a.pdf"), Path::new("b.pdf"), 150);
        assert!(gs.contains(&"-dPDFSETTINGS=/ebook".to_string()));
        assert_eq!(gs.last().unwrap(), "a.pdf");
        assert!(
            gs_args(Path::new("a"), Path::new("b"), 72).contains(&"-dPDFSETTINGS=/screen".into())
        );
        assert!(
            gs_args(Path::new("a"), Path::new("b"), 300).contains(&"-dPDFSETTINGS=/printer".into())
        );
        let w = whisper_args(Path::new("m.bin"), Path::new("a.wav"), Path::new("out"));
        assert_eq!(w[..4], ["-m", "m.bin", "-f", "a.wav"]);
        assert!(w.ends_with(&["-of".to_string(), "out".to_string()]));
    }

    #[test]
    fn download_table_is_https_and_pinned() {
        let all = MODELS.iter().map(|m| &m.1).chain([&WHISPER_ZIP]);
        for d in all {
            assert!(
                d.url.starts_with("https://github.com/")
                    || d.url.starts_with("https://huggingface.co/")
            );
            assert_eq!(d.sha256.len(), 64);
            assert!(d.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(d.size > 0);
        }
        assert_eq!(model("nope").0, "base");
    }

    #[test]
    fn rejects_escaping_zip_entries() {
        assert!(safe_entry("Release/whisper-cli.exe"));
        assert!(!safe_entry("../evil.exe"));
        assert!(!safe_entry("a/../../evil.exe"));
        assert!(!safe_entry("a\\..\\evil.exe"));
        assert!(!safe_entry("/abs.exe"));
        assert!(!safe_entry("C:/abs.exe"));
    }

    #[test]
    fn parses_certutil_output() {
        let h = "ab".repeat(32);
        let text = format!(
            "SHA256 hash of f:\r\n{h}\r\nCertUtil: -hashfile command completed successfully.\r\n"
        );
        assert_eq!(parse_certutil(&text), Some(h));
        assert_eq!(parse_certutil("nope"), None);
    }

    #[test]
    fn missing_tool_message_points_to_settings() {
        assert_eq!(
            needs(Tool::Rembg),
            "needs rembg - get it in Settings > Tools"
        );
    }
}
