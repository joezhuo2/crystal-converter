use crate::{config::Config, render, update};
use std::{path::PathBuf, sync::Mutex};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub enum TrayMsg {
    LeftClick,
    Menu(MenuId),
}

pub static QUEUE: Mutex<Vec<TrayMsg>> = Mutex::new(Vec::new());

pub enum Action {
    CheckUpdates,
    OpenRelease,
    Reveal(PathBuf),
    Undo,
    Settings,
    ToggleLogin(bool),
    ToggleTop(bool),
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    check: MenuItem,
    status: MenuItem,
    recent: Submenu,
    recent_items: Vec<(MenuItem, PathBuf)>,
    undo: MenuItem,
    settings: MenuItem,
    login: CheckMenuItem,
    top: CheckMenuItem,
    quit: MenuItem,
}

pub fn icon() -> Icon {
    let pm = render::app_icon(32);
    Icon::from_rgba(render::unpremultiply(&pm), 32, 32).expect("tray icon")
}

impl Tray {
    pub fn new(wake: impl Fn() + Send + Sync + Clone + 'static, config: &Config) -> Self {
        let version = env!("CARGO_PKG_VERSION");
        let check = MenuItem::new("Check for updates", true, None);
        let status = MenuItem::new(format!("Version {version}"), false, None);
        let recent = Submenu::new("Recent", false);
        let undo = MenuItem::new("Undo last conversion", false, None);
        let settings = MenuItem::new("Open settings", true, None);
        let login = CheckMenuItem::new("Launch at login", true, config.launch_at_login, None);
        let top = CheckMenuItem::new("Always on top", true, config.always_on_top, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[
            &check,
            &status,
            &PredefinedMenuItem::separator(),
            &recent,
            &undo,
            &PredefinedMenuItem::separator(),
            &settings,
            &login,
            &top,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        let w = wake.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                QUEUE.lock().unwrap().push(TrayMsg::LeftClick);
                w();
            }
        }));
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            QUEUE.lock().unwrap().push(TrayMsg::Menu(e.id));
            wake();
        }));
        let icon = TrayIconBuilder::new()
            .with_icon(icon())
            .with_tooltip("Crystal Converter")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .expect("tray icon");
        Self {
            _icon: icon,
            check,
            status,
            recent,
            recent_items: Vec::new(),
            undo,
            settings,
            login,
            top,
            quit,
        }
    }

    pub fn action(&self, msg: TrayMsg) -> Option<Action> {
        let id = match msg {
            TrayMsg::LeftClick => return Some(Action::Settings),
            TrayMsg::Menu(id) => id,
        };
        if id == self.check.id() {
            Some(Action::CheckUpdates)
        } else if id == self.status.id() {
            Some(Action::OpenRelease)
        } else if id == self.undo.id() {
            Some(Action::Undo)
        } else if id == self.settings.id() {
            Some(Action::Settings)
        } else if id == self.login.id() {
            Some(Action::ToggleLogin(self.login.is_checked()))
        } else if id == self.top.id() {
            Some(Action::ToggleTop(self.top.is_checked()))
        } else if id == self.quit.id() {
            Some(Action::Quit)
        } else {
            self.recent_items
                .iter()
                .find(|(item, _)| id == item.id())
                .map(|(_, p)| Action::Reveal(p.clone()))
        }
    }

    pub fn sync(&self, config: &Config) {
        self.login.set_checked(config.launch_at_login);
        self.top.set_checked(config.always_on_top);
    }

    pub fn set_checking(&self) {
        self.status.set_text("Checking for updates...");
        self.status.set_enabled(false);
    }

    pub fn set_update(&self, status: &update::Status) {
        let version = env!("CARGO_PKG_VERSION");
        let (text, enabled) = match status {
            update::Status::UpToDate => (format!("Up to date (v{version})"), false),
            update::Status::Available(v) => (format!("Update found (v{v})"), true),
            update::Status::Failed => (format!("Couldn't check (v{version})"), false),
        };
        self.status.set_text(text);
        self.status.set_enabled(enabled);
    }

    pub fn set_recent(&mut self, recent: &[PathBuf], can_undo: bool) {
        for (item, _) in self.recent_items.drain(..) {
            let _ = self.recent.remove(&item);
        }
        for path in recent {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let item = MenuItem::new(label, true, None);
            let _ = self.recent.append(&item);
            self.recent_items.push((item, path.clone()));
        }
        self.recent.set_enabled(!recent.is_empty());
        self.undo.set_enabled(can_undo);
    }
}
