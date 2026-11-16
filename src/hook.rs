use crate::{app, config::Config};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering::Relaxed};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::{
            CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK,
            KBDLLHOOKSTRUCT, LLKHF_INJECTED, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
        },
    },
};

static HOST: AtomicIsize = AtomicIsize::new(0);
static HOOK: AtomicIsize = AtomicIsize::new(0);
static CONVERT: AtomicU32 = AtomicU32::new(0);
static MODIFY: AtomicU32 = AtomicU32::new(0);
static HELD: AtomicU32 = AtomicU32::new(0);
pub static ESCAPE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn install(host: HWND, config: &Config) {
    set_keys(config);
    HOST.store(host.0 as isize, Relaxed);
    unsafe {
        let module = GetModuleHandleW(None).ok().map(Into::into);
        if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) {
            HOOK.store(h.0 as isize, Relaxed);
        }
    }
}

pub fn set_keys(config: &Config) {
    CONVERT.store(config.convert_key, Relaxed);
    MODIFY.store(config.modify_key, Relaxed);
}

pub fn uninstall() {
    let h = HOOK.swap(0, Relaxed);
    if h != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(h as _));
        }
    }
}

fn post(msg: u32, wparam: usize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(HOST.load(Relaxed) as _)),
            msg,
            WPARAM(wparam),
            LPARAM(0),
        );
    }
}

fn mouse_down() -> bool {
    unsafe {
        GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if kb.flags.0 & LLKHF_INJECTED.0 == 0 {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let vk = kb.vkCode;
            let kind = if vk == CONVERT.load(Relaxed) {
                1
            } else if vk == MODIFY.load(Relaxed) {
                2
            } else {
                0
            };
            if kind != 0 {
                let held = HELD.load(Relaxed);
                if down {
                    if held == kind {
                        return LRESULT(1);
                    }
                    if held == 0 && mouse_down() {
                        HELD.store(kind, Relaxed);
                        post(app::WM_HOTKEY_DOWN, kind as usize);
                        return LRESULT(1);
                    }
                } else if held == kind {
                    HELD.store(0, Relaxed);
                    post(app::WM_HOTKEY_UP, kind as usize);
                    return LRESULT(1);
                }
            }
            if down && vk == u32::from(VK_ESCAPE.0) && ESCAPE_ACTIVE.load(Relaxed) {
                post(app::WM_ESCAPE, 0);
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
