use crate::config::{Config, WheelKind};
use std::path::PathBuf;
use windows::Win32::Foundation::HWND;

pub struct Wheel {
    _host: isize,
}

impl Wheel {
    pub fn new(host: HWND) -> Self {
        Self { _host: host.0 as isize }
    }
    pub fn show(&mut self, _kind: WheelKind, _config: &Config) {}
    pub fn hotkey_up(&mut self) {}
    pub fn escape(&mut self) {}
    pub fn open_files(&mut self, _files: Vec<PathBuf>, _config: &Config) {}
    pub fn on_job(&mut self, _config: &Config) -> Option<Vec<PathBuf>> {
        None
    }
}
