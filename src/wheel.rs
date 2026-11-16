use crate::{
    app,
    config::{Category, Config, OutputMode, Preset, WheelKind},
    convert::{self, Event, Job},
    formats, hook, import,
    render::{self, CenterIcon, Fonts, Hit, Scene, SliceView, Status},
    sys::{self, Sound},
};
use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tiny_skia::Pixmap;
use windows::{
    core::w,
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
            GetStockObject, MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER,
            BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLACK_BRUSH, BLENDFUNCTION, DIB_RGB_COLORS,
            HBITMAP, HBRUSH, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::{
            Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
            LibraryLoader::GetModuleHandleW,
            Ole::{
                IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
                CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT},
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, KillTimer,
                RegisterClassExW, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow,
                UpdateLayeredWindow, HWND_NOTOPMOST, LWA_ALPHA, MA_NOACTIVATE, SWP_NOACTIVATE,
                SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_LBUTTONUP,
                WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
};
use windows_core::{implement, Ref};

const WM_MOUSELEAVE: u32 = 0x02A3;
const OPEN_S: f32 = 0.22;
const CLOSE_S: f32 = 0.18;
const MORPH_S: f32 = 0.34;
const TOAST_AFTER: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Hidden,
    Drag,
    Selecting,
    Status,
    Closing,
}

struct Catcher(HWND);

impl Drop for Catcher {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.0);
            let _ = DestroyWindow(self.0);
        }
    }
}

impl Catcher {
    fn create() -> Option<Self> {
        unsafe {
            let instance = GetModuleHandleW(None).ok()?.into();
            static REGISTERED: std::sync::Once = std::sync::Once::new();
            REGISTERED.call_once(|| {
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(catcher_proc),
                    hInstance: instance,
                    hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    lpszClassName: w!("CrystalConverterCatcher"),
                    ..Default::default()
                };
                RegisterClassExW(&class);
            });
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let r = info.rcMonitor;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                w!("CrystalConverterCatcher"),
                w!(""),
                WS_POPUP,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                None,
                None,
                Some(instance),
                None,
            )
            .ok()?;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);
            let target: IDropTarget = DropTarget.into();
            let _ = RegisterDragDrop(hwnd, &target);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(Self(hwnd))
        }
    }
}

unsafe extern "system" fn catcher_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_LBUTTONUP | WM_RBUTTONUP => {
            app::with_wheel(Wheel::outside_click);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

struct Window {
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pixmap: Pixmap,
    base: [Option<Pixmap>; 2],
    fonts: Fonts,
    scale: f32,
    pos: POINT,
    size: i32,
    tracking: bool,
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            let _ = KillTimer(Some(self.hwnd), 1);
            let _ = DestroyWindow(self.hwnd);
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Running {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Batch {
    queue: VecDeque<Job>,
    chain_at: Option<Instant>,
    job_at: Option<Instant>,
    total: usize,
    failed: Vec<String>,
    outputs: Vec<PathBuf>,
    stats: String,
}

struct Meta {
    size: u64,
    info: convert::Info,
    dims: Option<(u32, u32)>,
}

pub struct Wheel {
    host: isize,
    catcher: Option<Catcher>,
    win: Option<Window>,
    config: Config,
    kind: WheelKind,
    phase: Phase,
    files: Vec<PathBuf>,
    category: Option<Category>,
    slices: Vec<Preset>,
    slice_hover: Vec<f32>,
    slices_at: Option<Instant>,
    center_hover: f32,
    hover: Hit,
    opened: Instant,
    last_tick: Instant,
    job: Option<Running>,
    batch: Batch,
    error: String,
    meta: Option<(PathBuf, Arc<Mutex<Option<Meta>>>)>,
    est_shown: Option<String>,
    status: Status,
    status_at: Instant,
    morph_at: Option<Instant>,
    status_title: String,
    status_sub: String,
    outputs: Vec<PathBuf>,
    button_hover: [f32; 2],
    inside: bool,
    close_at: Option<Instant>,
    closing_at: Option<Instant>,
    dirty: bool,
    quick: Option<Preset>,
}

impl Wheel {
    pub fn new(host: HWND) -> Self {
        let now = Instant::now();
        Self {
            host: host.0 as isize,
            catcher: None,
            win: None,
            config: Config::default(),
            kind: WheelKind::Convert,
            phase: Phase::Hidden,
            files: Vec::new(),
            category: None,
            slices: Vec::new(),
            slice_hover: Vec::new(),
            slices_at: None,
            center_hover: 0.0,
            hover: Hit::None,
            opened: now,
            last_tick: now,
            job: None,
            batch: Batch::default(),
            error: String::new(),
            meta: None,
            est_shown: None,
            status: Status::Preparing,
            status_at: now,
            morph_at: None,
            status_title: String::new(),
            status_sub: String::new(),
            outputs: Vec::new(),
            button_hover: [0.0; 2],
            inside: false,
            close_at: None,
            closing_at: None,
            dirty: true,
            quick: None,
        }
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Drag | Phase::Selecting | Phase::Status)
    }

    pub fn show(&mut self, kind: WheelKind, config: &Config) {
        let running = self.phase == Phase::Status && self.job.is_some();
        if self.busy() && !running {
            return;
        }
        let host = HWND(self.host as _);
        let keep = running.then(|| {
            (
                self.job.take(),
                std::mem::take(&mut self.batch),
                self.status,
                self.status_at,
                std::mem::take(&mut self.status_title),
                std::mem::take(&mut self.status_sub),
            )
        });
        *self = Self::new(host);
        if let Some((job, batch, status, at, title, sub)) = keep {
            self.job = job;
            self.batch = batch;
            self.status = status;
            self.status_at = at;
            self.status_title = title;
            self.status_sub = sub;
        }
        self.kind = kind;
        self.config = config.clone();
        self.phase = Phase::Drag;
        self.catcher = Catcher::create();
        self.win = Window::create();
        if self.win.is_none() {
            self.catcher = None;
            self.phase = Phase::Hidden;
            return;
        }
        hook::ESCAPE_ACTIVE.store(true, Ordering::Relaxed);
        self.tick();
    }

    pub fn show_preset(&mut self, index: usize, config: &Config) {
        let Some(preset) = config.presets.get(index).cloned() else {
            return;
        };
        if self.busy() {
            return;
        }
        self.show(preset.wheel, config);
        if self.phase == Phase::Drag {
            self.quick = Some(preset);
        }
    }

    pub fn open_files(&mut self, files: Vec<PathBuf>, config: &Config) {
        if self.phase == Phase::Selecting {
            self.set_files(files.into_iter().chain(self.files.clone()).collect());
            return;
        }
        self.show(WheelKind::Convert, config);
        if self.phase == Phase::Drag {
            self.set_files(files);
            self.phase = Phase::Selecting;
        }
    }

    pub fn hotkey_up(&mut self) {
        if self.phase == Phase::Drag {
            self.close();
        }
    }

    pub fn escape(&mut self) {
        match self.phase {
            Phase::Drag | Phase::Selecting => self.close(),
            Phase::Status => {
                if let Some(job) = &self.job {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                self.close();
            }
            _ => {}
        }
    }

    fn close(&mut self) {
        if matches!(self.phase, Phase::Hidden | Phase::Closing) {
            return;
        }
        if self.job.is_some() && matches!(self.phase, Phase::Drag | Phase::Selecting) {
            self.phase = Phase::Status;
            self.morph_at = Some(Instant::now());
            self.catcher = None;
            self.hover = Hit::None;
            self.dirty = true;
            return;
        }
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.batch.queue.clear();
        self.phase = Phase::Closing;
        self.closing_at = Some(Instant::now());
        self.catcher = None;
    }

    fn outside_click(&mut self) {
        if matches!(self.phase, Phase::Drag | Phase::Selecting) {
            self.close();
        }
    }

    fn set_files(&mut self, files: Vec<PathBuf>) {
        self.dirty = true;
        let files = expand(files);
        let mut counts: HashMap<Category, usize> = HashMap::new();
        let detected: Vec<(PathBuf, Option<Category>)> = files
            .into_iter()
            .map(|f| {
                let c = formats::detect(&f);
                (f, c)
            })
            .collect();
        for (_, c) in &detected {
            if let Some(c) = c {
                *counts.entry(*c).or_default() += 1;
            }
        }
        let category = detected
            .iter()
            .filter_map(|(_, c)| *c)
            .max_by_key(|c| counts[c]);
        self.files = detected
            .into_iter()
            .filter(|(_, c)| *c == category && c.is_some())
            .map(|(f, _)| f)
            .collect();
        self.category = category;
        let ext = self
            .files
            .first()
            .map(|f| formats::ext_of(f))
            .unwrap_or_default();
        self.slices = match category {
            Some(c) => formats::slices(
                self.kind,
                c,
                &ext,
                convert::soffice().is_some(),
                &self.config.presets,
                self.files.len(),
            ),
            None => Vec::new(),
        };
        if let Some(p) = &self.quick {
            self.slices = match category {
                Some(c) if p.matches(c) => vec![p.clone()],
                _ => Vec::new(),
            };
        }
        self.slice_hover = vec![0.0; self.slices.len()];
        self.slices_at = Some(Instant::now());
        let first = self.files.first().cloned();
        if self.meta.as_ref().map(|m| &m.0) != first.as_ref() {
            self.meta = first
                .filter(|_| {
                    matches!(
                        category,
                        Some(Category::Image | Category::Video | Category::Audio)
                    )
                })
                .map(|path| {
                    let slot = Arc::new(Mutex::new(None));
                    let (out, file) = (slot.clone(), path.clone());
                    std::thread::spawn(move || {
                        if let Ok((info, dims)) = convert::probe_meta(&file) {
                            let size = std::fs::metadata(&file).map_or(0, |m| m.len());
                            *out.lock().unwrap() = Some(Meta { size, info, dims });
                        }
                    });
                    (path, slot)
                });
        }
    }

    fn local(&self, x: i32, y: i32) -> (f32, f32) {
        match &self.win {
            Some(w) => ((x - w.pos.x) as f32, (y - w.pos.y) as f32),
            None => (-1.0, -1.0),
        }
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        let scale = self.win.as_ref().map_or(1.0, |w| w.scale);
        render::hit_wheel(scale, self.slices.len(), x, y)
    }

    fn drag_enter(&mut self, files: Vec<PathBuf>, pt: (i32, i32)) -> bool {
        if !matches!(self.phase, Phase::Drag | Phase::Selecting) || files.is_empty() {
            return false;
        }
        if self.files.is_empty() || self.phase == Phase::Drag {
            self.set_files(files);
        }
        self.drag_over(pt);
        !self.files.is_empty()
    }

    fn drag_over(&mut self, pt: (i32, i32)) {
        let (x, y) = self.local(pt.0, pt.1);
        self.hover = self.hit(x, y);
    }

    fn drag_leave(&mut self) {
        self.dirty = true;
        self.hover = Hit::None;
    }

    fn drop_files(&mut self, files: Vec<PathBuf>, pt: (i32, i32)) -> bool {
        if !matches!(self.phase, Phase::Drag | Phase::Selecting) {
            return false;
        }
        if !files.is_empty() {
            self.set_files(files);
        }
        if self.files.is_empty() {
            self.close();
            return false;
        }
        if self.quick.is_some() {
            if self.slices.is_empty() {
                self.close();
                return false;
            }
            self.start(0);
            return true;
        }
        let (x, y) = self.local(pt.0, pt.1);
        match self.hit(x, y) {
            Hit::Slice(i) => self.start(i),
            Hit::Center => {
                self.phase = Phase::Selecting;
                self.hover = Hit::None;
            }
            _ => {
                self.close();
                return false;
            }
        }
        true
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        if let Some(w) = &mut self.win {
            if !w.tracking {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: w.hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
                w.tracking = true;
            }
        }
        match self.phase {
            Phase::Selecting => self.hover = self.hit(x, y),
            Phase::Status => {
                let scale = self.win.as_ref().map_or(1.0, |w| w.scale);
                self.hover = render::hit_button(scale, x, y);
                self.inside = self.hover != Hit::None;
            }
            _ => {}
        }
    }

    fn mouse_leave(&mut self) {
        self.dirty = true;
        if let Some(w) = &mut self.win {
            w.tracking = false;
        }
        if self.phase != Phase::Drag {
            self.hover = Hit::None;
        }
        if self.inside {
            self.inside = false;
            if let Some(at) = self.close_at {
                self.close_at = Some(at.max(Instant::now() + Duration::from_millis(600)));
            }
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        self.dirty = true;
        match self.phase {
            Phase::Selecting => match self.hit(x, y) {
                Hit::Slice(i) => self.start(i),
                Hit::Center => self.close(),
                _ => {}
            },
            Phase::Status if self.status == Status::Done => {
                let scale = self.win.as_ref().map_or(1.0, |w| w.scale);
                match render::hit_button(scale, x, y) {
                    Hit::Button(0) => {
                        if let Some(first) = self.outputs.first() {
                            sys::reveal(first);
                        }
                        self.close();
                    }
                    Hit::Button(_) => {
                        if let Some(w) = &self.win {
                            if sys::copy_files(w.hwnd, &self.outputs) {
                                self.status_sub = "Copied to clipboard".into();
                                self.close_at = Some(Instant::now() + Duration::from_millis(900));
                            }
                        }
                    }
                    _ => self.close(),
                }
            }
            Phase::Status if self.status == Status::Error => {
                let scale = self.win.as_ref().map_or(1.0, |w| w.scale);
                match render::hit_button(scale, x, y) {
                    Hit::Button(0) => {
                        if let Some(w) = &self.win {
                            if sys::copy_text(w.hwnd, &self.error) {
                                self.status_sub = "Copied error".into();
                                self.close_at = Some(Instant::now() + Duration::from_millis(900));
                            }
                        }
                    }
                    Hit::Button(_) => {
                        sys::open(&log_path().to_string_lossy());
                        self.close();
                    }
                    _ => self.close(),
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, index: usize) {
        self.dirty = true;
        self.quick = None;
        let Some(preset) = self.slices.get(index).cloned() else {
            return;
        };
        let now = Instant::now();
        let running = self.job.is_some();
        self.catcher = None;
        self.phase = Phase::Status;
        self.morph_at = Some(now);
        self.hover = Hit::None;
        if !running {
            self.status = Status::Preparing;
            self.status_at = now;
            self.status_title = "Preparing".into();
            self.status_sub = match self.files.len() {
                1 => convert::file_name(&self.files[0]),
                n => format!("{n} files"),
            };
            if self.config.sounds {
                sys::play(Sound::Start);
            }
        }
        if !self.config.always_on_top {
            if let Some(w) = &self.win {
                unsafe {
                    let _ = SetWindowPos(
                        w.hwnd,
                        Some(HWND_NOTOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        let out_dir = (self.config.output_mode == OutputMode::Folder
            && !self.config.output_folder.trim().is_empty())
        .then(|| PathBuf::from(self.config.output_folder.trim()))
        .or_else(|| {
            self.files
                .iter()
                .any(|f| import::is_staged(f))
                .then(import::pictures_dir)
                .flatten()
        });
        let job = Job {
            files: self.files.clone(),
            preset,
            out_dir,
            hardware: self.config.hardware_encoding,
            parallel: self.config.parallel_jobs as usize,
            replace_original: self.config.replace_original,
        };
        if running {
            self.batch.queue.push_back(job);
            self.status_title = self.title();
        } else {
            self.spawn(job);
        }
    }

    fn title(&self) -> String {
        let base = if matches!(self.status, Status::Converting(_)) {
            "Converting"
        } else {
            "Preparing"
        };
        match self.batch.queue.len() {
            0 => base.into(),
            n => format!("{base} · {n} waiting"),
        }
    }

    fn spawn(&mut self, job: Job) {
        let now = Instant::now();
        self.batch.chain_at.get_or_insert(now);
        self.batch.job_at = Some(now);
        self.batch.stats.clear();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let host = self.host;
        std::thread::spawn(move || {
            let _awake = sys::keep_awake();
            convert::run(job, &flag, &mut |e| {
                let _ = tx.send(e);
                app::post(host, app::WM_JOB);
            })
        });
        self.job = Some(Running { rx, cancel });
    }

    fn advance(&mut self) -> Option<Vec<PathBuf>> {
        if let Some(job) = self.batch.queue.pop_front() {
            self.status = Status::Preparing;
            self.spawn(job);
            self.status_title = self.title();
            return None;
        }
        self.finish()
    }

    fn finish(&mut self) -> Option<Vec<PathBuf>> {
        let batch = std::mem::take(&mut self.batch);
        self.job = None;
        let now = Instant::now();
        let long = batch.chain_at.is_some_and(|t| t.elapsed() >= TOAST_AFTER);
        let notify = long && (self.phase != Phase::Status || !self.config.always_on_top);
        let failed = batch.failed.len();
        let message = batch.failed.join("\n");
        if !message.is_empty() {
            log_error(&message);
            self.error = message.clone();
        }
        let delay = u64::from(self.config.close_delay_ms);
        if batch.outputs.is_empty() {
            self.status = Status::Error;
            self.status_at = now;
            self.status_title = if failed > 1 {
                format!("0 of {failed} done, {failed} failed")
            } else {
                "Failed".into()
            };
            self.status_sub = message.lines().next().unwrap_or("").to_string();
            if self.config.sounds {
                sys::play(Sound::Error);
            }
            self.close_at = Some(now + Duration::from_millis(delay.max(4000)));
            if notify {
                sys::toast(&self.status_title, &self.status_sub);
            }
            return None;
        }
        self.status = Status::Done;
        self.status_at = now;
        self.status_title = if failed == 0 {
            "Done".into()
        } else {
            format!(
                "{} of {} done, {failed} failed",
                batch.total.saturating_sub(failed),
                batch.total
            )
        };
        self.status_sub = match batch.outputs.len() {
            1 => convert::file_name(&batch.outputs[0]),
            n => format!("{n} files"),
        };
        if self.config.copy_to_clipboard {
            if let Some(w) = &self.win {
                if sys::copy_files(w.hwnd, &batch.outputs) {
                    self.status_sub = format!("{} · copied", self.status_sub);
                }
            }
        }
        if self.config.sounds {
            sys::play(Sound::Done);
        }
        let settle = self.morph_at.map_or(Duration::ZERO, |m| {
            Duration::from_secs_f32(MORPH_S).saturating_sub(m.elapsed())
        });
        self.close_at = Some(now + settle + Duration::from_millis(delay));
        if notify {
            sys::toast(&self.status_title, &self.status_sub);
        }
        self.outputs = batch.outputs.clone();
        Some(batch.outputs)
    }

    pub fn on_job(&mut self, _config: &Config) -> Option<Vec<PathBuf>> {
        self.dirty = true;
        let events: Vec<Event> = self
            .job
            .as_ref()
            .map(|j| j.rx.try_iter().collect())
            .unwrap_or_default();
        let mut finished = None;
        for event in events {
            if matches!(self.phase, Phase::Hidden | Phase::Closing) {
                break;
            }
            match event {
                Event::Progress {
                    index,
                    total,
                    fraction,
                    name,
                    speed,
                } => {
                    self.status = Status::Converting(fraction);
                    self.status_title = self.title();
                    self.batch.stats = self
                        .batch
                        .job_at
                        .map_or_else(String::new, |t| stats_line(t.elapsed(), fraction, speed));
                    self.status_sub = if total > 1 {
                        format!("{} of {total} · {name}", index + 1)
                    } else {
                        name
                    };
                }
                Event::Done {
                    outputs,
                    failed,
                    total,
                } => {
                    self.batch.total += total;
                    self.batch.failed.extend(failed);
                    self.batch.outputs.extend(outputs);
                    finished = self.advance();
                    break;
                }
                Event::Failed(msg) => {
                    let lines: Vec<String> = msg.lines().map(String::from).collect();
                    self.batch.total += lines.len();
                    self.batch.failed.extend(lines);
                    finished = self.advance();
                    break;
                }
                Event::Cancelled => {
                    self.job = None;
                    self.batch = Batch::default();
                    self.close();
                    break;
                }
            }
        }
        finished
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        let follow = 1.0 - (-dt * 30.0).exp();
        let mut moving = false;
        let mut ease = |h: &mut f32, target: f32| {
            *h += (target - *h) * follow;
            if (target - *h).abs() < 0.004 {
                *h = target;
            } else {
                moving = true;
            }
        };

        let open = match (self.phase, self.closing_at) {
            (Phase::Closing, Some(at)) => {
                let t = at.elapsed().as_secs_f32() / CLOSE_S;
                if t >= 1.0 {
                    self.win = None;
                    self.phase = Phase::Hidden;
                    self.job = None;
                    hook::ESCAPE_ACTIVE.store(false, Ordering::Relaxed);
                    return;
                }
                1.0 - render::ease_out_cubic(t)
            }
            _ => (self.opened.elapsed().as_secs_f32() / OPEN_S).min(1.0),
        };
        if self.phase == Phase::Status && !self.inside && self.close_at.is_some_and(|at| now >= at)
        {
            self.close();
        }

        for (i, h) in self.slice_hover.iter_mut().enumerate() {
            let target = if self.hover == Hit::Slice(i) {
                1.0
            } else {
                0.0
            };
            ease(h, target);
        }
        let target = if self.hover == Hit::Center { 1.0 } else { 0.0 };
        ease(&mut self.center_hover, target);
        for (i, h) in self.button_hover.iter_mut().enumerate() {
            let target = if self.hover == Hit::Button(i) {
                1.0
            } else {
                0.0
            };
            ease(h, target);
        }

        let since_slices = self.slices_at.map_or(0.0, |t| t.elapsed().as_secs_f32());
        let labels: Vec<String> = self.slices.iter().map(|p| p.name.clone()).collect();
        let views: Vec<SliceView> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| SliceView {
                label,
                hover: self.slice_hover.get(i).copied().unwrap_or(0.0),
                appear: ((since_slices - i as f32 * 0.025) / 0.22).clamp(0.0, 1.0),
            })
            .collect();
        let kind_label = match self.kind {
            WheelKind::Convert => "Convert",
            WheelKind::Modify => "Modify",
        };
        let (title, subtitle) = match (self.phase, self.category) {
            (_, None) if self.files.is_empty() => ("Drop here".to_string(), kind_label.to_string()),
            (_, None) => ("Unsupported".into(), "file type".into()),
            (_, Some(Category::Document)) if self.slices.is_empty() => {
                ("Documents".into(), "need LibreOffice".into())
            }
            (_, Some(_)) if self.slices.is_empty() => ("Nothing to do".into(), kind_label.into()),
            (Phase::Selecting, Some(_)) => (
                if self.kind == WheelKind::Convert {
                    "Pick a format".into()
                } else {
                    "Pick an action".into()
                },
                file_summary(&self.files),
            ),
            (_, Some(c)) => (c.label().to_string(), file_summary(&self.files)),
        };
        let estimate = match self.hover {
            Hit::Slice(i) if self.files.len() == 1 => self.slices.get(i).and_then(|p| {
                let slot = self.meta.as_ref()?.1.lock().ok()?;
                let m = slot.as_ref()?;
                convert::estimate(p, m.size, &m.info, m.dims)
            }),
            _ => None,
        };
        if estimate != self.est_shown {
            self.est_shown.clone_from(&estimate);
            self.dirty = true;
        }
        let subtitle = estimate.unwrap_or(subtitle);
        let morph = self.morph_at.map_or(0.0, |t| {
            render::ease_in_out_cubic(t.elapsed().as_secs_f32() / MORPH_S)
        });
        let accent = if self.kind == WheelKind::Convert {
            render::CONVERT_ACCENT
        } else {
            render::MODIFY_ACCENT
        };
        let tips = if self.status == Status::Error {
            render::ERROR_TIPS
        } else {
            render::BUTTON_TIPS
        };
        let tooltip = match self.hover {
            Hit::Button(i) if matches!(self.status, Status::Done | Status::Error) => {
                Some((i, tips[i]))
            }
            _ => self
                .button_hover
                .iter()
                .position(|h| *h > 0.02)
                .map(|i| (i, tips[i])),
        };
        let status_t = self.status_at.elapsed().as_secs_f32();
        let animating = moving
            || self.dirty
            || open < 1.0
            || self.phase == Phase::Closing
            || (morph > 0.0 && morph < 1.0)
            || since_slices < 0.22 + 0.025 * self.slices.len() as f32
            || (self.morph_at.is_some()
                && matches!(self.status, Status::Preparing | Status::Converting(_)))
            || (self.morph_at.is_some() && status_t < 1.0);
        if !animating || self.quick.is_some() {
            return;
        }
        self.dirty = false;
        let Some(win) = &mut self.win else { return };
        let scene = Scene {
            scale: win.scale,
            open,
            accent,
            slices: &views,
            center_hover: self.center_hover,
            center_icon: if self.phase == Phase::Selecting {
                CenterIcon::Cancel
            } else {
                CenterIcon::Drop
            },
            title: if self.morph_at.is_some() {
                &self.status_title
            } else {
                &title
            },
            subtitle: if self.morph_at.is_some() {
                &self.status_sub
            } else {
                &subtitle
            },
            morph,
            status: self.morph_at.map(|_| self.status),
            status_t,
            time: self.opened.elapsed().as_secs_f32(),
            button_hover: self.button_hover,
            tooltip,
            stats: &self.batch.stats,
        };
        let base = win.base[usize::from(morph >= 1.0)].as_ref();
        render::draw(&mut win.pixmap, &scene, &win.fonts, base);
        win.present();
    }
}

fn file_summary(files: &[PathBuf]) -> String {
    match files {
        [] => String::new(),
        [one] => formats::ext_of(one).to_uppercase(),
        many => format!("{} files", many.len()),
    }
}

fn expand(files: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for f in files {
        if f.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&f) {
                let mut inner: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_file())
                    .collect();
                inner.sort();
                out.extend(inner);
            }
        } else if f.is_file() {
            out.push(f);
        }
    }
    out
}

fn log_path() -> PathBuf {
    crate::config::dir().join("last-error.txt")
}

fn log_error(msg: &str) {
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::write(log_path(), msg);
}

fn clock(secs: f32) -> String {
    let s = secs.round() as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn stats_line(elapsed: Duration, fraction: Option<f32>, speed: Option<f32>) -> String {
    let mut parts = Vec::new();
    if let Some(f) = fraction.filter(|f| *f > 0.02 && elapsed.as_secs() >= 2) {
        parts.push(format!(
            "{} left",
            clock(elapsed.as_secs_f32() * (1.0 - f) / f)
        ));
    }
    if let Some(x) = speed {
        parts.push(format!("{x:.1}x"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_eta_and_speed() {
        let s = Duration::from_secs(10);
        assert_eq!(stats_line(s, Some(0.25), Some(1.84)), "0:30 left · 1.8x");
        assert_eq!(stats_line(s, Some(0.01), None), "");
        assert_eq!(stats_line(Duration::from_secs(1), Some(0.5), None), "");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}

impl Window {
    fn create() -> Option<Self> {
        unsafe {
            let instance = GetModuleHandleW(None).ok()?.into();
            static REGISTERED: std::sync::Once = std::sync::Once::new();
            REGISTERED.call_once(|| {
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wheel_proc),
                    hInstance: instance,
                    lpszClassName: w!("CrystalConverterWheel"),
                    ..Default::default()
                };
                RegisterClassExW(&class);
            });
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let scale = dx as f32 / 96.0;
            let size = (render::SIZE * scale).round() as i32;
            let work = info.rcWork;
            let pos = POINT {
                x: (work.left + work.right - size) / 2,
                y: (work.top + work.bottom - size) / 2,
            };
            let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
            let hwnd = CreateWindowExW(
                ex,
                w!("CrystalConverterWheel"),
                w!("Crystal Converter"),
                WS_POPUP,
                pos.x,
                pos.y,
                size,
                size,
                None,
                None,
                Some(instance),
                None,
            )
            .ok()?;
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(dc, bitmap.into());
            let target: IDropTarget = DropTarget.into();
            let _ = RegisterDragDrop(hwnd, &target);
            SetTimer(Some(hwnd), 1, 15, None);
            let win = Self {
                hwnd,
                dc,
                bitmap,
                old,
                bits: bits.cast(),
                pixmap: Pixmap::new(size as u32, size as u32)?,
                base: [
                    render::background(scale, 0.0),
                    render::background(scale, 1.0),
                ],
                fonts: Fonts::load()?,
                scale,
                pos,
                size,
                tracking: false,
            };
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(win)
        }
    }

    fn present(&mut self) {
        let src = self.pixmap.data();
        let dst = unsafe { std::slice::from_raw_parts_mut(self.bits, src.len()) };
        for (d, s) in dst
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<4>().0)
        {
            *d = [s[2], s[1], s[0], s[3]];
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE {
            cx: self.size,
            cy: self.size,
        };
        let origin = POINT::default();
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&self.pos),
                Some(&size),
                Some(self.dc),
                Some(&origin),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }
}

fn mouse_xy(lparam: LPARAM) -> (f32, f32) {
    (
        f32::from((lparam.0 & 0xFFFF) as u16 as i16),
        f32::from(((lparam.0 >> 16) & 0xFFFF) as u16 as i16),
    )
}

unsafe extern "system" fn wheel_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = match msg {
        WM_TIMER => app::with_wheel(Wheel::tick).is_some(),
        WM_MOUSEMOVE => {
            let (x, y) = mouse_xy(lparam);
            app::with_wheel(|w| w.mouse_move(x, y)).is_some()
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_xy(lparam);
            app::with_wheel(|w| w.click(x, y)).is_some()
        }
        WM_RBUTTONUP => app::with_wheel(Wheel::escape).is_some(),
        WM_MOUSELEAVE => app::with_wheel(Wheel::mouse_leave).is_some(),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        _ => false,
    };
    if handled {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

pub(crate) fn read_files(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else {
            return Vec::new();
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let files = (0..count)
            .map(|i| {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                PathBuf::from(OsString::from_wide(&buf[..len]))
            })
            .collect();
        ReleaseStgMedium(&mut medium);
        files
    }
}

fn dropped_files(data: &IDataObject) -> Vec<PathBuf> {
    let files = read_files(data);
    if files.is_empty() {
        import::from_browser(data)
    } else {
        files
    }
}

#[implement(IDropTarget)]
struct DropTarget;

fn set_effect(effect: *mut DROPEFFECT, accept: bool) {
    if !effect.is_null() {
        unsafe {
            *effect = if accept {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            }
        };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let files = data.ok().map(dropped_files).unwrap_or_default();
        let accept = app::with_wheel(|w| w.drag_enter(files, (pt.x, pt.y))).unwrap_or(false);
        set_effect(effect, accept);
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let accept = app::with_wheel(|w| {
            w.drag_over((pt.x, pt.y));
            !w.files.is_empty()
        })
        .unwrap_or(false);
        set_effect(effect, accept);
        Ok(())
    }

    fn DragLeave(&self) -> windows_core::Result<()> {
        app::with_wheel(Wheel::drag_leave);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let known = app::with_wheel(|w| !w.files.is_empty()).unwrap_or(false);
        let files = match data.ok().ok() {
            Some(d) if known => read_files(d),
            Some(d) => dropped_files(d),
            None => Vec::new(),
        };
        let accept = app::with_wheel(|w| w.drop_files(files, (pt.x, pt.y))).unwrap_or(false);
        set_effect(effect, accept);
        Ok(())
    }
}
