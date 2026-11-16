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
    pub trim_start: f32,
    pub trim_end: f32,
    pub op: String,
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
            trim_start: 0.0,
            trim_end: 0.0,
            op: String::new(),
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
}
