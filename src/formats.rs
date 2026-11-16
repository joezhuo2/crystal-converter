use crate::config::{Category, Preset, WheelKind};
use std::{fs::File, io::Read, path::Path};

pub const MAX_SLICES: usize = 10;

const IMAGE: &[&str] = &["png", "jpg", "webp", "avif", "gif", "bmp", "tiff", "ico", "heic", "heif", "svg", "tga"];
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "3gp"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "wma", "aiff", "aif"];
const DOCUMENT: &[&str] = &["doc", "docx", "odt", "rtf", "txt", "html", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp"];
const OFFICE_PDF_ONLY: &[&str] = &["xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp"];

pub fn ext_of(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpeg" | "jfif" | "jpe" => "jpg".into(),
        "tif" => "tiff".into(),
        "htm" => "html".into(),
        _ => ext,
    }
}

pub fn detect(path: &Path) -> Option<Category> {
    let mut head = [0u8; 32];
    let n = File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    sniff(&head[..n]).or_else(|| by_ext(&ext_of(path)))
}

fn by_ext(ext: &str) -> Option<Category> {
    [(IMAGE, Category::Image), (VIDEO, Category::Video), (AUDIO, Category::Audio), (DOCUMENT, Category::Document)]
        .into_iter()
        .find(|(list, _)| list.contains(&ext))
        .map(|(_, c)| c)
}

fn sniff(h: &[u8]) -> Option<Category> {
    use Category::*;
    let at = |o: usize, s: &[u8]| h.get(o..o + s.len()) == Some(s);
    if at(0, b"\x89PNG") || at(0, b"\xFF\xD8\xFF") || at(0, b"GIF8") || at(0, b"II*\0") || at(0, b"MM\0*") {
        return Some(Image);
    }
    if at(0, b"RIFF") {
        return [(b"WEBP", Image), (b"WAVE", Audio), (b"AVI ", Video)]
            .into_iter()
            .find(|(tag, _)| at(8, *tag))
            .map(|(_, c)| c);
    }
    if at(4, b"ftyp") {
        return Some(match h.get(8..12) {
            Some(b"heic" | b"heix" | b"mif1" | b"msf1" | b"avif" | b"avis") => Image,
            Some(b"M4A " | b"M4B ") => Audio,
            _ => Video,
        });
    }
    if at(0, b"\x1A\x45\xDF\xA3") || at(0, b"FLV") || at(0, &[0x30, 0x26, 0xB2, 0x75]) {
        return Some(Video);
    }
    if at(0, b"OggS") || at(0, b"fLaC") || at(0, b"ID3") || (h.len() > 1 && h[0] == 0xFF && h[1] & 0xE0 == 0xE0) {
        return Some(Audio);
    }
    None
}

pub fn targets(category: Category, ext: &str, office: bool) -> Vec<&'static str> {
    let list: &[&'static str] = match category {
        Category::Image if ext == "gif" => &["mp4", "webm", "webp", "png", "jpg"],
        Category::Image => &["png", "jpg", "webp", "avif", "gif", "ico", "bmp", "pdf"],
        Category::Video => &["mp4", "webm", "mov", "mkv", "avi", "gif", "mp3", "m4a"],
        Category::Audio => &["mp3", "wav", "flac", "opus", "ogg", "m4a"],
        Category::Document if !office => &[],
        Category::Document if OFFICE_PDF_ONLY.contains(&ext) => &["pdf"],
        Category::Document => &["pdf", "docx", "odt", "txt", "html"],
        Category::Any => &[],
    };
    list.iter().copied().filter(|t| *t != ext).collect()
}

fn modify_builtins(category: Category) -> Vec<Preset> {
    let p = |name: &str| Preset::named(name, category);
    match category {
        Category::Image => vec![
            Preset { quality: 75, ..p("Quality 75") },
            Preset { scale_pct: 50, ..p("50%") },
            Preset { max_dim: 1920, ..p("1920 px") },
            Preset { max_dim: 1080, ..p("1080 px") },
            Preset { strip_metadata: true, ..p("No metadata") },
            Preset { rotate: 90, ..p("Rotate") },
        ],
        Category::Video => vec![
            Preset { max_size_mb: 8.0, ..p("8 MB") },
            Preset { max_size_mb: 25.0, ..p("25 MB") },
            Preset { max_size_mb: 50.0, ..p("50 MB") },
            Preset { max_dim: 1920, ..p("1080p") },
            Preset { max_dim: 1280, ..p("720p") },
            Preset { fps: 30, ..p("30 fps") },
            Preset { mute: true, ..p("Mute") },
            Preset { rotate: 90, ..p("Rotate") },
        ],
        Category::Audio => vec![
            Preset { volume_db: 6.0, ..p("+6 dB") },
            Preset { volume_db: -6.0, ..p("-6 dB") },
            Preset { normalize: true, ..p("Normalize") },
            Preset { trim_silence: true, ..p("Trim silence") },
            Preset { mono: true, ..p("Mono") },
            Preset { bitrate_kbps: 128, ..p("128 kbps") },
        ],
        _ => vec![],
    }
}

pub fn slices(kind: WheelKind, category: Category, ext: &str, office: bool, presets: &[Preset]) -> Vec<Preset> {
    let mut out: Vec<Preset> = match kind {
        WheelKind::Convert => targets(category, ext, office).into_iter().map(Preset::convert_to).collect(),
        WheelKind::Modify => modify_builtins(category),
    };
    out.extend(presets.iter().filter(|p| p.wheel == kind && p.matches(category)).cloned());
    out.truncate(MAX_SLICES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_magic_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n"), Some(Category::Image));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some(Category::Audio));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom"), Some(Category::Video));
        assert_eq!(sniff(b"\0\0\0\x20ftypheic"), Some(Category::Image));
        assert_eq!(sniff(b"hello"), None);
        assert_eq!(by_ext("docx"), Some(Category::Document));
    }

    #[test]
    fn convert_targets_skip_input_format() {
        assert!(!targets(Category::Image, "png", false).contains(&"png"));
        assert!(targets(Category::Document, "docx", false).is_empty());
        assert_eq!(targets(Category::Document, "xlsx", true), vec!["pdf"]);
    }

    #[test]
    fn slices_include_matching_presets_only() {
        let presets = crate::config::default_presets();
        let video = slices(WheelKind::Modify, Category::Video, "mp4", false, &presets);
        assert!(video.iter().any(|p| p.name == "Discord 10 MB"));
        let audio = slices(WheelKind::Modify, Category::Audio, "mp3", false, &presets);
        assert!(!audio.iter().any(|p| p.name == "Discord 10 MB"));
        assert!(video.len() <= MAX_SLICES);
    }
}
