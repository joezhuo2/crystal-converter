use crate::config::{Category, Preset};
use crate::formats::{detect, ext_of};
use crate::{archive, markdown as md, ocr, pdfmerge};
use std::{
    cell::Cell,
    env, fs,
    io::{self, BufRead, BufReader, Read},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
        mpsc, OnceLock,
    },
};

const NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;
const WHITE_BG: &str = "format=rgba,split[fg][bg];[bg]lutrgb=r=255:g=255:b=255:a=255[w];[w][fg]overlay=format=auto,format=rgb24";
const MIB_KBIT: f64 = 8192.0;
const FAVICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];
const RINGTONE_MAX_SECS: &str = "40";

pub enum Event {
    Progress {
        index: usize,
        total: usize,
        fraction: Option<f32>,
        name: String,
        speed: Option<f32>,
    },
    Done {
        outputs: Vec<PathBuf>,
        failed: Vec<String>,
        total: usize,
    },
    Failed(String),
    Cancelled,
}

pub struct Job {
    pub files: Vec<PathBuf>,
    pub preset: Preset,
    pub out_dir: Option<PathBuf>,
    pub hardware: bool,
    pub parallel: usize,
    pub replace_original: bool,
}

enum Msg {
    Start(usize),
    Frac(usize, f32, Option<f32>),
    End(usize, Result<Vec<PathBuf>, String>),
}

thread_local! {
    static SPEED: Cell<Option<f32>> = const { Cell::new(None) };
}

#[derive(Default, Debug, PartialEq)]
pub struct Info {
    pub duration: Option<f64>,
    pub has_audio: bool,
    pub has_video: bool,
}

pub fn run(job: Job, cancel: &AtomicBool, emit: &mut dyn FnMut(Event)) {
    let grouped = matches!(job.preset.format.as_str(), "merge" | "zip")
        || matches!(job.preset.op.as_str(), "join" | "slideshow")
        || job.preset.format == "pdf"
            && job.files.iter().all(|f| detect(f) == Some(Category::Image));
    let groups: Vec<Vec<PathBuf>> = if grouped {
        vec![job.files.clone()]
    } else {
        job.files.iter().map(|f| vec![f.clone()]).collect()
    };
    let total = groups.len();
    let light = groups
        .iter()
        .all(|g| matches!(detect(&g[0]), Some(Category::Image | Category::Audio)));
    let workers = if light {
        job.parallel.min(total).max(1)
    } else {
        1
    };
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let mut results: Vec<Option<Result<Vec<PathBuf>, String>>> = vec![None; total];
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (groups, job, next) = (&groups, &job, &next);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= groups.len() || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = tx.send(Msg::Start(i));
                let result = convert(&groups[i], job, cancel, &mut |f| {
                    let _ = tx.send(Msg::Frac(i, f, SPEED.take()));
                });
                if let Ok(outputs) = &result {
                    if job.replace_original
                        && !cancel.load(Ordering::Relaxed)
                        && replaceable(&groups[i], outputs)
                    {
                        crate::sys::recycle(&groups[i]);
                    }
                }
                let _ = tx.send(Msg::End(i, result));
            });
        }
        drop(tx);
        let mut frac = vec![0f32; total];
        let mut done = 0;
        let mut name = String::new();
        let mut speed = None;
        for msg in rx {
            match msg {
                Msg::Start(i) => name = file_name(&groups[i][0]),
                Msg::Frac(i, f, s) => {
                    frac[i] = f;
                    speed = s.filter(|_| workers == 1);
                }
                Msg::End(i, result) => {
                    frac[i] = 1.0;
                    done += 1;
                    results[i] = Some(result);
                    continue;
                }
            }
            let overall = frac.iter().sum::<f32>() / total as f32;
            emit(Event::Progress {
                index: done.min(total - 1),
                total,
                fraction: (overall > 0.0).then_some(overall),
                name: name.clone(),
                speed,
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        emit(Event::Cancelled);
        return;
    }
    let mut outputs = Vec::new();
    let mut failed = Vec::new();
    for (group, result) in groups.iter().zip(results) {
        match result {
            Some(Ok(p)) => outputs.extend(p),
            Some(Err(e)) => failed.push(format!("{}: {e}", file_name(&group[0]))),
            None => {}
        }
    }
    emit(if outputs.is_empty() {
        Event::Failed(failed.join(
            "
",
        ))
    } else {
        Event::Done {
            outputs,
            failed,
            total,
        }
    });
}

fn replaceable(sources: &[PathBuf], outputs: &[PathBuf]) -> bool {
    !outputs.is_empty()
        && outputs.iter().all(|o| {
            !sources.contains(o) && fs::metadata(o).is_ok_and(|m| m.is_file() && m.len() > 0)
        })
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
    let suffix = (fmt == in_ext || !p.op.is_empty()).then(|| slug(&p.name));
    let dir = match &job.out_dir {
        Some(d) => d.clone(),
        None => input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if category == Category::Document && in_ext == "pdf" && matches!(fmt.as_str(), "png" | "jpg") {
        return pdf_pages(input, &dir, &stem(input), &fmt, cancel, progress);
    }
    let out = if category == Category::Archive {
        archive::unique_dir(&dir, &stem(input))
    } else {
        let ext = if fmt == "merge" { "pdf" } else { &fmt };
        unique_path(&dir, &stem(input), suffix.as_deref(), ext)
    };
    let result = match category {
        Category::Archive => archive::extract(input, &out),
        _ if fmt == "zip" => archive::zip(group, &out),
        Category::Document if in_ext == "pdf" && fmt == "merge" => pdfmerge::merge(group, &out),
        Category::Document if in_ext == "pdf" && fmt == "txt" => {
            ocr_pdf(input, &out, cancel, progress)
        }
        Category::Image if fmt == "txt" => ocr_image(input, &out, cancel),
        Category::Document if in_ext == "md" => markdown(input, &fmt, &out, cancel),
        Category::Document => office(input, &fmt, &out),
        Category::Image if p.favicon => favicon(input, &out, cancel, progress),
        Category::Image if p.lossless => lossless_png(input, &out, cancel, progress),
        Category::Image if p.strip_gps => strip_gps_file(input, &out),
        Category::Image if fmt == "pdf" => images_to_pdf(group, &out, p, cancel, progress),
        _ if p.op == "join" => join(
            group,
            &out,
            &fmt,
            category,
            p,
            job.hardware,
            cancel,
            progress,
        ),
        Category::Image if p.op == "slideshow" => slideshow(group, &out, cancel, progress),
        Category::Video if fmt == "srt" => subtitles(input, &out, cancel, progress),
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
    if p.trim_end > 0.0 && p.trim_end <= p.trim_start {
        return Err("trim end must be after trim start".into());
    }
    let mut info = probe(input)?;
    info.duration = trim_span(p, info.duration);
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

fn trim_span(p: &Preset, duration: Option<f64>) -> Option<f64> {
    duration.map(|d| {
        let end = if p.trim_end > 0.0 {
            d.min(f64::from(p.trim_end))
        } else {
            d
        };
        (end - f64::from(p.trim_start)).max(0.1)
    })
}

fn has_ops(p: &Preset) -> bool {
    !p.op.is_empty()
        || p.trim_start > 0.0
        || p.trim_end > 0.0
        || p.max_size_mb > 0.0
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
    if let Some((w, h)) = parse_ratio(&p.crop) {
        vf.push(format!(
            "crop='if(gte(iw/ih,{w}/{h}),ih*{w}/{h},iw)':'if(gte(iw/ih,{w}/{h}),ih,iw*{h}/{w})'"
        ));
    }
    if p.grayscale {
        vf.push("hue=s=0".into());
    }
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

fn parse_ratio(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(':')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

fn audio_edit_filters(p: &Preset, duration: Option<f64>) -> (Vec<String>, Vec<String>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let start = f64::from(p.cut_start.max(0.0));
    if start > 0.0 || p.cut_len > 0.0 {
        let mut t = format!("atrim=start={start}");
        if p.cut_len > 0.0 {
            t.push_str(&format!(":duration={}", p.cut_len));
        }
        head.push(t);
        head.push("asetpts=PTS-STARTPTS".into());
    }
    if p.fade_s > 0.0 {
        let len = match (duration.map(|d| d - start), p.cut_len > 0.0) {
            (Some(a), true) => Some(a.min(f64::from(p.cut_len))),
            (Some(a), false) => Some(a),
            (None, true) => Some(f64::from(p.cut_len)),
            (None, false) => None,
        };
        let fade = len.map_or(f64::from(p.fade_s), |l| f64::from(p.fade_s).min(l / 2.0));
        tail.push(format!("afade=t=in:d={fade:.3}"));
        if let Some(l) = len {
            tail.push(format!("afade=t=out:st={:.3}:d={fade:.3}", l - fade));
        }
    }
    if p.speed > 0.0 && p.speed != 1.0 {
        tail.push(format!("atempo={}", p.speed.clamp(0.5, 100.0)));
    }
    (head, tail)
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
    if p.trim_start > 0.0 {
        a.extend(["-ss".into(), p.trim_start.to_string()]);
    }
    if p.trim_end > 0.0 {
        a.extend(["-to".into(), p.trim_end.to_string()]);
    }
    let mut vf = image_filters(p);
    let mut af: Vec<String> = Vec::new();
    let audio_out = matches!(
        fmt,
        "mp3" | "wav" | "flac" | "opus" | "ogg" | "m4a" | "m4r" | "aac"
    );
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
    match p.op.as_str() {
        "speed2" => {
            vf.push("setpts=PTS/2".into());
            af.push("atempo=2".into());
        }
        "hdr" => vf.push(hdr_chain(ffmpeg_filters()).into()),
        "thumb" => {
            if let Some(d) = info.duration {
                a.extend(["-ss".into(), format!("{:.2}", d * 0.1)]);
            }
        }
        "sheet" => vf.push(sheet_filter(info.duration)),
        _ => {}
    }
    match fmt {
        "webp" if category == Category::Video => {
            if p.fps == 0 {
                vf.push("fps=15".into());
            }
            if p.max_dim == 0 && p.scale_pct == 0 {
                vf.push("scale='min(iw,480)':-2".into());
            }
        }
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

    let (edit_head, edit_tail) = audio_edit_filters(p, info.duration);
    af.splice(0..0, edit_head);
    af.extend(edit_tail);

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
        "m4r" => push(
            &mut a,
            &[
                "-vn",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-t",
                RINGTONE_MAX_SECS,
                "-f",
                "ipod",
            ],
        ),
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
            if anim_gif_in || category == Category::Video {
                push(&mut a, &["-c:v", "libwebp_anim", "-loop", "0", "-an"]);
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

pub fn hdr_chain(filters: &str) -> &'static str {
    if filters.contains(" zscale ") {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
    } else if filters.contains(" tonemapx ") {
        "tonemapx=tonemap=hable:desat=0:p=bt709:t=bt709:m=bt709:r=tv:format=yuv420p"
    } else {
        "colorspace=all=bt709:iall=bt2020:fast=1,format=yuv420p"
    }
}

fn ffmpeg_filters() -> &'static str {
    static F: OnceLock<String> = OnceLock::new();
    F.get_or_init(|| {
        command(ffmpeg())
            .args(["-hide_banner", "-filters"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    })
}

pub fn sheet_filter(duration: Option<f64>) -> String {
    let fps = match duration.filter(|d| *d > 0.0) {
        Some(d) => format!("{:.5}", 12.0 / d),
        None => "0.1".into(),
    };
    format!("fps={fps},scale=320:-2,tile=4x3:padding=4")
}

fn temp_file(ext: &str) -> PathBuf {
    env::temp_dir().join(format!(
        "crystal-converter-{}-{}.{ext}",
        std::process::id(),
        next_id()
    ))
}

fn favicon(
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let mut frames = Vec::new();
    for (i, size) in FAVICON_SIZES.into_iter().enumerate() {
        let tmp = temp_file("png");
        let mut args = base_args(input);
        args.extend([
            "-vf".into(),
            format!("scale={size}:{size}:force_original_aspect_ratio=decrease,pad={size}:{size}:(ow-iw)/2:(oh-ih)/2:color=black@0,format=rgba"),
            "-frames:v".into(),
            "1".into(),
            "-update".into(),
            "1".into(),
            tmp.to_string_lossy().into(),
        ]);
        let png = run_ffmpeg(&args, None, cancel, &mut |_| {})
            .and_then(|_| fs::read(&tmp).map_err(|e| e.to_string()));
        let _ = fs::remove_file(&tmp);
        frames.push((size, png?));
        progress((i + 1) as f32 / FAVICON_SIZES.len() as f32);
    }
    fs::write(out, ico_from_pngs(&frames)).map_err(|e| e.to_string())
}

pub fn ico_from_pngs(frames: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend((frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, png) in frames {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend((png.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in frames {
        out.extend_from_slice(png);
    }
    out
}

fn lossless_png(
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let data = fs::read(input).map_err(|e| e.to_string())?;
    if !data.starts_with(b"\x89PNG") {
        return Err("lossless compression works on PNG files only".into());
    }
    if data.windows(4).any(|w| w == b"acTL") {
        return Err("animated PNG is not supported".into());
    }
    let mut args = base_args(input);
    push(
        &mut args,
        &[
            "-c:v",
            "png",
            "-compression_level",
            "9",
            "-pred",
            "mixed",
            "-frames:v",
            "1",
            "-update",
            "1",
        ],
    );
    args.push(out.to_string_lossy().into());
    run_ffmpeg(&args, None, cancel, progress)?;
    if fs::metadata(out).map_or(true, |m| m.len() >= data.len() as u64) {
        fs::write(out, &data).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn strip_gps_file(input: &Path, out: &Path) -> Result<(), String> {
    let mut data = fs::read(input).map_err(|e| e.to_string())?;
    strip_gps(&mut data)?;
    fs::write(out, data).map_err(|e| e.to_string())
}

fn scrub_exif_gps(t: &mut [u8]) {
    let big = match t.get(..2) {
        Some(b"MM") => true,
        Some(b"II") => false,
        _ => return,
    };
    let u16_at = |t: &[u8], o: usize| -> Option<usize> {
        let b: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
        Some(usize::from(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }))
    };
    let u32_at = |t: &[u8], o: usize| -> Option<usize> {
        let b: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        } as usize)
    };
    let Some(ifd0) = u32_at(t, 4) else { return };
    let Some(n) = u16_at(t, ifd0) else { return };
    let entries = ifd0 + 2;
    let end = entries + 12 * n;
    if end + 4 > t.len() {
        return;
    }
    let Some(i) = (0..n).find(|i| u16_at(t, entries + 12 * i) == Some(0x8825)) else {
        return;
    };
    if let Some(gps) = u32_at(t, entries + 12 * i + 8) {
        if let Some(gn) = u16_at(t, gps) {
            let table = gps + 2;
            if table + 12 * gn + 4 <= t.len() {
                for j in 0..gn {
                    let e = table + 12 * j;
                    let unit = match u16_at(t, e + 2) {
                        Some(3 | 8) => 2,
                        Some(4 | 9 | 11) => 4,
                        Some(5 | 10 | 12) => 8,
                        _ => 1,
                    };
                    let size = u32_at(t, e + 4).unwrap_or(0).saturating_mul(unit);
                    if size > 4 {
                        let off = u32_at(t, e + 8).unwrap_or(usize::MAX);
                        if let Some(data) = t.get_mut(off..off.saturating_add(size)) {
                            data.fill(0);
                        }
                    }
                }
                t[gps..table + 12 * gn + 4].fill(0);
            }
        }
    }
    t.copy_within(entries + 12 * (i + 1)..end + 4, entries + 12 * i);
    t[end - 8..end + 4].fill(0);
    let count = (n as u16 - 1).to_be_bytes();
    let count = if big { count } else { [count[1], count[0]] };
    t[ifd0..ifd0 + 2].copy_from_slice(&count);
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub fn strip_gps(d: &mut [u8]) -> Result<(), String> {
    if d.starts_with(b"\xFF\xD8") {
        let mut i = 2;
        while i + 4 <= d.len() && d[i] == 0xFF && d[i + 1] != 0xDA {
            let len = usize::from(u16::from_be_bytes([d[i + 2], d[i + 3]]));
            let end = (i + 2 + len).min(d.len());
            if d[i + 1] == 0xE1 && end >= i + 4 && d[i + 4..end].starts_with(b"Exif\0\0") {
                scrub_exif_gps(&mut d[i + 10..end]);
            }
            i = end;
        }
    } else if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut i = 8;
        while i + 12 <= d.len() {
            let len = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
            let end = (i + 8 + len).min(d.len());
            if &d[i + 4..i + 8] == b"eXIf" && end + 4 <= d.len() {
                scrub_exif_gps(&mut d[i + 8..end]);
                let crc = crc32(&d[i + 4..end]).to_be_bytes();
                d[end..end + 4].copy_from_slice(&crc);
            }
            i = end + 4;
        }
    } else if d.starts_with(b"RIFF") && d.get(8..12) == Some(b"WEBP") {
        let mut i = 12;
        while i + 8 <= d.len() {
            let len = u32::from_le_bytes([d[i + 4], d[i + 5], d[i + 6], d[i + 7]]) as usize;
            let end = (i + 8 + len).min(d.len());
            if &d[i..i + 4] == b"EXIF" {
                let skip = if d[i + 8..end].starts_with(b"Exif\0\0") {
                    6
                } else {
                    0
                };
                scrub_exif_gps(&mut d[i + 8 + skip..end]);
            }
            i = end + (len & 1);
        }
    } else {
        return Err("GPS removal supports JPEG, PNG and WebP only".into());
    }
    Ok(())
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

pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(NO_WINDOW | BELOW_NORMAL_PRIORITY)
        .stdin(Stdio::null());
    c
}

fn probe_text(input: &Path) -> Result<String, String> {
    let out = command(ffmpeg())
        .arg("-hide_banner")
        .arg("-i")
        .arg(input)
        .output()
        .map_err(spawn_err)?;
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

pub fn probe(input: &Path) -> Result<Info, String> {
    probe_meta(input).map(|(info, _)| info)
}

pub fn probe_meta(input: &Path) -> Result<(Info, Option<(u32, u32)>), String> {
    let text = probe_text(input)?;
    Ok((parse_probe(&text), parse_dims(&text)))
}

fn parse_dims(text: &str) -> Option<(u32, u32)> {
    text.split("Video:")
        .nth(1)?
        .lines()
        .next()?
        .split([' ', ','])
        .find_map(|t| {
            let (w, h) = t.split_once('x')?;
            let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
            (w > 0 && h > 0).then_some((w, h))
        })
}

fn parse_speed(line: &str) -> Option<f32> {
    line.strip_prefix("speed=")?
        .trim()
        .strip_suffix('x')?
        .trim()
        .parse()
        .ok()
}

fn fmt_size(bytes: f64) -> String {
    let mb = bytes / 1048576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes / 1024.0)
    }
}

pub fn estimate(p: &Preset, size: u64, info: &Info, dims: Option<(u32, u32)>) -> Option<String> {
    if p.max_size_mb > 0.0 && f64::from(p.max_size_mb) * 1048576.0 < size as f64 {
        let audio = info.has_audio && !p.mute;
        if let Some((v, a)) = target_bitrates(p.max_size_mb, info.duration, audio) {
            let bytes = f64::from(v + a) * 125.0 * info.duration.unwrap_or_default();
            return Some(format!("~{}", fmt_size(bytes)));
        }
    }
    let (w, h) = dims?;
    let (mut nw, mut nh) = (f64::from(w), f64::from(h));
    if p.scale_pct > 0 && p.scale_pct < 100 {
        let f = f64::from(p.scale_pct) / 100.0;
        (nw, nh) = (nw * f, nh * f);
    }
    let longest = nw.max(nh);
    if p.max_dim > 0 && longest > f64::from(p.max_dim) {
        let f = f64::from(p.max_dim) / longest;
        (nw, nh) = (nw * f, nh * f);
    }
    let (nw, nh) = (nw.round() as u32, nh.round() as u32);
    (nw != w || nh != h).then(|| format!("~{nw}×{nh}"))
}

fn text_subtitle_index(text: &str) -> Option<usize> {
    text.lines()
        .filter(|l| l.contains("Stream #"))
        .filter_map(|l| l.split_once(": Subtitle: ").map(|(_, r)| r))
        .position(|r| {
            let codec = r
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(',');
            matches!(
                codec,
                "subrip" | "srt" | "ass" | "ssa" | "mov_text" | "webvtt" | "text" | "subviewer"
            )
        })
}

fn stream_sig(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.contains("Stream #"))
        .filter_map(|l| {
            let (kind, rest) = ["Video", "Audio"]
                .into_iter()
                .find_map(|k| l.split_once(&format!(": {k}: ")).map(|(_, r)| (k, r)))?;
            let words: Vec<&str> = rest
                .split_whitespace()
                .map(|w| w.trim_end_matches(','))
                .collect();
            let near = |m: &str, d: isize| {
                words
                    .iter()
                    .position(|w| *w == m)
                    .and_then(|i| words.get(i.wrapping_add_signed(d)))
                    .copied()
                    .unwrap_or("")
            };
            let size = words
                .iter()
                .find(|w| {
                    w.split_once('x')
                        .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
                })
                .copied()
                .unwrap_or("");
            Some(format!(
                "{kind} {} {size} {} {} {}",
                words.first().copied().unwrap_or(""),
                near("fps", -1),
                near("Hz", -1),
                near("Hz", 1)
            ))
        })
        .collect()
}

fn concat_list(files: &[PathBuf], seconds: Option<u32>) -> String {
    let entry = |f: &PathBuf| {
        format!(
            "file '{}'\n",
            f.to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''")
        )
    };
    let mut s = String::new();
    for f in files {
        s.push_str(&entry(f));
        if let Some(d) = seconds {
            s.push_str(&format!("duration {d}\n"));
        }
    }
    if seconds.is_some() {
        if let Some(last) = files.last() {
            s.push_str(&entry(last));
        }
    }
    s
}

fn concat_input(args: &mut Vec<String>) {
    if let Some(i) = args.iter().position(|a| a == "-i") {
        args.splice(i..i, ["-f", "concat", "-safe", "0"].map(String::from));
    }
}

fn write_list(text: &str) -> Result<PathBuf, String> {
    let path = env::temp_dir().join(format!(
        "crystal-converter-{}-{}.txt",
        std::process::id(),
        next_id()
    ));
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

const SLIDE_SECONDS: u32 = 3;

fn slideshow_args(list: &Path, out: &Path) -> Vec<String> {
    let mut a = base_args(list);
    concat_input(&mut a);
    push(
        &mut a,
        &[
            "-vf",
            "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p",
            "-r",
            "30",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ],
    );
    a.push(out.to_string_lossy().into());
    a
}

fn slideshow(
    group: &[PathBuf],
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let list = write_list(&concat_list(group, Some(SLIDE_SECONDS)))?;
    let total = f64::from(SLIDE_SECONDS) * group.len() as f64;
    let result = run_ffmpeg(&slideshow_args(&list, out), Some(total), cancel, progress);
    let _ = fs::remove_file(list);
    result
}

fn subtitles(
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let index =
        text_subtitle_index(&probe_text(input)?).ok_or("no text subtitle stream in this file")?;
    let mut a = base_args(input);
    a.extend(["-map".into(), format!("0:s:{index}")]);
    push(&mut a, &["-c:s", "srt"]);
    a.push(out.to_string_lossy().into());
    run_ffmpeg(&a, None, cancel, progress)
}

#[allow(clippy::too_many_arguments)]
fn join(
    group: &[PathBuf],
    out: &Path,
    fmt: &str,
    category: Category,
    p: &Preset,
    hardware: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    if group.len() < 2 {
        return Err("joining needs at least two files".into());
    }
    let mut sigs = Vec::new();
    let mut total = Some(0.0);
    let mut first = Info::default();
    for (i, f) in group.iter().enumerate() {
        let text = probe_text(f).map_err(|e| format!("{}: {e}", file_name(f)))?;
        let info = parse_probe(&text);
        sigs.push(stream_sig(&text));
        total = total.zip(info.duration).map(|(a, b)| a + b);
        if i == 0 {
            first = info;
        }
    }
    let list = write_list(&concat_list(group, None))?;
    let mut args = if sigs.iter().all(|s| *s == sigs[0]) {
        let mut a = base_args(&list);
        push(&mut a, &["-c", "copy"]);
        a.push(out.to_string_lossy().into());
        a
    } else {
        let info = Info {
            duration: total,
            ..first
        };
        let encoder = if hardware { hw_encoder() } else { None };
        build_args(&list, out, fmt, category, p, &info, encoder)
    };
    concat_input(&mut args);
    let result = run_ffmpeg(&args, total, cancel, progress);
    let _ = fs::remove_file(list);
    result
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
    let mut speed = None;
    for line in stdout.lines().map_while(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        if let Some(s) = parse_speed(&line) {
            speed = Some(s);
        }
        if let (Some(d), Some(us)) = (duration, line.strip_prefix("out_time_us=")) {
            if let Ok(us) = us.trim().parse::<f64>() {
                SPEED.set(speed);
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

fn write_text(out: &Path, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("no text found".into());
    }
    fs::write(out, text).map_err(|e| e.to_string())
}

fn ocr_image(input: &Path, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let tmp = env::temp_dir().join(format!(
        "crystal-converter-{}-{}.png",
        std::process::id(),
        next_id()
    ));
    let mut vf = image_filters(&Preset {
        max_dim: 4000,
        ..Preset::default()
    });
    vf.push(WHITE_BG.into());
    let mut args = base_args(input);
    args.extend(["-vf".into(), vf.join(","), "-frames:v".into(), "1".into()]);
    args.extend(["-update".into(), "1".into(), tmp.to_string_lossy().into()]);
    let text = run_ffmpeg(&args, None, cancel, &mut |_| {}).and_then(|_| ocr::recognize(&tmp));
    let _ = fs::remove_file(&tmp);
    write_text(out, text?)
}

fn ocr_pdf(
    input: &Path,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(), String> {
    let tmp = env::temp_dir().join(format!(
        "crystal-converter-{}-{}",
        std::process::id(),
        next_id()
    ));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        let pages = pdf_pages(input, &tmp, "page", "png", cancel, &mut |f| {
            progress(f / 2.0)
        })?;
        let mut texts = Vec::new();
        for (i, page) in pages.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            texts.push(ocr::recognize(page)?);
            progress(0.5 + (i + 1) as f32 / pages.len() as f32 / 2.0);
        }
        write_text(out, texts.join("\n\n"))
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn edge() -> Option<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(env::var_os)
        .map(|p| PathBuf::from(p).join(r"Microsoft\Edge\Application\msedge.exe"))
        .find(|p| p.exists())
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!(
        "file:///{}",
        s.replace('%', "%25")
            .replace('\\', "/")
            .replace(' ', "%20")
            .replace('#', "%23")
    )
}

fn markdown(input: &Path, fmt: &str, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let bytes = fs::read(input).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let body = md::to_html(text.trim_start_matches('\u{feff}'));
    let title = stem(input);
    if fmt == "html" {
        return fs::write(out, md::page(&title, &body, None)).map_err(|e| e.to_string());
    }
    let edge = edge().ok_or("Microsoft Edge is needed to convert Markdown to PDF")?;
    let tmp = env::temp_dir().join(format!(
        "crystal-converter-{}-{}",
        std::process::id(),
        next_id()
    ));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        let abs = std::path::absolute(input).map_err(|e| e.to_string())?;
        let base = format!("{}/", file_url(abs.parent().unwrap_or(&abs)));
        let html = tmp.join("page.html");
        let pdf = tmp.join("page.pdf");
        fs::write(&html, md::page(&title, &body, Some(&base))).map_err(|e| e.to_string())?;
        let mut child = command(edge)
            .args(["--headless", "--disable-gpu", "--no-pdf-header-footer"])
            .arg(format!("--user-data-dir={}", tmp.join("profile").display()))
            .arg(format!("--print-to-pdf={}", pdf.display()))
            .arg(file_url(&html))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        while child.try_wait().map_err(|e| e.to_string())?.is_none() {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                return Err("cancelled".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if pdf.exists() {
            move_file(&pdf, out)
        } else {
            Err("Microsoft Edge could not print this file".into())
        }
    })();
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
    Ok(write_pdf(&objs))
}

pub fn write_pdf(objs: &[Vec<u8>]) -> Vec<u8> {
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
    out
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
    fn parses_speed_and_dims() {
        assert_eq!(parse_speed("speed=1.85x"), Some(1.85));
        assert_eq!(parse_speed("speed=  12x"), Some(12.0));
        assert_eq!(parse_speed("speed=N/A"), None);
        let dims = parse_dims(
            "Stream #0:0: Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv), 1920x1080 [SAR 1:1 DAR 16:9], 30 fps",
        );
        assert_eq!(dims, Some((1920, 1080)));
        assert_eq!(parse_dims("Audio: aac"), None);
    }

    #[test]
    fn estimates_size_and_dimensions() {
        let info = Info {
            duration: Some(60.0),
            has_audio: true,
            has_video: true,
        };
        let small = Preset {
            max_size_mb: 8.0,
            ..Preset::default()
        };
        assert_eq!(
            estimate(&small, 100 << 20, &info, None).as_deref(),
            Some("~7.2 MB")
        );
        assert_eq!(estimate(&small, 4 << 20, &info, None), None);
        let half = Preset {
            scale_pct: 50,
            ..Preset::default()
        };
        assert_eq!(
            estimate(&half, 0, &info, Some((1920, 1080))).as_deref(),
            Some("~960×540")
        );
        let cap = Preset {
            max_dim: 1280,
            ..Preset::default()
        };
        assert_eq!(estimate(&cap, 0, &info, Some((640, 480))), None);
    }

    #[test]
    fn replaces_only_with_real_output() {
        let dir = env::temp_dir().join(format!("cc-rep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (src, out, empty) = (dir.join("a.png"), dir.join("a.jpg"), dir.join("e.jpg"));
        fs::write(&out, b"x").unwrap();
        fs::write(&empty, b"").unwrap();
        let src = [src];
        assert!(replaceable(&src, std::slice::from_ref(&out)));
        assert!(!replaceable(&src, &[empty]));
        assert!(!replaceable(&src, &[dir.join("missing.jpg")]));
        assert!(!replaceable(&src, &[]));
        assert!(!replaceable(
            std::slice::from_ref(&out),
            std::slice::from_ref(&out)
        ));
        fs::remove_dir_all(dir).unwrap();
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

    fn args_for(p: &Preset, fmt: &str, category: Category, duration: Option<f64>) -> Vec<String> {
        let info = Info {
            duration,
            has_audio: true,
            has_video: true,
        };
        build_args(
            Path::new("in.mp4"),
            Path::new("out"),
            fmt,
            category,
            p,
            &info,
            None,
        )
    }

    fn has(a: &[String], pair: [&str; 2]) -> bool {
        a.windows(2).any(|w| w == pair)
    }

    #[test]
    fn builds_video_operation_args() {
        let trim = Preset {
            trim_start: 5.0,
            trim_end: 35.0,
            ..Preset::default()
        };
        let a = args_for(&trim, "mp4", Category::Video, Some(100.0));
        assert!(has(&a, ["-ss", "5"]) && has(&a, ["-to", "35"]));
        assert_eq!(trim_span(&trim, Some(100.0)), Some(30.0));
        assert_eq!(trim_span(&trim, Some(20.0)), Some(15.0));
        assert_eq!(trim_span(&Preset::default(), Some(20.0)), Some(20.0));
        assert!(has_ops(&trim));

        let speed = Preset {
            op: "speed2".into(),
            ..Preset::default()
        };
        let a = args_for(&speed, "mp4", Category::Video, Some(10.0));
        assert!(has(&a, ["-af", "atempo=2"]));
        assert!(a.iter().any(|x| x.starts_with("setpts=PTS/2")));

        let thumb = Preset {
            op: "thumb".into(),
            ..Preset::default()
        };
        let a = args_for(&thumb, "jpg", Category::Video, Some(100.0));
        assert!(has(&a, ["-ss", "10.00"]) && has(&a, ["-frames:v", "1"]));

        let sheet = Preset {
            op: "sheet".into(),
            ..Preset::default()
        };
        let a = args_for(&sheet, "jpg", Category::Video, Some(60.0));
        assert!(a
            .iter()
            .any(|x| x.contains("fps=0.20000,scale=320:-2,tile=4x3")));

        let a = args_for(&Preset::default(), "webp", Category::Video, Some(5.0));
        assert!(has(&a, ["-c:v", "libwebp_anim"]) && a.contains(&"-an".to_string()));
        assert!(a.iter().any(|x| x.contains("fps=15")));
    }

    #[test]
    fn picks_hdr_chain_by_available_filters() {
        assert!(hdr_chain(" TSC zscale  V->V").starts_with("zscale="));
        assert!(hdr_chain(" ... tonemapx  V->V").starts_with("tonemapx="));
        assert!(hdr_chain("").starts_with("colorspace="));
    }

    #[test]
    fn finds_text_subtitles_and_stream_signatures() {
        let text = "  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709), 1920x1080 [SAR 1:1 DAR 16:9], 30 fps, 30 tbr\n  Stream #0:1(eng): Audio: aac (LC), 48000 Hz, stereo, fltp\n  Stream #0:2: Subtitle: hdmv_pgs_subtitle\n  Stream #0:3: Subtitle: subrip\n";
        assert_eq!(text_subtitle_index(text), Some(1));
        assert_eq!(text_subtitle_index("Stream #0:0: Video: h264"), None);
        assert_eq!(
            stream_sig(text),
            vec!["Video h264 1920x1080 30  ", "Audio aac   48000 stereo"]
        );
    }

    #[test]
    fn builds_concat_inputs() {
        let files = [PathBuf::from(r"C:\a b\it's.png"), PathBuf::from("b.png")];
        assert_eq!(
            concat_list(&files, None),
            "file 'C:/a b/it'\\''s.png'\nfile 'b.png'\n"
        );
        assert_eq!(
            concat_list(&files, Some(3)),
            "file 'C:/a b/it'\\''s.png'\nduration 3\nfile 'b.png'\nduration 3\nfile 'b.png'\n"
        );
        let a = slideshow_args(Path::new("l.txt"), Path::new("o.mp4"));
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert_eq!(a[i - 4..i], ["-f", "concat", "-safe", "0"]);
    }

    fn exif_with_gps() -> Vec<u8> {
        let mut t = b"II*\0\x08\0\0\0".to_vec();
        t.extend([2, 0]);
        t.extend([0x0F, 0x01, 2, 0, 3, 0, 0, 0, b'A', b'B', 0, 0]);
        t.extend([0x25, 0x88, 4, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
        t.extend([0, 0, 0, 0]);
        assert_eq!(t.len(), 38);
        t.extend([2, 0]);
        t.extend([2, 0, 5, 0, 3, 0, 0, 0, 68, 0, 0, 0]);
        t.extend([1, 0, 2, 0, 2, 0, 0, 0, b'N', 0, 0, 0]);
        t.extend([0, 0, 0, 0]);
        assert_eq!(t.len(), 68);
        t.extend((0..24u8).map(|i| 0xA0 + i));
        t
    }

    #[test]
    fn strips_only_gps_from_jpeg_and_png() {
        let tiff = exif_with_gps();
        let mut jpg = b"\xFF\xD8\xFF\xE1".to_vec();
        jpg.extend(((tiff.len() + 8) as u16).to_be_bytes());
        jpg.extend(b"Exif\0\0");
        jpg.extend(&tiff);
        jpg.extend(b"\xFF\xDA\0\x02");
        strip_gps(&mut jpg).unwrap();
        let t = &jpg[12..12 + tiff.len()];
        assert_eq!(&t[8..10], [1, 0]);
        assert_eq!(&t[10..16], [0x0F, 0x01, 2, 0, 3, 0]);
        assert!(t[22..].iter().all(|b| *b == 0));
        assert!(!jpg.contains(&0xA5) && !jpg.contains(&b'N'));

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend((tiff.len() as u32).to_be_bytes());
        png.extend(b"eXIf");
        png.extend(&tiff);
        png.extend(crc32(&[b"eXIf".as_slice(), &tiff].concat()).to_be_bytes());
        strip_gps(&mut png).unwrap();
        let end = 16 + tiff.len();
        assert_eq!(
            u32::from_be_bytes(png[end..end + 4].try_into().unwrap()),
            crc32(&png[12..end])
        );
        assert!(!png.contains(&0xA5));
        assert!(strip_gps(&mut b"GIF89a".to_vec()).is_err());
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn ico_container_lists_every_size() {
        let ico = ico_from_pngs(&[(16, vec![1; 5]), (256, vec![2; 7])]);
        assert_eq!(&ico[..6], [0, 0, 1, 0, 2, 0]);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[22], 0);
        assert_eq!(u32::from_le_bytes(ico[14..18].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 38);
        assert_eq!(u32::from_le_bytes(ico[34..38].try_into().unwrap()), 43);
        assert_eq!(ico.len(), 50);
    }

    #[test]
    fn builds_image_and_audio_edit_args() {
        let crop = Preset {
            crop: "16:9".into(),
            grayscale: true,
            ..Preset::default()
        };
        let vf = image_filters(&crop);
        assert!(vf[0].starts_with("crop='if(gte(iw/ih,16/9)") && vf[1] == "hue=s=0");
        assert_eq!(parse_ratio("x"), None);
        assert_eq!(parse_ratio("0:1"), None);
        let p = Preset {
            cut_start: 5.0,
            cut_len: 30.0,
            fade_s: 2.0,
            speed: 1.5,
            ..Preset::default()
        };
        let (head, tail) = audio_edit_filters(&p, Some(100.0));
        assert_eq!(head[0], "atrim=start=5:duration=30");
        assert_eq!(
            tail,
            [
                "afade=t=in:d=2.000",
                "afade=t=out:st=28.000:d=2.000",
                "atempo=1.5"
            ]
        );
        let info = Info {
            duration: Some(100.0),
            has_audio: true,
            has_video: false,
        };
        let a = build_args(
            Path::new("in.mp3"),
            Path::new("o.m4r"),
            "m4r",
            Category::Audio,
            &p,
            &info,
            None,
        );
        assert!(a.windows(2).any(|w| w == ["-t", "40"]));
        assert!(a.windows(2).any(|w| w == ["-f", "ipod"]));
        assert!(a.iter().any(|x| x.starts_with("atrim=")));
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
        assert!(command(ffmpeg())
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=320x240:d=1"
            ])
            .args([
                "-vf",
                "setparams=colorspace=bt2020nc:color_primaries=bt2020:color_trc=smpte2084,format=yuv420p10le",
                "-c:v",
                "libx265",
                "-x265-params",
                "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:log-level=error",
            ])
            .arg(dir.join("hdr.mp4"))
            .status()
            .unwrap()
            .success());
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
            (
                src.clone(),
                Preset {
                    trim_start: 0.5,
                    trim_end: 1.5,
                    ..Preset::named("Trim", Category::Video)
                },
            ),
            (src.clone(), Preset::convert_to("webp")),
            (
                src.clone(),
                Preset {
                    op: "speed2".into(),
                    ..Preset::named("2x speed", Category::Video)
                },
            ),
            (
                dir.join("hdr.mp4"),
                Preset {
                    op: "hdr".into(),
                    ..Preset::named("HDR to SDR", Category::Video)
                },
            ),
            (
                src.clone(),
                Preset {
                    op: "thumb".into(),
                    format: "jpg".into(),
                    ..Preset::named("Thumbnail", Category::Video)
                },
            ),
            (
                src.clone(),
                Preset {
                    op: "sheet".into(),
                    format: "jpg".into(),
                    ..Preset::named("Contact sheet", Category::Video)
                },
            ),
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
                    parallel: 1,
                    replace_original: false,
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
        let srt = dir.join("subs.srt");
        fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nhello\n").unwrap();
        let withsubs = dir.join("withsubs.mp4");
        assert!(command(ffmpeg())
            .args(["-loglevel", "error", "-i"])
            .arg(&src)
            .arg("-i")
            .arg(&srt)
            .args(["-c", "copy", "-c:s", "mov_text"])
            .arg(&withsubs)
            .status()
            .unwrap()
            .success());
        let png2 = dir.join("pic2.png");
        fs::copy(&png, &png2).unwrap();
        let src2 = dir.join("clip2.mp4");
        fs::copy(&src, &src2).unwrap();
        let odd = dir.join("odd.mp4");
        assert!(command(ffmpeg())
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=160x120:d=1"
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&odd)
            .status()
            .unwrap()
            .success());
        let wav1 = dir.join("a.wav");
        let wav2 = dir.join("b.wav");
        for w in [&wav1, &wav2] {
            assert!(command(ffmpeg())
                .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=d=1"])
                .arg(w)
                .status()
                .unwrap()
                .success());
        }
        let join = |name: &str, files: Vec<PathBuf>, preset: Preset| {
            let mut outcome = None;
            run(
                Job {
                    files,
                    preset,
                    out_dir: None,
                    hardware: false,
                    parallel: 1,
                    replace_original: false,
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
        };
        let op = |name: &str, op: &str, category| Preset {
            op: op.into(),
            ..Preset::named(name, category)
        };
        join("srt", vec![withsubs], Preset::convert_to("srt"));
        join(
            "join copy",
            vec![src.clone(), src2.clone()],
            op("Join", "join", Category::Video),
        );
        join(
            "join reencode",
            vec![src.clone(), odd],
            op("Join", "join", Category::Video),
        );
        join(
            "join audio",
            vec![wav1, wav2],
            op("Join", "join", Category::Audio),
        );
        join(
            "slideshow",
            vec![png, png2],
            Preset {
                format: "mp4".into(),
                ..op("Slideshow", "slideshow", Category::Image)
            },
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
