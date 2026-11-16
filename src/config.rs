use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Image,
    Video,
    Audio,
    Document,
    Any,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Self::Any,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Document,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Document => "Document",
            Self::Any => "Any",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum WheelKind {
    Convert,
    Modify,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    NextToOriginal,
    Folder,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub wheel: WheelKind,
    pub category: Category,
    pub format: String,
    pub max_size_mb: f32,
    pub max_dim: u32,
    pub scale_pct: u32,
    pub quality: u32,
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub rotate: u32,
    pub volume_db: f32,
    pub normalize: bool,
    pub trim_silence: bool,
    pub mono: bool,
    pub mute: bool,
    pub strip_metadata: bool,
    pub key: u32,
    pub args: String,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: "New preset".into(),
            wheel: WheelKind::Modify,
            category: Category::Any,
            format: String::new(),
            max_size_mb: 0.0,
            max_dim: 0,
            scale_pct: 0,
            quality: 0,
            bitrate_kbps: 0,
            fps: 0,
            rotate: 0,
            volume_db: 0.0,
            normalize: false,
            trim_silence: false,
            mono: false,
            mute: false,
            strip_metadata: false,
            key: 0,
            args: String::new(),
        }
    }
}

impl Preset {
    pub fn named(name: &str, category: Category) -> Self {
        Self {
            name: name.into(),
            category,
            ..Default::default()
        }
    }

    pub fn convert_to(format: &str) -> Self {
        Self {
            name: format.to_uppercase(),
            wheel: WheelKind::Convert,
            format: format.into(),
            ..Default::default()
        }
    }

    pub fn matches(&self, category: Category) -> bool {
        self.category == Category::Any || self.category == category
    }
}

pub fn split_args(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in ffmpeg arguments".into());
    }
    if started {
        out.push(cur);
    }
    if out.iter().any(|a| a == "-i") {
        return Err("ffmpeg arguments can't add inputs (-i)".into());
    }
    Ok(out)
}

pub fn export_presets(presets: &[Preset]) -> String {
    serde_json::to_string_pretty(presets).unwrap_or_default()
}

pub fn import_presets(text: &str) -> Result<Vec<Preset>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("Not valid preset JSON: {e}"))?;
    let items = value.as_array().ok_or("Expected a JSON list of presets")?;
    if items.is_empty() || items.len() > 50 {
        return Err("Expected between 1 and 50 presets".into());
    }
    let known = match serde_json::to_value(Preset::default()) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => return Err("Internal error".into()),
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        let obj = item
            .as_object()
            .ok_or_else(|| format!("Preset {n}: expected an object"))?;
        if let Some(k) = obj.keys().find(|k| !known.contains_key(*k)) {
            return Err(format!("Preset {n}: unknown field \"{k}\""));
        }
        let p: Preset =
            serde_json::from_value(item.clone()).map_err(|e| format!("Preset {n}: {e}"))?;
        if p.name.trim().is_empty() || p.name.chars().count() > 64 {
            return Err(format!("Preset {n}: name must be 1 to 64 characters"));
        }
        if p.format.len() > 8 || !p.format.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("Preset {n}: invalid format \"{}\"", p.format));
        }
        if p.key > 0xFE {
            return Err(format!("Preset {n}: invalid key"));
        }
        if p.args.len() > 1000 {
            return Err(format!("Preset {n}: ffmpeg arguments too long"));
        }
        split_args(&p.args).map_err(|e| format!("Preset {n}: {e}"))?;
        out.push(p);
    }
    Ok(out)
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub convert_key: u32,
    pub modify_key: u32,
    pub close_delay_ms: u32,
    pub sounds: bool,
    pub copy_to_clipboard: bool,
    pub output_mode: OutputMode,
    pub output_folder: String,
    pub hardware_encoding: bool,
    pub launch_at_login: bool,
    pub always_on_top: bool,
    pub context_menu: bool,
    pub check_updates: bool,
    pub presets: Vec<Preset>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            convert_key: 0xC0,
            modify_key: 0x14,
            close_delay_ms: 1500,
            sounds: true,
            copy_to_clipboard: false,
            output_mode: OutputMode::NextToOriginal,
            output_folder: String::new(),
            hardware_encoding: true,
            launch_at_login: false,
            always_on_top: true,
            context_menu: false,
            check_updates: true,
            presets: default_presets(),
        }
    }
}

pub fn default_presets() -> Vec<Preset> {
    vec![
        Preset {
            format: "mp4".into(),
            max_size_mb: 10.0,
            ..Preset::named("Discord 10 MB", Category::Video)
        },
        Preset {
            wheel: WheelKind::Convert,
            format: "webp".into(),
            max_dim: 1920,
            quality: 80,
            ..Preset::named("Web", Category::Image)
        },
    ]
}

pub fn dir() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("crystal-converter")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

impl Config {
    pub fn exists() -> bool {
        path().exists()
    }

    pub fn load() -> Self {
        let Ok(text) = fs::read_to_string(path()) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|_| {
            let _ = fs::rename(path(), dir().join("config.json.bak"));
            Self::default()
        })
    }

    pub fn save(&self) -> io::Result<()> {
        fs::create_dir_all(dir())?;
        let tmp = dir().join("config.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(tmp, path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_fills_defaults() {
        let c: Config =
            serde_json::from_str(r#"{"sounds":false,"presets":[{"name":"x"}]}"#).unwrap();
        assert!(!c.sounds);
        assert_eq!(c.close_delay_ms, 1500);
        assert_eq!(c.presets[0].wheel, WheelKind::Modify);
    }

    #[test]
    fn splits_args_with_quotes() {
        assert_eq!(
            split_args(r#"-vf "scale=1:2, fps=5" -metadata 'a b' ""  x"#).unwrap(),
            vec!["-vf", "scale=1:2, fps=5", "-metadata", "a b", "", "x"]
        );
        assert!(split_args("-vf 'oops").is_err());
        assert!(split_args("-i other.mp4").is_err());
        assert!(split_args("   ").unwrap().is_empty());
    }

    #[test]
    fn presets_round_trip_and_validate() {
        let presets = vec![Preset {
            key: 0x31,
            args: "-preset slow".into(),
            ..default_presets().remove(0)
        }];
        assert_eq!(import_presets(&export_presets(&presets)).unwrap(), presets);
        assert!(import_presets(r#"[{"name":"x","exe":"cmd.exe"}]"#)
            .unwrap_err()
            .contains("unknown field"));
        assert!(import_presets(r#"[{"name":"x","fps":"fast"}]"#).is_err());
        assert!(import_presets(r#"[{"name":"x","format":"../x"}]"#).is_err());
        assert!(import_presets(r#"[{"name":"x","args":"'"}]"#).is_err());
        assert!(import_presets(r#"{"name":"x"}"#).is_err());
        assert!(import_presets("[]").is_err());
    }
}
