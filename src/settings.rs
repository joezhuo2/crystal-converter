use crate::{
    app,
    config::{self, Category, Config, OutputMode, Preset, WheelKind},
    convert, formats, render, sys, update,
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ComboBox, CornerRadius, Event, FontData, FontDefinitions,
    FontFamily, FontId, Id, MouseWheelUnit, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const ACCENT: Color32 = Color32::from_rgb(168, 224, 255);
const ERROR: Color32 = Color32::from_rgb(255, 69, 58);
const BG: Color32 = Color32::from_rgb(10, 10, 10);
const SURFACE: Color32 = Color32::from_rgb(20, 20, 20);
const RAISED: Color32 = Color32::from_rgb(30, 30, 30);
const HOVER: Color32 = Color32::from_rgb(40, 40, 40);
const TEXT: Color32 = Color32::from_rgb(245, 245, 245);
const MUTED: Color32 = Color32::from_rgb(140, 140, 140);
const SPRING: (f32, f32, f32, f32) = (0.34, 1.25, 0.64, 1.0);
const EASE: (f32, f32, f32, f32) = (0.2, 0.8, 0.2, 1.0);
const RESET_AFTER: f64 = 1.5;

const KEY_NAMES: &[(u32, &str)] = &[
    (0xC0, "` (Backtick)"),
    (0x14, "Caps Lock"),
    (0x09, "Tab"),
    (0x20, "Space"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x24, "Home"),
    (0x23, "End"),
    (0x21, "Page Up"),
    (0x22, "Page Down"),
    (0x13, "Pause"),
    (0x91, "Scroll Lock"),
    (0x90, "Num Lock"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
];
const IGNORED_KEYS: &[u32] = &[
    0x01, 0x02, 0x04, 0x05, 0x06, 0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];
const DEFAULT_KEYS: [u32; 4] = [0xC0, 0x14, 0x45, 0x56];

pub fn key_name(vk: u32) -> String {
    if let Some((_, n)) = KEY_NAMES.iter().find(|(k, _)| *k == vk) {
        return n.to_string();
    }
    if (0x70..=0x87).contains(&vk) {
        return format!("F{}", vk - 0x6F);
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyNameTextW, MapVirtualKeyW, MAPVK_VK_TO_VSC,
    };
    let mut buf = [0u16; 64];
    let n = unsafe {
        let sc = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
        GetKeyNameTextW((sc << 16) as i32, &mut buf)
    };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        format!("Key 0x{vk:02X}")
    }
}

fn keys_down() -> Vec<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    (0x08..=0xFEu32)
        .filter(|vk| !IGNORED_KEYS.contains(vk) && unsafe { GetAsyncKeyState(*vk as i32) } < 0)
        .collect()
}

fn cubic_bezier((x1, y1, x2, y2): (f32, f32, f32, f32), x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bez = |a: f32, b: f32, t: f32| {
        3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3)
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(4)
}

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: f64,
    dur: f32,
}

impl Tween {
    fn at(&self, now: f64, ease: &dyn Fn(f32) -> f32) -> (f32, bool) {
        let t = ((now - self.start) as f32 / self.dur.max(1e-3)).clamp(0.0, 1.0);
        (self.from + (self.to - self.from) * ease(t), t < 1.0)
    }
}

fn tween(ui: &Ui, id: Id, target: f32, dur: f32, ease: &dyn Fn(f32) -> f32) -> f32 {
    let now = ui.input(|i| i.time);
    let mut tw = ui.data(|d| d.get_temp::<Tween>(id)).unwrap_or(Tween {
        from: target,
        to: target,
        start: now,
        dur,
    });
    if tw.to != target {
        let (current, _) = tw.at(now, ease);
        tw = Tween {
            from: current,
            to: target,
            start: now,
            dur,
        };
    }
    ui.data_mut(|d| d.insert_temp(id, tw));
    let (v, running) = tw.at(now, ease);
    if running {
        ui.ctx().request_repaint();
    }
    v
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(48.0, 28.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let id = resp.id;
    let p = tween(
        ui,
        id.with("pos"),
        if *on { 1.0 } else { 0.0 },
        0.32,
        &|t| cubic_bezier(SPRING, t),
    );
    let pressed = resp.is_pointer_button_down_on();
    let stretch = tween(
        ui,
        id.with("press"),
        if pressed { 1.0 } else { 0.0 },
        0.2,
        &ease_out_cubic,
    );
    let painter = ui.painter();
    let track = lerp_color(RAISED, ACCENT, p);
    painter.rect_filled(rect, CornerRadius::same(14), track);
    if resp.hovered() {
        painter.rect_stroke(
            rect,
            CornerRadius::same(14),
            Stroke::new(1.0, HOVER),
            StrokeKind::Inside,
        );
    }
    let knob_w = 22.0 + 4.0 * stretch;
    let x = rect.left() + 3.0 + p * (42.0 - knob_w);
    let knob = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(knob_w, 22.0));
    painter.rect_filled(
        knob.translate(vec2(0.0, 1.5)),
        CornerRadius::same(11),
        Color32::from_black_alpha(70),
    );
    painter.rect_filled(knob, CornerRadius::same(11), Color32::from_white_alpha(230));
    resp
}

fn glide_bar<T: PartialEq + Copy>(
    ui: &mut Ui,
    id: Id,
    items: &[(T, &str)],
    value: &mut T,
    width: f32,
) -> Response {
    let h = 32.0;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), SURFACE);
    let w = (rect.width() - 6.0) / items.len() as f32;
    let selected = items.iter().position(|(v, _)| v == value).unwrap_or(0);
    let prev = ui
        .data(|d| d.get_temp::<usize>(id.with("prev")))
        .unwrap_or(selected);
    let forward = selected >= prev;
    let (dl, dr) = if forward { (0.32, 0.2) } else { (0.2, 0.32) };
    let target_l = rect.left() + 3.0 + w * selected as f32;
    let left = tween(ui, id.with("l"), target_l, dl, &|t| cubic_bezier(SPRING, t));
    let right = tween(ui, id.with("r"), target_l + w, dr, &|t| {
        cubic_bezier(SPRING, t)
    });
    if prev != selected {
        ui.data_mut(|d| d.insert_temp(id.with("prev"), selected));
    }
    let ind = Rect::from_min_max(
        pos2(left, rect.top() + 3.0),
        pos2(right.max(left + 4.0), rect.bottom() - 3.0),
    );
    ui.painter().rect_filled(
        ind.translate(vec2(0.0, 2.0)),
        CornerRadius::same(7),
        Color32::from_black_alpha(38),
    );
    ui.painter()
        .rect_filled(ind, CornerRadius::same(7), ACCENT.gamma_multiply(0.22));
    ui.painter().rect_stroke(
        ind,
        CornerRadius::same(7),
        Stroke::new(1.0, ACCENT.gamma_multiply(0.45)),
        StrokeKind::Inside,
    );
    for (i, (v, label)) in items.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(rect.left() + 3.0 + w * i as f32, rect.top()),
            vec2(w, h),
        );
        let r = ui.interact(cell, id.with(i), Sense::click());
        if r.clicked() && *value != *v {
            *value = *v;
            resp.mark_changed();
        }
        let on = tween(
            ui,
            id.with(("t", i)),
            if i == selected {
                1.0
            } else if r.hovered() {
                0.5
            } else {
                0.0
            },
            0.2,
            &ease_out_cubic,
        );
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(13.5),
            lerp_color(MUTED, TEXT, on),
        );
    }
    resp
}

#[derive(Clone, Copy, Default)]
struct RangeState {
    down: Option<f64>,
    from: f32,
    pending: Option<f32>,
    glide: Option<(f32, f32, f64)>,
}

fn snap(x: f32, min: f32, step: f32) -> f32 {
    if step > 0.0 {
        min + ((x - min) / step).round() * step
    } else {
        x
    }
}

fn smooth_range(
    ui: &mut Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
    width: f32,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let id = resp.id;
    let now = ui.input(|i| i.time);
    let mut st = ui
        .data(|d| d.get_temp::<RangeState>(id))
        .unwrap_or_default();
    let track = rect.shrink2(vec2(9.0, 0.0));
    let at_pointer = |pos: Pos2| {
        snap(
            min + (max - min) * ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0),
            min,
            step,
        )
        .clamp(min, max)
    };
    let held = |st: &RangeState| st.down.is_some_and(|d| now - d > 0.25);
    if resp.is_pointer_button_down_on() && st.down.is_none() {
        st = RangeState {
            down: Some(now),
            from: *value,
            pending: None,
            glide: None,
        };
    }
    if let (Some(_), Some(pos)) = (st.down, resp.interact_pointer_pos()) {
        let v = at_pointer(pos);
        if held(&st) {
            if v != *value {
                *value = v;
                resp.mark_changed();
            }
            st.pending = None;
        } else {
            st.pending = Some(v);
        }
    }
    if st.down.is_some() && !resp.is_pointer_button_down_on() {
        if let Some(to) = st.pending.filter(|to| *to != st.from) {
            if held(&st) {
                *value = to;
                resp.mark_changed();
            } else {
                st.glide = Some((st.from, to, now));
            }
        }
        st.down = None;
        st.pending = None;
    }
    if let Some((from, to, start)) = st.glide {
        let elapsed = (now - start) as f32;
        let v = snap(
            from + (to - from) * ease_out_quart(elapsed / 0.25),
            min,
            step,
        )
        .clamp(min.min(max), max.max(min));
        if v != *value {
            *value = v;
            resp.mark_changed();
        }
        if elapsed >= 0.25 {
            st.glide = None;
        } else {
            ui.ctx().request_repaint();
        }
    }
    ui.data_mut(|d| d.insert_temp(id, st));
    let pct = if max > min {
        ((*value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let painter = ui.painter();
    let line = Rect::from_center_size(track.center(), vec2(track.width(), 4.0));
    painter.rect_filled(line, CornerRadius::same(2), RAISED);
    let x = track.left() + track.width() * pct;
    painter.rect_filled(
        Rect::from_min_max(line.min, pos2(x, line.max.y)),
        CornerRadius::same(2),
        ACCENT,
    );
    let grow = tween(
        ui,
        id.with("hover"),
        if resp.hovered() || resp.dragged() {
            1.0
        } else {
            0.0
        },
        0.2,
        &ease_out_cubic,
    );
    painter.circle_filled(
        pos2(x, track.center().y + 1.0),
        8.0 + 0.6 * grow,
        Color32::from_black_alpha(60),
    );
    painter.circle_filled(pos2(x, track.center().y), 7.5 + 0.6 * grow, TEXT);
    resp
}

fn animated_number(ui: &mut Ui, id: Id, value: f32, fmt: &dyn Fn(f32) -> String) {
    let shown = tween(ui, id, value, 0.45, &ease_out_cubic);
    ui.add_sized(
        [78.0, 20.0],
        egui::Label::new(RichText::new(fmt(shown)).color(TEXT).monospace()),
    );
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Plus,
    Trash,
    Up,
    Down,
    Reset,
    Folder,
    Refresh,
    Link,
    Copy,
    Check,
    Cross,
    Key,
    Download,
}

fn paint_icon(painter: &egui::Painter, icon: Icon, c: Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    let p = |x: f32, y: f32| pos2(c.x + x, c.y + y);
    let line = |pts: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            pts.iter().map(|(x, y)| p(*x, *y)).collect(),
            s,
        ))
    };
    match icon {
        Icon::Plus => {
            line(&[(-5.0, 0.0), (5.0, 0.0)]);
            line(&[(0.0, -5.0), (0.0, 5.0)]);
        }
        Icon::Trash => {
            line(&[(-5.5, -3.5), (5.5, -3.5)]);
            line(&[(-4.0, -3.5), (-3.2, 5.5), (3.2, 5.5), (4.0, -3.5)]);
            line(&[(-1.8, -3.5), (-1.8, -5.5), (1.8, -5.5), (1.8, -3.5)]);
        }
        Icon::Up => {
            line(&[(-4.5, 2.5), (0.0, -2.5), (4.5, 2.5)]);
        }
        Icon::Down => {
            line(&[(-4.5, -2.5), (0.0, 2.5), (4.5, -2.5)]);
        }
        Icon::Reset | Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -0.6 + i as f32 / 20.0 * 4.9;
                    p(5.0 * a.cos(), 5.0 * a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(pts, s));
            line(&[
                (5.0 * 4.3f32.cos() - 2.5, 5.0 * 4.3f32.sin()),
                (5.0 * 4.3f32.cos(), 5.0 * 4.3f32.sin()),
                (5.0 * 4.3f32.cos(), 5.0 * 4.3f32.sin() + 2.5),
            ]);
        }
        Icon::Folder => {
            line(&[
                (-6.0, -4.0),
                (-2.0, -4.0),
                (-0.5, -2.5),
                (6.0, -2.5),
                (6.0, 4.5),
                (-6.0, 4.5),
                (-6.0, -4.0),
            ]);
        }
        Icon::Link => {
            line(&[(-1.0, -5.0), (5.0, -5.0), (5.0, 1.0)]);
            line(&[(5.0, -5.0), (-1.5, 1.5)]);
            line(&[
                (2.0, 3.0),
                (2.0, 5.0),
                (-5.0, 5.0),
                (-5.0, -2.0),
                (-3.0, -2.0),
            ]);
        }
        Icon::Copy => {
            line(&[(-2.0, -5.0), (5.0, -5.0), (5.0, 2.0)]);
            line(&[
                (-5.0, -2.0),
                (2.0, -2.0),
                (2.0, 5.0),
                (-5.0, 5.0),
                (-5.0, -2.0),
            ]);
        }
        Icon::Key => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(14.0, 9.0)),
                CornerRadius::same(2),
                s,
                StrokeKind::Middle,
            );
            for x in [-3.5, 0.0, 3.5] {
                painter.circle_filled(p(x, -1.2), 0.8, color);
            }
            line(&[(-3.0, 2.2), (3.0, 2.2)]);
        }
        Icon::Download => {
            line(&[(0.0, -6.0), (0.0, 2.0)]);
            line(&[(-3.5, -1.5), (0.0, 2.0), (3.5, -1.5)]);
            line(&[(-6.0, 2.5), (-6.0, 5.5), (6.0, 5.5), (6.0, 2.5)]);
        }
        Icon::Check => {
            line(&[(-5.0, 0.0), (-1.5, 3.5), (5.0, -3.5)]);
        }
        Icon::Cross => {
            line(&[(-4.0, -4.0), (4.0, 4.0)]);
            line(&[(4.0, -4.0), (-4.0, 4.0)]);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fb {
    Idle,
    Busy,
    Done,
    Error,
}

#[derive(Default)]
struct Feedback {
    states: HashMap<&'static str, (Fb, f64)>,
}

impl Feedback {
    fn state(&mut self, key: &'static str, now: f64) -> (Fb, f64) {
        let entry = self.states.entry(key).or_insert((Fb::Idle, -10.0));
        if matches!(entry.0, Fb::Done | Fb::Error) && now - entry.1 > RESET_AFTER {
            *entry = (Fb::Idle, entry.1 + RESET_AFTER);
        }
        *entry
    }

    fn set(&mut self, key: &'static str, fb: Fb, now: f64) {
        self.states.insert(key, (fb, now));
    }

    fn button(&mut self, ui: &mut Ui, key: &'static str, icon: Icon, label: &str) -> bool {
        let now = ui.input(|i| i.time);
        let (fb, since) = self.state(key, now);
        let (glyph, text, color) = match fb {
            Fb::Idle => (icon, label.to_string(), TEXT),
            Fb::Busy => (icon, format!("{label}…"), TEXT),
            Fb::Done => (Icon::Check, "Done".to_string(), ACCENT),
            Fb::Error => (Icon::Cross, "Failed".to_string(), ERROR),
        };
        let font = FontId::proportional(13.0);
        let text_w = ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), color).size().x);
        let size = if text.is_empty() {
            vec2(30.0, 30.0)
        } else {
            vec2(text_w + 40.0, 30.0)
        };
        let (rect, resp) = ui.allocate_exact_size(
            size,
            if fb == Fb::Busy {
                Sense::hover()
            } else {
                Sense::click()
            },
        );
        let hover = tween(
            ui,
            resp.id.with("h"),
            if resp.hovered() { 1.0 } else { 0.0 },
            0.15,
            &ease_out_cubic,
        );
        let press = if resp.is_pointer_button_down_on() {
            0.985
        } else {
            1.0
        };
        let r = Rect::from_center_size(rect.center(), rect.size() * press);
        ui.painter()
            .rect_filled(r, CornerRadius::same(8), lerp_color(RAISED, HOVER, hover));
        let swap = cubic_bezier(EASE, ((now - since) as f32 / 0.3).clamp(0.0, 1.0));
        let mut alpha = swap;
        if fb == Fb::Busy {
            alpha *= 0.7 + 0.3 * ((now * std::f64::consts::TAU / 0.8).cos() as f32 * 0.5 + 0.5);
        }
        let dy = 4.0 * (1.0 - swap);
        let c = color.gamma_multiply(alpha);
        let icon_x = if text.is_empty() {
            r.center().x
        } else {
            r.left() + 17.0
        };
        paint_icon(ui.painter(), glyph, pos2(icon_x, r.center().y + dy), c);
        if !text.is_empty() {
            ui.painter().text(
                pos2(r.left() + 30.0, r.center().y + dy),
                Align2::LEFT_CENTER,
                text,
                font,
                c,
            );
        }
        if swap < 1.0 || fb == Fb::Busy || matches!(fb, Fb::Done | Fb::Error) {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        resp.clicked()
    }
}

fn row(ui: &mut Ui, label: &str, hint: &str, add: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().top();
    ui.horizontal(|ui| {
        ui.set_min_height(40.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).color(TEXT).size(14.0));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).color(MUTED).size(11.5));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
    let bottom = ui.cursor().top() + 4.0;
    let x = ui.max_rect().x_range();
    if bottom > top {
        ui.painter().hline(x, bottom, Stroke::new(1.0, SURFACE));
    }
    ui.add_space(10.0);
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Hotkeys,
    Presets,
    About,
}

struct SettingsApp {
    config: Config,
    saved: Config,
    dirty_at: Option<f64>,
    tab: Tab,
    scroll: f32,
    scroll_target: f32,
    scroll_max: f32,
    selected: usize,
    feedback: Feedback,
    update: Arc<Mutex<Option<update::Status>>>,
    checking: bool,
    recording: Option<(usize, Vec<u32>)>,
    deps: Option<(bool, bool)>,
    installs: Arc<Mutex<Vec<(&'static str, bool)>>>,
}

impl SettingsApp {
    fn new(cc: &eframe::CreationContext, tab: Tab) -> Self {
        let mut fonts = FontDefinitions::default();
        let dir = std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("Fonts");
        if let Ok(bytes) = std::fs::read(dir.join("segoeui.ttf")) {
            fonts
                .font_data
                .insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "segoe".into());
        }
        if let Ok(bytes) = std::fs::read(dir.join("consola.ttf")) {
            fonts
                .font_data
                .insert("consolas".into(), Arc::new(FontData::from_owned(bytes)));
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "consolas".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = BG;
        v.extreme_bg_color = SURFACE;
        v.faint_bg_color = SURFACE;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        for w in [&mut v.widgets.inactive, &mut v.widgets.noninteractive] {
            w.bg_fill = RAISED;
            w.weak_bg_fill = RAISED;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, TEXT);
            w.corner_radius = CornerRadius::same(8);
        }
        v.widgets.noninteractive.bg_fill = BG;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        for w in [
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.bg_fill = HOVER;
            w.weak_bg_fill = HOVER;
            w.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.45));
            w.fg_stroke = Stroke::new(1.0, TEXT);
            w.corner_radius = CornerRadius::same(8);
        }
        cc.egui_ctx.set_visuals(v);
        let config = Config::load();
        Self {
            saved: config.clone(),
            config,
            dirty_at: None,
            tab,
            scroll: 0.0,
            scroll_target: 0.0,
            scroll_max: 0.0,
            selected: 0,
            feedback: Feedback::default(),
            update: Arc::new(Mutex::new(None)),
            checking: false,
            recording: None,
            deps: None,
            installs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn save(&mut self) {
        if self.config.save().is_ok() {
            self.saved = self.config.clone();
            if let Some(host) = app::find_host() {
                app::post(host.0 as isize, app::WM_RELOAD);
            }
        }
    }

    fn general(&mut self, ui: &mut Ui) {
        let c = &mut self.config;
        row(ui, "Close after", "How long Done stays up", |ui| {
            animated_number(
                ui,
                Id::unique("delay"),
                c.close_delay_ms as f32 / 1000.0,
                &|x| format!("{x:>4.1} s"),
            );
            let mut v = c.close_delay_ms as f32;
            if smooth_range(ui, &mut v, 500.0, 5000.0, 100.0, 200.0).changed() {
                c.close_delay_ms = v as u32;
            }
        });
        row(ui, "Sounds", "Chime when a job starts and ends", |ui| {
            switch(ui, &mut c.sounds);
        });
        row(ui, "Copy to clipboard", "Copy results when done", |ui| {
            switch(ui, &mut c.copy_to_clipboard);
        });
        row(ui, "Save to", "Where converted files go", |ui| {
            glide_bar(
                ui,
                Id::unique("output"),
                &[
                    (OutputMode::NextToOriginal, "Next to original"),
                    (OutputMode::Folder, "Folder"),
                ],
                &mut c.output_mode,
                240.0,
            );
        });
        if c.output_mode == OutputMode::Folder {
            row(ui, "Folder", "", |ui| {
                let browse = self.feedback.button(ui, "browse", Icon::Folder, "Browse");
                ui.add(
                    egui::TextEdit::singleline(&mut c.output_folder)
                        .desired_width(220.0)
                        .hint_text("C:\\Converted"),
                );
                if browse {
                    let now = ui.input(|i| i.time);
                    match sys::pick_folder() {
                        Some(p) => {
                            c.output_folder = p.display().to_string();
                            self.feedback.set("browse", Fb::Done, now);
                        }
                        None => self.feedback.set("browse", Fb::Error, now),
                    }
                }
            });
        }
        row(
            ui,
            "Hardware encoding",
            "Use the GPU for MP4 when available",
            |ui| {
                switch(ui, &mut c.hardware_encoding);
            },
        );
        row(
            ui,
            "Parallel files",
            "Images and audio convert this many at once",
            |ui| {
                let mut v = c.parallel_jobs.clamp(1, 8) as f32;
                if smooth_range(ui, &mut v, 1.0, 8.0, 1.0, 120.0).changed() {
                    c.parallel_jobs = v as u32;
                }
                ui.label(c.parallel_jobs.clamp(1, 8).to_string());
            },
        );
        row(
            ui,
            "Replace original",
            "Move the source to the Recycle Bin after a successful conversion",
            |ui| {
                switch(ui, &mut c.replace_original);
            },
        );
        row(ui, "Launch at login", "", |ui| {
            switch(ui, &mut c.launch_at_login);
        });
        row(
            ui,
            "Always on top",
            "Keep the status circle above windows",
            |ui| {
                switch(ui, &mut c.always_on_top);
            },
        );
        row(
            ui,
            "Explorer menu",
            "Add Convert with Crystal Converter",
            |ui| {
                switch(ui, &mut c.context_menu);
            },
        );
        row(ui, "Check for updates", "Check GitHub at startup", |ui| {
            switch(ui, &mut c.check_updates);
        });
    }

    fn hotkeys(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        ui.label(RichText::new("Hold a key while dragging a file to open the wheel. Release over a slice to convert, or drop in the center and pick.").color(MUTED));
        ui.add_space(12.0);
        if let Some((which, before)) = self.recording.clone() {
            ui.ctx().request_repaint();
            let down = keys_down();
            if let Some(vk) = down.iter().find(|k| !before.contains(k)) {
                if *vk != 0x1B {
                    *self.key_mut(which) = *vk;
                    self.feedback
                        .set(["rec0", "rec1", "rec2", "rec3"][which], Fb::Done, now);
                } else {
                    self.feedback
                        .set(["rec0", "rec1", "rec2", "rec3"][which], Fb::Idle, now);
                }
                self.recording = None;
            } else {
                self.recording = Some((
                    which,
                    before.into_iter().filter(|k| down.contains(k)).collect(),
                ));
            }
        }
        for (which, label, hint) in [
            (0, "Convert wheel", "Change the file format"),
            (1, "Modify wheel", "Quality, size, volume"),
            (2, "Explorer selection", "Open the wheel for selected files"),
            (3, "Clipboard", "Open the wheel for copied files or image"),
        ] {
            let key = *self.key_mut(which);
            let recording = self.recording.as_ref().is_some_and(|(w, _)| *w == which);
            let rec_id = ["rec0", "rec1", "rec2", "rec3"][which];
            let reset_id = ["reset0", "reset1", "reset2", "reset3"][which];
            let mut start_recording = false;
            let mut reset = false;
            let mut turn_off = false;
            row(ui, label, hint, |ui| {
                let text = if recording {
                    "Press a key".to_string()
                } else if which < 2 {
                    key_name(key)
                } else if key == 0 {
                    "Off".to_string()
                } else {
                    format!("Ctrl+Alt+Shift+{}", key_name(key))
                };
                start_recording = self.feedback.button(ui, rec_id, Icon::Key, &text);
                if which >= 2 && key != 0 {
                    turn_off =
                        self.feedback
                            .button(ui, ["off2", "off3"][which - 2], Icon::Trash, "");
                }
                if key != DEFAULT_KEYS[which] {
                    reset = self.feedback.button(ui, reset_id, Icon::Reset, "");
                }
            });
            if start_recording {
                self.recording = Some((which, keys_down()));
                self.feedback.set(rec_id, Fb::Busy, now);
            }
            if turn_off {
                *self.key_mut(which) = 0;
            }
            if reset {
                *self.key_mut(which) = DEFAULT_KEYS[which];
                self.feedback.set(reset_id, Fb::Done, now);
            }
        }
        if self.config.convert_key == self.config.modify_key {
            ui.label(
                RichText::new("Both wheels use the same key. Pick different keys.").color(ERROR),
            );
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Click a key, then press the one you want. Esc cancels. Shift, Ctrl, Alt and Win change what a drag does, so they can't be used. The Explorer and Clipboard hotkeys are pressed together with Ctrl+Alt+Shift; the trash button turns one off.").color(MUTED));
    }

    fn key_mut(&mut self, which: usize) -> &mut u32 {
        match which {
            0 => &mut self.config.convert_key,
            1 => &mut self.config.modify_key,
            2 => &mut self.config.selection_key,
            _ => &mut self.config.clipboard_key,
        }
    }

    fn presets(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        ui.horizontal(|ui| {
            if self.feedback.button(ui, "add", Icon::Plus, "Add") {
                self.config.presets.push(Preset::default());
                self.selected = self.config.presets.len() - 1;
                self.feedback.set("add", Fb::Done, now);
            }
            if self.feedback.button(ui, "up", Icon::Up, "") {
                let ok = self.selected > 0 && self.selected < self.config.presets.len();
                if ok {
                    self.config.presets.swap(self.selected, self.selected - 1);
                    self.selected -= 1;
                }
                self.feedback
                    .set("up", if ok { Fb::Done } else { Fb::Error }, now);
            }
            if self.feedback.button(ui, "down", Icon::Down, "") {
                let ok = self.selected + 1 < self.config.presets.len();
                if ok {
                    self.config.presets.swap(self.selected, self.selected + 1);
                    self.selected += 1;
                }
                self.feedback
                    .set("down", if ok { Fb::Done } else { Fb::Error }, now);
            }
            if self.feedback.button(ui, "delete", Icon::Trash, "Delete") {
                let ok = self.selected < self.config.presets.len();
                if ok {
                    self.config.presets.remove(self.selected);
                    self.selected = self.selected.saturating_sub(1);
                }
                self.feedback
                    .set("delete", if ok { Fb::Done } else { Fb::Error }, now);
            }
            if self.feedback.button(ui, "reset", Icon::Reset, "Defaults") {
                self.config.presets = config::default_presets();
                self.selected = 0;
                self.feedback.set("reset", Fb::Done, now);
            }
        });
        ui.add_space(10.0);
        let names: Vec<String> = self.config.presets.iter().map(|p| p.name.clone()).collect();
        ui.horizontal_wrapped(|ui| {
            for (i, name) in names.iter().enumerate() {
                if ui.selectable_label(self.selected == i, name).clicked() {
                    self.selected = i;
                }
            }
        });
        ui.add_space(12.0);
        let Some(p) = self.config.presets.get_mut(self.selected) else {
            ui.label(RichText::new("No presets. Add one to put it on a wheel.").color(MUTED));
            return;
        };
        let id = Id::unique(("preset", self.selected));
        row(ui, "Name", "Shown on the slice", |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(240.0));
        });
        row(ui, "Wheel", "", |ui| {
            glide_bar(
                ui,
                id.with("wheel"),
                &[
                    (WheelKind::Convert, "Convert"),
                    (WheelKind::Modify, "Modify"),
                ],
                &mut p.wheel,
                200.0,
            );
        });
        row(ui, "File type", "", |ui| {
            let items: Vec<(Category, &str)> =
                Category::ALL.iter().map(|c| (*c, c.label())).collect();
            glide_bar(ui, id.with("cat"), &items, &mut p.category, 360.0);
        });
        row(ui, "Output format", "Keep leaves it unchanged", |ui| {
            let options = format_options(p.category);
            let shown = if p.format.is_empty() {
                "Keep".to_string()
            } else {
                p.format.to_uppercase()
            };
            ComboBox::from_id_salt(id.with("fmt"))
                .width(140.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut p.format, String::new(), "Keep");
                    for f in options {
                        ui.selectable_value(&mut p.format, f.to_string(), f.to_uppercase());
                    }
                });
        });
        let slider = |ui: &mut Ui,
                      label: &str,
                      hint: &str,
                      key: &str,
                      v: &mut f32,
                      min: f32,
                      max: f32,
                      step: f32,
                      fmt: &dyn Fn(f32) -> String| {
            row(ui, label, hint, |ui| {
                animated_number(ui, id.with(key), *v, fmt);
                smooth_range(ui, v, min, max, step, 200.0);
            });
        };
        let off = |unit: &'static str| {
            move |x: f32| {
                if x.round() == 0.0 {
                    "Off".to_string()
                } else {
                    format!("{} {unit}", x.round())
                }
            }
        };
        let mut f = p.max_size_mb;
        slider(
            ui,
            "Max size",
            "Video and audio target size",
            "size",
            &mut f,
            0.0,
            500.0,
            1.0,
            &off("MB"),
        );
        p.max_size_mb = f;
        let mut f = p.max_dim as f32;
        slider(
            ui,
            "Max dimension",
            "Longest edge",
            "dim",
            &mut f,
            0.0,
            7680.0,
            20.0,
            &off("px"),
        );
        p.max_dim = f as u32;
        let mut f = p.scale_pct as f32;
        slider(
            ui,
            "Scale",
            "Percent of the original",
            "scale",
            &mut f,
            0.0,
            100.0,
            5.0,
            &off("%"),
        );
        p.scale_pct = f as u32;
        let mut f = p.quality as f32;
        slider(
            ui,
            "Quality",
            "Images and video, 1 to 100",
            "q",
            &mut f,
            0.0,
            100.0,
            1.0,
            &off(""),
        );
        p.quality = f as u32;
        let mut f = p.bitrate_kbps as f32;
        slider(
            ui,
            "Audio bitrate",
            "",
            "br",
            &mut f,
            0.0,
            320.0,
            8.0,
            &off("kbps"),
        );
        p.bitrate_kbps = f as u32;
        let mut f = p.fps as f32;
        slider(
            ui,
            "Frame rate",
            "",
            "fps",
            &mut f,
            0.0,
            120.0,
            1.0,
            &off("fps"),
        );
        p.fps = f as u32;
        let mut f = p.volume_db;
        slider(
            ui,
            "Volume",
            "Gain in decibels",
            "vol",
            &mut f,
            -20.0,
            20.0,
            0.5,
            &|x| format!("{x:+.1} dB"),
        );
        p.volume_db = f;
        row(ui, "Rotate", "", |ui| {
            glide_bar(
                ui,
                id.with("rot"),
                &[(0u32, "0°"), (90, "90°"), (180, "180°"), (270, "270°")],
                &mut p.rotate,
                240.0,
            );
        });
        for (label, value) in [
            ("Trim start", &mut p.trim_start),
            ("Trim end", &mut p.trim_end),
        ] {
            row(ui, label, "Seconds, 0 leaves it unchanged", |ui| {
                ui.add(
                    egui::DragValue::new(value)
                        .speed(0.5)
                        .range(0.0..=86400.0)
                        .suffix(" s"),
                );
            });
        }
        for (label, value) in [
            ("Normalize loudness", &mut p.normalize),
            ("Trim silence", &mut p.trim_silence),
            ("Mono", &mut p.mono),
            ("Mute", &mut p.mute),
            ("Strip metadata", &mut p.strip_metadata),
        ] {
            row(ui, label, "", |ui| {
                switch(ui, value);
            });
        }
    }

    fn about(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        ui.label(RichText::new("Crystal Converter").size(22.0).color(TEXT));
        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(MUTED));
        ui.add_space(14.0);
        let finished: Vec<(&'static str, bool)> =
            std::mem::take(&mut *self.installs.lock().unwrap());
        for (key, ok) in finished {
            self.feedback
                .set(key, if ok { Fb::Done } else { Fb::Error }, now);
            self.deps = None;
        }
        let (ffmpeg, office) = *self.deps.get_or_insert_with(|| {
            (
                convert::find_ffmpeg().is_some(),
                convert::soffice().is_some(),
            )
        });
        for (key, name, hint, found, package, required) in [
            (
                "ffmpeg",
                "ffmpeg",
                "Images, audio and video",
                ffmpeg,
                "Gyan.FFmpeg",
                true,
            ),
            (
                "office",
                "LibreOffice",
                "Documents (optional)",
                office,
                "TheDocumentFoundation.LibreOffice",
                false,
            ),
        ] {
            let busy = self.feedback.state(key, now).0 == Fb::Busy;
            let mut install = false;
            row(ui, name, hint, |ui| {
                if found && !busy {
                    ui.label(RichText::new("Found").color(ACCENT));
                    return;
                }
                install = self.feedback.button(ui, key, Icon::Download, "Install");
                if !busy {
                    ui.label(
                        RichText::new(if required { "Missing" } else { "Not installed" })
                            .color(if required { ERROR } else { MUTED }),
                    );
                }
            });
            if install {
                self.feedback.set(key, Fb::Busy, now);
                let done = self.installs.clone();
                let ctx = ui.ctx().clone();
                std::thread::spawn(move || {
                    let ok = sys::winget_install(package);
                    done.lock().unwrap().push((key, ok));
                    ctx.request_repaint();
                });
            }
        }
        if self.checking {
            let result = self.update.lock().unwrap().take();
            if let Some(status) = result {
                self.checking = false;
                let fb = if status == update::Status::Failed {
                    Fb::Error
                } else {
                    Fb::Done
                };
                self.feedback.set("update", fb, now);
                *self.update.lock().unwrap() = Some(status);
                ui.ctx().request_repaint();
            }
        }
        ui.horizontal(|ui| {
            if self
                .feedback
                .button(ui, "update", Icon::Refresh, "Check for updates")
            {
                self.checking = true;
                self.feedback.set("update", Fb::Busy, now);
                *self.update.lock().unwrap() = None;
                let slot = self.update.clone();
                let ctx = ui.ctx().clone();
                update::spawn_check(Duration::ZERO, move || {
                    let r = update::RESULT.lock().unwrap().clone();
                    *slot.lock().unwrap() = r;
                    ctx.request_repaint();
                });
            }
            if self.feedback.button(ui, "release", Icon::Link, "Releases") {
                sys::open(update::RELEASES);
                self.feedback.set("release", Fb::Done, now);
            }
            if self
                .feedback
                .button(ui, "folder", Icon::Folder, "Config folder")
            {
                let _ = std::fs::create_dir_all(config::dir());
                sys::open(&config::dir().display().to_string());
                self.feedback.set("folder", Fb::Done, now);
            }
            if self.feedback.button(ui, "copyver", Icon::Copy, "") {
                ui.ctx()
                    .copy_text(format!("Crystal Converter {}", env!("CARGO_PKG_VERSION")));
                self.feedback.set("copyver", Fb::Done, now);
            }
        });
        if !self.checking {
            if let Some(status) = self.update.lock().unwrap().clone() {
                let text = match status {
                    update::Status::UpToDate => "You're on the latest version.".to_string(),
                    update::Status::Available(v) => {
                        format!("Version {v} is available on the Releases page.")
                    }
                    update::Status::Failed => "Couldn't reach GitHub.".to_string(),
                };
                ui.add_space(6.0);
                ui.label(RichText::new(text).color(MUTED));
            }
        }
    }
}

fn format_options(category: Category) -> Vec<&'static str> {
    let cats = match category {
        Category::Any => vec![Category::Image, Category::Video, Category::Audio],
        c => vec![c],
    };
    let mut out: Vec<&'static str> = Vec::new();
    for c in cats {
        for f in formats::targets(c, "", true) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(18.0))
            .show(ui, |ui| {
                let mut tab = self.tab;
                let width = ui.available_width();
                glide_bar(
                    ui,
                    Id::unique("tabs"),
                    &[
                        (Tab::General, "General"),
                        (Tab::Hotkeys, "Hotkeys"),
                        (Tab::Presets, "Presets"),
                        (Tab::About, "About"),
                    ],
                    &mut tab,
                    width,
                );
                if tab != self.tab {
                    self.tab = tab;
                    self.scroll = 0.0;
                    self.scroll_target = 0.0;
                }
                ui.add_space(14.0);
                let area = ui.available_rect_before_wrap();
                let hovered = ui.rect_contains_pointer(area);
                let wheel: f32 = ui.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            Event::MouseWheel {
                                unit,
                                delta,
                                modifiers,
                                ..
                            } if !modifiers.ctrl => Some(match unit {
                                MouseWheelUnit::Point => delta.y,
                                MouseWheelUnit::Line => delta.y * 100.0,
                                MouseWheelUnit::Page => delta.y * area.height(),
                            }),
                            _ => None,
                        })
                        .sum()
                });
                if hovered && wheel != 0.0 {
                    self.scroll_target = (self.scroll_target - wheel).clamp(0.0, self.scroll_max);
                }
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                let ease = 1.0 - (1.0 - 0.12f32).powf(dt * 60.0);
                self.scroll += (self.scroll_target - self.scroll) * ease;
                if (self.scroll_target - self.scroll).abs() < 0.5 {
                    self.scroll = self.scroll_target;
                } else {
                    ui.ctx().request_repaint();
                }
                let out = egui::ScrollArea::vertical()
                    .scroll_source(egui::scroll_area::ScrollSource {
                        mouse_wheel: false,
                        ..egui::scroll_area::ScrollSource::SCROLL_BAR
                    })
                    .vertical_scroll_offset(self.scroll)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        Tab::General => self.general(ui),
                        Tab::Hotkeys => self.hotkeys(ui),
                        Tab::Presets => self.presets(ui),
                        Tab::About => self.about(ui),
                    });
                self.scroll_max = (out.content_size.y - out.inner_rect.height()).max(0.0);
                let actual = out.state.clamped_offset().y;
                if (actual - self.scroll).abs() > 2.0 {
                    self.scroll = actual;
                    self.scroll_target = actual;
                }
            });
        if self.config != self.saved {
            let since = *self.dirty_at.get_or_insert(now);
            if now - since > 0.3 {
                self.save();
                self.dirty_at = None;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(320));
            }
        } else {
            self.dirty_at = None;
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.config != self.saved {
            self.save();
        }
    }
}

pub fn run(tab: Option<&str>) {
    let tab = match tab {
        Some("hotkeys") => Tab::Hotkeys,
        Some("presets") => Tab::Presets,
        Some("about") => Tab::About,
        _ => Tab::General,
    };
    let icon = render::app_icon(64);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::SETTINGS_TITLE)
            .with_inner_size([620.0, 600.0])
            .with_min_inner_size([560.0, 420.0])
            .with_icon(Arc::new(egui::IconData {
                rgba: render::unpremultiply(&icon),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    let _ = eframe::run_native(
        app::SETTINGS_TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, tab)))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_bezier_overshoots_then_settles() {
        assert!(cubic_bezier(SPRING, 0.8) > 1.0);
        assert!((cubic_bezier(SPRING, 1.0) - 1.0).abs() < 1e-6);
        assert!(cubic_bezier(EASE, 0.5) > 0.5);
    }

    #[test]
    fn keys_have_names() {
        assert_eq!(key_name(0xC0), "` (Backtick)");
        assert!(key_name(0xFF).starts_with("Key"));
    }
}
