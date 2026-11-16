use std::path::Path;
use windows::{
    core::HSTRING,
    Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat},
    Media::Ocr::OcrEngine,
    Storage::{FileAccessMode, StorageFile},
    Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
};

const NO_LANGUAGE: &str = "No OCR language is installed. Add a language in Windows Settings > Time & language > Language & region, then try again.";

pub fn recognize(png: &Path) -> Result<String, String> {
    let w = |e: windows::core::Error| format!("OCR: {}", e.message());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| NO_LANGUAGE)?;
    let path = std::path::absolute(png).map_err(|e| e.to_string())?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
        .and_then(|op| op.join())
        .map_err(w)?;
    let stream = file
        .OpenAsync(FileAccessMode::Read)
        .and_then(|op| op.join())
        .map_err(w)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .and_then(|op| op.join())
        .map_err(w)?;
    let bitmap = decoder
        .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)
        .and_then(|op| op.join())
        .map_err(w)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|op| op.join())
        .map_err(w)?;
    let mut lines = Vec::new();
    for line in &result.Lines().map_err(w)? {
        lines.push(line.Text().map_err(w)?.to_string());
    }
    Ok(lines.join("\n"))
}
