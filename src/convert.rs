use crate::config::{Category, Preset};
use crate::formats::{detect, ext_of};
use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        OnceLock,
    },
};

const NO_WINDOW: u32 = 0x0800_0000;
const WHITE_BG: &str = "format=rgba,split[fg][bg];[bg]lutrgb=r=255:g=255:b=255:a=255[w];[w][fg]overlay=format=auto,format=rgb24";
const MIB_KBIT: f64 = 8192.0;

pub enum Event {
    Progress {
        index: usize,
        total: usize,
        fraction: Option<f32>,
        name: String,
    },
    Done {
        outputs: Vec<PathBuf>,
        failed: Vec<String>,
    },
    Failed(String),
    Cancelled,
}

pub struct Job {
    pub files: Vec<PathBuf>,
    pub preset: Preset,
    pub out_dir: Option<PathBuf>,
    pub hardware: bool,
}

#[derive(Default, Debug, PartialEq)]
pub struct Info {
    pub duration: Option<f64>,
    pub has_audio: bool,
    pub has_video: bool,
}

pub fn run(job: Job, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    let merge_pdf =
        job.preset.format == "pdf" && job.files.iter().all(|f| detect(f) == Some(Category::Image));
    let groups: Vec<Vec<PathBuf>> = if merge_pdf {
        vec![job.files.clone()]
    } else {
        job.files.iter().map(|f| vec![f.clone()]).collect()
    };
    let total = groups.len();
    let mut outputs = Vec::new();
    let mut failed = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let name = file_name(&group[0]);
        emit(Event::Progress {
            index,
            total,
            fraction: None,
            name: name.clone(),
        });
        let result = convert(group, &job, cancel, &mut |f| {
            emit(Event::Progress {
                index,
                total,
                fraction: Some(f),
                name: name.clone(),
            })
        });
        if cancel.load(Ordering::Relaxed) {
            emit(Event::Cancelled);
            return;
        }
        match result {
            Ok(p) => outputs.extend(p),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    emit(if outputs.is_empty() {
        Event::Failed(failed.join("\n"))
    } else {
        Event::Done { outputs, failed }
    });
}

fn convert(
    group: &[PathBuf],
    job: &Job,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<PathBuf>, String> {
    let input = &group[0];
    let category = detect(input).ok_or("unsupported file type")?;
    let in_ext = ext_of(input);
    let p = &job.preset;
    let fmt = if p.format.is_empty() {
        in_ext.clone()
    } else {
        p.format.clone()
    };
    let suffix = (fmt == in_ext).then(|| slug(&p.name));
    let dir = match &job.out_dir {
        Some(d) => d.clone(),
        None => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if category == Category::Document && in_ext == "pdf" && matches!(fmt.as_str(), "png" | "jpg") {
        return pdf_pages(input, &dir, &stem(input), &fmt, cancel, progress);
    }
    let out = unique_path(&dir, &stem(input), suffix.as_deref(), &fmt);
    let result = match category {
        Category::Document => office(input, &fmt, &out),
        Category::Image if fmt == "pdf" => images_to_pdf(group, &out, p, cancel, progress),
        _ => ffmpeg_convert(
            input,
            &out,
            &fmt,
            category,
            p,
            job.hardware,
            cancel,
            progress,
        ),
    };
    if result.is_err() {
        let _ = fs::remove_file(&out);
    }
    result.map(|_| vec![out])
}

fn pdf_pages(
    input: &Path,
    dir: &Path,
    stem: &str,
    fmt: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<PathBuf>, String> {
    use windows::{
        core::HSTRING,
        Data::Pdf::{PdfDocument, PdfPageRenderOptions},
        Graphics::Imaging::BitmapEncoder,
        Storage::{
            StorageFile,
            Streams::{DataReader, InMemoryRandomAccessStream},
        },
        Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
    };
    let w = |e: windows::core::Error| format!("PDF: {}", e.message());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let path = std::path::absolute(input).map_err(|e| e.to_string())?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
        .and_then(|op| op.join())
        .map_err(w)?;
    let doc = PdfDocument::LoadFromFileAsync(&file)
        .and_then(|op| op.join())
        .map_err(w)?;
    let count = doc.PageCount().map_err(w)?;
    let encoder = if fmt == "jpg" {
        BitmapEncoder::JpegEncoderId()
    } else {
        BitmapEncoder::PngEncoderId()
    }
    .map_err(w)?;
    let mut outputs = Vec::new();
    for i in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let page = doc.GetPage(i).map_err(w)?;
        let size = page.Size().map_err(w)?;
        let options = PdfPageRenderOptions::new().map_err(w)?;
        options
            .SetDestinationWidth((size.Width * 2.0).round() as u32)
            .map_err(w)?;
        options.SetBitmapEncoderId(encoder).map_err(w)?;
        let stream = InMemoryRandomAccessStream::new().map_err(w)?;
        page.RenderWithOptionsToStreamAsync(&stream, &options)
            .and_then(|op| op.join())
            .map_err(w)?;
        let len = stream.Size().map_err(w)? as u32;
        let reader =
            DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(w)?).map_err(w)?;
        reader.LoadAsync(len).and_then(|op| op.join()).map_err(w)?;
        let mut bytes = vec![0u8; len as usize];
        reader.ReadBytes(&mut bytes).map_err(w)?;
        let suffix = (count > 1).then(|| format!("p{}", i + 1));
        let out = unique_path(dir, stem, suffix.as_deref(), fmt);
        fs::write(&out, bytes).map_err(|e| e.to_string())?;
        outputs.push(out);
        progress((i + 1) as f32 / count as f32);
    }
    Ok(outputs)
}

#[allow(clippy::too_many_arguments)]
fn ffmpeg_convert(
    input: &Path,
    out: &Path,
    fmt: &str,
    category: Category,
    p: &Preset,
    hardware: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let info = probe(input)?;
    let containers = ["mp4", "m4v", "mov", "mkv"];
    if !has_ops(p) && containers.contains(&fmt) && containers.contains(&ext_of(input).as_str()) {
        let mut args = base_args(input);
        args.extend(["-map", "0", "-c", "copy"].map(String::from));
        args.push(out.to_string_lossy().into());
        if run_ffmpeg(&args, info.duration, cancel, progress).is_ok() {
            return Ok(());
        }
    }
    let mut p = p.clone();
    let input_size = fs::metadata(input).map(|m| m.len() as f64).unwrap_or(0.0);
    if f64::from(p.max_size_mb) * 1048576.0 >= input_size {
        p.max_size_mb = 0.0;
    }
    let encoder = if hardware { hw_encoder() } else { None };
    let args = build_args(input, out, fmt, category, &p, &info, encoder);
    run_ffmpeg(&args, info.duration, cancel, progress)
}

fn has_ops(p: &Preset) -> bool {
    p.max_size_mb > 0.0
        || p.max_dim > 0
        || (p.scale_pct > 0 && p.scale_pct < 100)
        || p.quality > 0
        || p.bitrate_kbps > 0
        || p.fps > 0
        || p.rotate > 0
        || p.volume_db != 0.0
        || p.normalize
        || p.trim_silence
        || p.mono
        || p.mute
        || p.strip_metadata
}

fn base_args(input: &Path) -> Vec<String> {
    let mut a: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-y",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
        "-i",
    ]
    .map(String::from)
    .to_vec();
    a.push(input.to_string_lossy().into());
    a
}

fn image_filters(p: &Preset) -> Vec<String> {
    let mut vf = Vec::new();
    match p.rotate {
        90 => vf.push("transpose=1".into()),
        180 => vf.push("transpose=1,transpose=1".into()),
        270 => vf.push("transpose=2".into()),
        _ => {}
    }
    if p.scale_pct > 0 && p.scale_pct < 100 {
        vf.push(format!(
            "scale=trunc(iw*{0}/200)*2:trunc(ih*{0}/200)*2",
            p.scale_pct
        ));
    }
    if p.max_dim > 0 {
        vf.push(format!(
            "scale=w='if(gte(iw,ih),min(iw,{0}),-2)':h='if(gte(iw,ih),-2,min(ih,{0}))'",
            p.max_dim
        ));
    }
    if p.fps > 0 {
        vf.push(format!("fps={}", p.fps));
    }
    vf
}

pub fn target_bitrates(max_mb: f32, duration: Option<f64>, audio: bool) -> Option<(u32, u32)> {
    let d = duration.filter(|d| *d > 0.0)?;
    if max_mb <= 0.0 {
        return None;
    }
    let total = (f64::from(max_mb) * MIB_KBIT * 0.92 / d) as u32;
    let a = if audio { (total / 8).clamp(32, 128) } else { 0 };
    Some((total.saturating_sub(a).max(50), a))
}

fn push(a: &mut Vec<String>, items: &[&str]) {
    a.extend(items.iter().map(|s| s.to_string()));
}

pub fn build_args(
    input: &Path,
    out: &Path,
    fmt: &str,
    category: Category,
    p: &Preset,
    info: &Info,
    encoder: Option<&str>,
) -> Vec<String> {
    let mut a = base_args(input);
    let mut vf = image_filters(p);
    let mut af: Vec<String> = Vec::new();
    let audio_out = matches!(fmt, "mp3" | "wav" | "flac" | "opus" | "ogg" | "m4a" | "aac");
    let h264 = matches!(fmt, "mp4" | "m4v" | "mov" | "mkv");
    let anim_gif_in = category == Category::Image && ext_of(input) == "gif";
    let audio = info.has_audio && !p.mute;
    let target = target_bitrates(p.max_size_mb, info.duration, audio);
    let crf = |default: u32| {
        if p.quality > 0 {
            51 - p.quality.min(100) * 33 / 100
        } else {
            default
        }
    };

    if category == Category::Image && matches!(fmt, "jpg" | "bmp") {
        vf.push(WHITE_BG.into());
    }
    match fmt {
        "ico" => vf.push(
            "scale=w='min(iw,256)':h='min(ih,256)':force_original_aspect_ratio=decrease".into(),
        ),
        "gif" => {
            if category == Category::Video {
                if p.fps == 0 {
                    vf.push("fps=15".into());
                }
                if p.max_dim == 0 && p.scale_pct == 0 {
                    vf.push("scale='min(iw,480)':-2".into());
                }
            }
            vf.push("split[a][b];[a]palettegen[p];[b][p]paletteuse".into());
        }
        _ => {}
    }
    if h264 || fmt == "webm" {
        vf.push("scale=trunc(iw/2)*2:trunc(ih/2)*2".into());
    }

    if p.volume_db != 0.0 {
        af.push(format!("volume={}dB", p.volume_db));
    }
    if p.normalize {
        af.push("loudnorm=I=-14:TP=-1.5:LRA=11".into());
    }
    if p.trim_silence {
        af.push(
            "silenceremove=start_periods=1:start_threshold=-50dB,areverse,silenceremove=start_periods=1:start_threshold=-50dB,areverse"
                .into(),
        );
    }

    let abr = |default: u32| -> String {
        let kbps = match (p.bitrate_kbps, target) {
            (b, _) if b > 0 => b,
            (_, Some((_, a))) if a > 0 => a,
            _ => default,
        };
        format!("{kbps}k")
    };

    match fmt {
        "mp4" | "m4v" | "mov" | "mkv" => {
            let enc = encoder.unwrap_or("libx264");
            push(&mut a, &["-c:v", enc]);
            match (enc, target) {
                (_, Some((v, _))) => {
                    a.extend([
                        "-b:v".into(),
                        format!("{v}k"),
                        "-maxrate".into(),
                        format!("{v}k"),
                        "-bufsize".into(),
                        format!("{}k", v * 2),
                    ]);
                }
                ("h264_nvenc", None) => a.extend([
                    "-preset".into(),
                    "p5".into(),
                    "-cq".into(),
                    crf(24).to_string(),
                ]),
                ("h264_qsv", None) => a.extend(["-global_quality".into(), crf(24).to_string()]),
                ("h264_amf", None) => a.extend([
                    "-rc".into(),
                    "cqp".into(),
                    "-qp_i".into(),
                    crf(22).to_string(),
                    "-qp_p".into(),
                    crf(24).to_string(),
                ]),
                (_, None) => a.extend([
                    "-preset".into(),
                    "medium".into(),
                    "-crf".into(),
                    crf(23).to_string(),
                ]),
            }
            push(&mut a, &["-pix_fmt", "yuv420p"]);
            if audio {
                a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), abr(160)]);
            }
            if fmt != "mkv" {
                push(&mut a, &["-movflags", "+faststart"]);
            }
        }
        "webm" => {
            push(
                &mut a,
                &[
                    "-c:v",
                    "libvpx-vp9",
                    "-row-mt",
                    "1",
                    "-deadline",
                    "good",
                    "-cpu-used",
                    "4",
                ],
            );
            match target {
                Some((v, _)) => a.extend(["-b:v".into(), format!("{v}k")]),
                None => a.extend([
                    "-crf".into(),
                    crf(33).to_string(),
                    "-b:v".into(),
                    "0".into(),
                ]),
            }
            if audio {
                a.extend(["-c:a".into(), "libopus".into(), "-b:a".into(), abr(128)]);
            }
        }
        "avi" => {
            push(&mut a, &["-c:v", "mpeg4", "-q:v", "4"]);
            if audio {
                push(&mut a, &["-c:a", "libmp3lame", "-q:a", "3"]);
            }
        }
        "gif" => push(&mut a, &["-loop", "0", "-an"]),
        "mp3" => {
            push(&mut a, &["-vn", "-c:a", "libmp3lame"]);
            if p.bitrate_kbps > 0 || target.is_some() {
                a.extend(["-b:a".into(), abr(192)]);
            } else {
                push(&mut a, &["-q:a", "2"]);
            }
        }
        "m4a" | "aac" => a.extend([
            "-vn".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            abr(192),
        ]),
        "wav" => push(&mut a, &["-vn", "-c:a", "pcm_s16le"]),
        "flac" => push(&mut a, &["-vn", "-c:a", "flac"]),
        "opus" => a.extend([
            "-vn".into(),
            "-c:a".into(),
            "libopus".into(),
            "-b:a".into(),
            abr(128),
        ]),
        "ogg" => {
            push(&mut a, &["-vn", "-c:a", "libvorbis"]);
            if p.bitrate_kbps > 0 || target.is_some() {
                a.extend(["-b:a".into(), abr(160)]);
            } else {
                push(&mut a, &["-q:a", "5"]);
            }
        }
        "jpg" => {
            let q = if p.quality > 0 {
                2 + (100 - p.quality.min(100)) * 29 / 99
            } else {
                3
            };
            a.extend([
                "-q:v".into(),
                q.to_string(),
                "-frames:v".into(),
                "1".into(),
                "-update".into(),
                "1".into(),
            ]);
        }
        "png" | "bmp" | "tiff" => push(&mut a, &["-frames:v", "1", "-update", "1"]),
        "ico" => push(&mut a, &["-frames:v", "1"]),
        "avif" => {
            push(
                &mut a,
                &[
                    "-c:v",
                    "libaom-av1",
                    "-still-picture",
                    "1",
                    "-frames:v",
                    "1",
                ],
            );
            a.extend(["-crf".into(), crf(30).to_string()]);
        }
        "webp" => {
            let q = if p.quality > 0 {
                p.quality.min(100)
            } else {
                85
            };
            if anim_gif_in {
                push(&mut a, &["-c:v", "libwebp_anim", "-loop", "0"]);
            } else {
                push(&mut a, &["-c:v", "libwebp", "-frames:v", "1"]);
            }
            a.extend(["-quality".into(), q.to_string()]);
        }
        _ => {}
    }
    if p.mute {
        a.push("-an".into());
    }
    if p.mono {
        push(&mut a, &["-ac", "1"]);
    }
    if p.strip_metadata {
        push(&mut a, &["-map_metadata", "-1"]);
    }
    if !vf.is_empty() && !audio_out {
        a.extend(["-vf".into(), vf.join(",")]);
    }
    if !af.is_empty() && !p.mute {
        a.extend(["-af".into(), af.join(",")]);
    }
    a.push(out.to_string_lossy().into());
    a
}

fn path_dirs() -> Vec<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect()
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| vec![d.join("ffmpeg.exe")]))
        .unwrap_or_default();
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Microsoft\WinGet\Links\ffmpeg.exe"));
    }
    candidates.extend(path_dirs().into_iter().map(|d| d.join("ffmpeg.exe")));
    candidates.into_iter().find(|p| p.exists())
}

pub fn ffmpeg() -> PathBuf {
    find_ffmpeg().unwrap_or_else(|| "ffmpeg".into())
}

pub fn soffice() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(env::var_os)
        .map(|p| PathBuf::from(p).join(r"LibreOffice\program\soffice.exe"))
        .collect();
    candidates.extend(path_dirs().into_iter().map(|d| d.join("soffice.exe")));
    candidates.into_iter().find(|p| p.exists())
}

fn spawn_err(e: io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        "ffmpeg.exe not found. Put it next to crystal-converter.exe or on PATH.".into()
    } else {
        e.to_string()
    }
}

fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(NO_WINDOW).stdin(Stdio::null());
    c
}

pub fn probe(input: &Path) -> Result<Info, String> {
    let out = command(ffmpeg())
        .arg("-hide_banner")
        .arg("-i")
        .arg(input)
        .output()
        .map_err(spawn_err)?;
    Ok(parse_probe(&String::from_utf8_lossy(&out.stderr)))
}

fn parse_probe(text: &str) -> Info {
    let duration = text
        .split("Duration: ")
        .nth(1)
        .and_then(|s| s.get(..11))
        .and_then(|hms| {
            let mut parts = hms.split(':').map(|x| x.parse::<f64>().ok());
            Some(parts.next()?? * 3600.0 + parts.next()?? * 60.0 + parts.next()??)
        });
    Info {
        duration,
        has_audio: text.contains("Audio:"),
        has_video: text.contains("Video:"),
    }
}

fn run_ffmpeg(
    args: &[String],
    duration: Option<f64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let mut child = command(ffmpeg())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_err)?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    for line in stdout.lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        if let (Some(d), Some(us)) = (duration, line.strip_prefix("out_time_us=")) {
            if let Ok(us) = us.trim().parse::<f64>() {
                progress((us / 1e6 / d).clamp(0.0, 1.0) as f32);
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let err = errors.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if status.success() {
        Ok(())
    } else {
        Err(err
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ffmpeg failed")
            .to_string())
    }
}

fn hw_encoder() -> Option<&'static str> {
    static E: OnceLock<Option<&'static str>> = OnceLock::new();
    *E.get_or_init(|| {
        ["h264_nvenc", "h264_qsv", "h264_amf"]
            .into_iter()
            .find(|enc| {
                command(ffmpeg())
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        "color=s=256x256:d=0.1",
                    ])
                    .args(["-frames:v", "1", "-c:v", enc, "-f", "null", "-"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            })
    })
}

fn office(input: &Path, fmt: &str, out: &Path) -> Result<(), String> {
    let soffice = soffice().ok_or("LibreOffice is needed to convert documents")?;
    let tmp = env::temp_dir().join(format!(
        "crystal-converter-{}-{}",
        std::process::id(),
        next_id()
    ));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let filter = if fmt == "txt" {
        "txt:Text (encoded):UTF8"
    } else {
        fmt
    };
    let mut cmd = command(soffice);
    if ext_of(input) == "pdf" {
        cmd.arg("--infilter=writer_pdf_import");
    }
    let status = cmd
        .args([
            "--headless",
            "--norestore",
            "--convert-to",
            filter,
            "--outdir",
        ])
        .arg(&tmp)
        .arg(input)
        .status()
        .map_err(|e| e.to_string())?;
    let produced = tmp.join(format!("{}.{fmt}", stem(input)));
    let result = if status.success() && produced.exists() {
        move_file(&produced, out)
    } else {
        Err("LibreOffice could not convert this file".into())
    };
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to)
        .or_else(|_| {
            fs::copy(from, to)
                .map(|_| ())
                .and_then(|_| fs::remove_file(from))
        })
        .map_err(|e| e.to_string())
}

fn images_to_pdf(
    group: &[PathBuf],
    out: &Path,
    p: &Preset,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let mut pages = Vec::new();
    for (i, img) in group.iter().enumerate() {
        let tmp = env::temp_dir().join(format!(
            "crystal-converter-{}-{}.jpg",
            std::process::id(),
            next_id()
        ));
        let mut vf = image_filters(p);
        vf.push(WHITE_BG.into());
        let mut args = base_args(img);
        args.extend([
            "-vf".into(),
            vf.join(","),
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            "2".into(),
        ]);
        args.extend(["-update".into(), "1".into(), tmp.to_string_lossy().into()]);
        let page = run_ffmpeg(&args, None, cancel, &mut |_| {})
            .and_then(|_| fs::read(&tmp).map_err(|e| e.to_string()));
        let _ = fs::remove_file(&tmp);
        pages.push(page.map_err(|e| format!("{}: {e}", file_name(img)))?);
        progress((i + 1) as f32 / group.len() as f32);
    }
    fs::write(out, pdf_from_jpegs(&pages)?).map_err(|e| e.to_string())
}

fn jpeg_info(d: &[u8]) -> Option<(u32, u32, u8)> {
    let mut i = 2;
    while i + 9 < d.len() {
        if d[i] != 0xFF || d[i + 1] == 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let h = u16::from_be_bytes([d[i + 5], d[i + 6]]);
            let w = u16::from_be_bytes([d[i + 7], d[i + 8]]);
            return Some((u32::from(w), u32::from(h), d[i + 9]));
        }
        i += 2 + u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
    }
    None
}

pub fn pdf_from_jpegs(pages: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 3 * i)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes(),
    ];
    for (i, jpg) in pages.iter().enumerate() {
        let (w, h, comps) = jpeg_info(jpg).ok_or("unreadable JPEG page")?;
        let (pw, ph) = (w as f32 * 0.75, h as f32 * 0.75);
        let base = 3 + 3 * i;
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                base + 1,
                base + 2
            )
            .into_bytes(),
        );
        let cs = match comps {
            1 => "/DeviceGray",
            4 => "/DeviceCMYK",
            _ => "/DeviceRGB",
        };
        let mut img = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            jpg.len()
        )
        .into_bytes();
        img.extend_from_slice(jpg);
        img.extend_from_slice(b"\nendstream");
        objs.push(img);
        let content = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im0 Do Q");
        objs.push(
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .into_bytes(),
        );
    }
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    Ok(out)
}

fn next_id() -> u32 {
    static N: AtomicU32 = AtomicU32::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into())
}

pub fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '+' | '_'))
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "edited".into()
    } else {
        s
    }
}

pub fn unique_path(dir: &Path, stem: &str, suffix: Option<&str>, ext: &str) -> PathBuf {
    let base = match suffix {
        Some(s) => format!("{stem}-{s}"),
        None => stem.to_string(),
    };
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_output() {
        let info = parse_probe("  Duration: 00:01:02.50, start: 0\n Stream #0:0: Video: h264\n Stream #0:1: Audio: aac");
        assert_eq!(
            info,
            Info {
                duration: Some(62.5),
                has_audio: true,
                has_video: true
            }
        );
        assert_eq!(parse_probe("Duration: N/A, bitrate").duration, None);
    }

    #[test]
    fn target_bitrate_fits_size() {
        let (v, a) = target_bitrates(10.0, Some(60.0), true).unwrap();
        let mb = f64::from(v + a) * 60.0 / MIB_KBIT;
        assert!(mb < 10.0 && mb > 8.5, "{mb}");
        assert_eq!(target_bitrates(10.0, None, true), None);
    }

    #[test]
    fn slugs_and_unique_names() {
        assert_eq!(slug("Discord 10 MB"), "discord-10-mb");
        assert_eq!(slug("+6 dB"), "+6-db");
        assert_eq!(slug("50%"), "50");
        let dir = env::temp_dir().join(format!("cc-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a", None, "png"), dir.join("a (1).png"));
        assert_eq!(
            unique_path(&dir, "a", Some("50"), "png"),
            dir.join("a-50.png")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pdf_has_valid_xref() {
        let jpg = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 16, 0, 32, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
            0xFF, 0xD9,
        ];
        assert_eq!(jpeg_info(&jpg), Some((32, 16, 3)));
        let pdf = pdf_from_jpegs(&[jpg.clone(), jpg]).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 2"));
        let xref: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[xref..].starts_with(b"xref"));
    }

    #[test]
    fn builds_expected_args() {
        let info = Info {
            duration: Some(10.0),
            has_audio: true,
            has_video: true,
        };
        let p = Preset::convert_to("mp4");
        let a = build_args(
            Path::new("in.mov"),
            Path::new("out.mp4"),
            "mp4",
            Category::Video,
            &p,
            &info,
            None,
        );
        assert!(a.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(a.contains(&"+faststart".to_string()));
        let a = build_args(
            Path::new("in.mov"),
            Path::new("out.mp3"),
            "mp3",
            Category::Video,
            &p,
            &info,
            None,
        );
        assert!(a.contains(&"-vn".to_string()) && !a.contains(&"-vf".to_string()));
        let small = Preset {
            max_size_mb: 8.0,
            ..Preset::default()
        };
        let a = build_args(
            Path::new("in.mp4"),
            Path::new("o.mp4"),
            "mp4",
            Category::Video,
            &small,
            &info,
            Some("h264_nvenc"),
        );
        assert!(a.contains(&"-maxrate".to_string()) && a.contains(&"h264_nvenc".to_string()));
    }

    #[test]
    #[ignore = "needs ffmpeg with libaom, libwebp and libvpx"]
    fn real_ffmpeg_round_trip() {
        if command(ffmpeg()).arg("-version").output().is_err() {
            return;
        }
        let dir = env::temp_dir().join(format!("cc-ff-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("clip.mp4");
        let made = command(ffmpeg())
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=321x241:d=2",
                "-f",
                "lavfi",
                "-i",
                "sine=d=2",
            ])
            .args(["-shortest", "-pix_fmt", "yuv420p", "-vf", "scale=320:240"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(made.success());
        let png = dir.join("pic.png");
        assert!(command(ffmpeg())
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=99x77:d=1",
                "-frames:v",
                "1"
            ])
            .arg(&png)
            .status()
            .unwrap()
            .success());
        let cases: Vec<(PathBuf, Preset)> = vec![
            (src.clone(), Preset::convert_to("webm")),
            (src.clone(), Preset::convert_to("gif")),
            (src.clone(), Preset::convert_to("mp3")),
            (src.clone(), Preset::convert_to("mkv")),
            (
                src.clone(),
                Preset {
                    max_size_mb: 0.1,
                    ..Preset::named("Tiny", Category::Video)
                },
            ),
            (png.clone(), Preset::convert_to("jpg")),
            (png.clone(), Preset::convert_to("webp")),
            (png.clone(), Preset::convert_to("ico")),
            (png.clone(), Preset::convert_to("avif")),
            (png.clone(), Preset::convert_to("pdf")),
            (dir.join("pic.pdf"), Preset::convert_to("png")),
            (
                png.clone(),
                Preset {
                    scale_pct: 50,
                    rotate: 90,
                    ..Preset::named("Half", Category::Image)
                },
            ),
        ];
        let cancel = AtomicBool::new(false);
        for (file, preset) in cases {
            let name = preset.name.clone();
            let mut outcome = None;
            run(
                Job {
                    files: vec![file],
                    preset,
                    out_dir: None,
                    hardware: false,
                },
                &cancel,
                &mut |e| {
                    if let Event::Done { outputs, .. } = &e {
                        outcome = Some(Ok(outputs[0].clone()));
                    } else if let Event::Failed(msg) = &e {
                        outcome = Some(Err(msg.clone()));
                    }
                },
            );
            let out = outcome.unwrap().unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(fs::metadata(&out).unwrap().len() > 0, "{name}");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
