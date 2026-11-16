use crate::{
    config::{Config, WheelKind},
    hook, import, sys,
    tray::{Action, Tray, QUEUE},
    update,
    wheel::Wheel,
};
use std::{
    cell::RefCell, env, os::windows::ffi::OsStrExt, path::PathBuf, process::Command, time::Duration,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::COPYDATASTRUCT, LibraryLoader::GetModuleHandleW, Ole::OleInitialize,
            Threading::CreateMutexW,
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, FindWindowW,
                GetMessageW, PostMessageW, PostQuitMessage, RegisterClassExW, SendMessageW,
                SetForegroundWindow, ShowWindow, TranslateMessage, HWND_MESSAGE, MSG, SW_RESTORE,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COPYDATA, WM_HOTKEY, WNDCLASSEXW,
            },
        },
    },
};

pub const WM_HOTKEY_DOWN: u32 = WM_APP + 1;
pub const WM_HOTKEY_UP: u32 = WM_APP + 2;
pub const WM_ESCAPE: u32 = WM_APP + 3;
pub const WM_TRAY: u32 = WM_APP + 4;
pub const WM_JOB: u32 = WM_APP + 5;
pub const WM_RELOAD: u32 = WM_APP + 6;
pub const WM_UPDATE: u32 = WM_APP + 7;

const HOTKEY_SELECTION: i32 = 1;
const HOTKEY_CLIPBOARD: i32 = 2;

pub const HOST_CLASS: PCWSTR = w!("CrystalConverterHost");
pub const SETTINGS_TITLE: &str = "Crystal Converter Settings";

pub struct App {
    pub config: Config,
    host: isize,
    tray: Tray,
    wheel: Wheel,
    recent: Vec<PathBuf>,
    last: Vec<PathBuf>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

pub(crate) fn with_wheel<R>(f: impl FnOnce(&mut Wheel) -> R) -> Option<R> {
    with_app(|a| f(&mut a.wheel))
}

pub fn post(host: isize, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(host as _)), msg, WPARAM(0), LPARAM(0));
    }
}

pub fn find_host() -> Option<HWND> {
    unsafe {
        FindWindowExW(Some(HWND_MESSAGE), None, HOST_CLASS, None)
            .ok()
            .filter(|h| !h.is_invalid())
    }
}

fn forward(files: &[PathBuf]) {
    let Some(host) = find_host() else { return };
    let mut data: Vec<u16> = Vec::new();
    for f in files {
        data.extend(f.as_os_str().encode_wide());
        data.push(0);
    }
    let cds = COPYDATASTRUCT {
        dwData: 1,
        cbData: (data.len() * 2) as u32,
        lpData: data.as_mut_ptr().cast(),
    };
    unsafe {
        SendMessageW(
            host,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(&cds as *const _ as isize)),
        );
    }
}

pub fn open_settings() {
    unsafe {
        if let Ok(h) = FindWindowW(None, &windows::core::HSTRING::from(SETTINGS_TITLE)) {
            if !h.is_invalid() {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
                return;
            }
        }
    }
    if let Ok(exe) = env::current_exe() {
        let _ = Command::new(exe).arg("--settings").spawn();
    }
}

impl App {
    fn finished(&mut self, outputs: Vec<PathBuf>) {
        for o in outputs.iter().rev() {
            self.recent.retain(|r| r != o);
            self.recent.insert(0, o.clone());
        }
        self.recent.truncate(8);
        self.last = outputs;
        self.tray.set_recent(&self.recent, true);
    }

    fn tray_action(&mut self, action: Action) {
        match action {
            Action::CheckUpdates => {
                self.tray.set_checking();
                let host = self.host;
                update::spawn_check(Duration::ZERO, move || post(host, WM_UPDATE));
            }
            Action::OpenRelease => sys::open(update::RELEASES),
            Action::Reveal(p) => sys::reveal(&p),
            Action::Undo => {
                sys::recycle(&self.last);
                self.recent.retain(|r| !self.last.contains(r));
                self.last.clear();
                self.tray.set_recent(&self.recent, false);
            }
            Action::Settings => open_settings(),
            Action::ToggleLogin(on) => {
                self.config.launch_at_login = on;
                self.save_and_apply();
            }
            Action::ToggleTop(on) => {
                self.config.always_on_top = on;
                self.save_and_apply();
            }
            Action::Quit => unsafe { PostQuitMessage(0) },
        }
    }

    fn save_and_apply(&mut self) {
        let _ = self.config.save();
        sys::apply(&self.config);
    }

    fn reload(&mut self) {
        self.config = Config::load();
        hook::set_keys(&self.config);
        register_hotkeys(HWND(self.host as _), &self.config);
        sys::apply(&self.config);
        self.tray.sync(&self.config);
    }
}

fn register_hotkeys(host: HWND, config: &Config) {
    for (id, key) in [
        (HOTKEY_SELECTION, config.selection_key),
        (HOTKEY_CLIPBOARD, config.clipboard_key),
    ] {
        unsafe {
            let _ = UnregisterHotKey(Some(host), id);
            if key != 0 {
                let _ = RegisterHotKey(
                    Some(host),
                    id,
                    MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                    key,
                );
            }
        }
    }
}

fn kind(wparam: WPARAM) -> WheelKind {
    if wparam.0 == 2 {
        WheelKind::Modify
    } else {
        WheelKind::Convert
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY_DOWN => {
            with_app(|a| a.wheel.show(kind(wparam), &a.config));
        }
        WM_HOTKEY_UP => {
            with_app(|a| a.wheel.hotkey_up());
        }
        WM_ESCAPE => {
            with_app(|a| a.wheel.escape());
        }
        WM_JOB => {
            with_app(|a| {
                if let Some(outputs) = a.wheel.on_job(&a.config) {
                    a.finished(outputs);
                }
            });
        }
        WM_TRAY => {
            let msgs = std::mem::take(&mut *QUEUE.lock().unwrap());
            with_app(|a| {
                for m in msgs {
                    if let Some(action) = a.tray.action(m) {
                        a.tray_action(action);
                    }
                }
            });
        }
        WM_HOTKEY => {
            let files = if wparam.0 as i32 == HOTKEY_SELECTION {
                import::explorer_selection()
            } else {
                import::clipboard()
            };
            if !files.is_empty() {
                with_app(|a| a.wheel.open_files(files, &a.config));
            }
        }
        WM_RELOAD => {
            with_app(App::reload);
        }
        WM_UPDATE => {
            let status = update::RESULT.lock().unwrap().clone();
            if let Some(s) = status {
                with_app(|a| a.tray.set_update(&s));
            }
            sys::trim();
        }
        WM_COPYDATA => {
            let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            let data = unsafe {
                std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2)
            };
            let files: Vec<PathBuf> = String::from_utf16_lossy(data)
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect();
            if files.is_empty() {
                open_settings();
            } else {
                with_app(|a| a.wheel.open_files(files, &a.config));
            }
            return LRESULT(1);
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

pub fn run(files: Vec<PathBuf>) {
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("CrystalConverter.Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            forward(&files);
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);
        let instance = GetModuleHandleW(None).expect("module handle").into();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: instance,
            lpszClassName: HOST_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        let host = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            HOST_CLASS,
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
        .expect("host window");
        let host_id = host.0 as isize;
        let first_run = !Config::exists();
        let config = Config::load();
        hook::install(host, &config);
        register_hotkeys(host, &config);
        import::clear_staging();
        sys::apply(&config);
        let tray = Tray::new(move || post(host_id, WM_TRAY), &config);
        if config.check_updates {
            update::spawn_check(Duration::from_secs(30), move || post(host_id, WM_UPDATE));
        }
        if first_run {
            let _ = config.save();
            open_settings();
        }
        let mut wheel = Wheel::new(host);
        if !files.is_empty() {
            wheel.open_files(files, &config);
        }
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                config,
                host: host_id,
                tray,
                wheel,
                recent: Vec::new(),
                last: Vec::new(),
            })
        });
        sys::trim();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        hook::uninstall();
        APP.with(|a| a.borrow_mut().take());
    }
}
