use crystal_converter::{
    config::{Category, WheelKind},
    formats,
    render::{self, CenterIcon, Fonts, Scene, SliceView, Status},
};
use std::path::PathBuf;
use tiny_skia::{Color, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, Rect, SpreadMode, Transform};

const W: u32 = 720;
const H: u32 = 520;

struct Frame<'a> {
    kind: WheelKind,
    labels: Vec<String>,
    hover: Option<usize>,
    appear: f32,
    center_hover: f32,
    center_icon: CenterIcon,
    title: &'a str,
    subtitle: &'a str,
    open: f32,
    morph: f32,
    status: Option<Status>,
    status_t: f32,
    time: f32,
    button_hover: [f32; 2],
}

fn background() -> Pixmap {
    let mut bg = Pixmap::new(W, H).unwrap();
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(W as f32, H as f32),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(206, 214, 222, 255)),
            GradientStop::new(1.0, Color::from_rgba8(120, 132, 146, 255)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap();
    let paint = Paint { shader, ..Default::default() };
    bg.fill_rect(Rect::from_xywh(0.0, 0.0, W as f32, H as f32).unwrap(), &paint, Transform::identity(), None);
    bg
}

fn render(fonts: &Fonts, bg: &Pixmap, f: &Frame) -> Pixmap {
    let scale = 1.0;
    let size = (render::SIZE * scale) as u32;
    let mut wheel = Pixmap::new(size, size).unwrap();
    let views: Vec<SliceView> = f
        .labels
        .iter()
        .enumerate()
        .map(|(i, l)| SliceView { label: l, hover: if f.hover == Some(i) { 1.0 } else { 0.0 }, appear: f.appear })
        .collect();
    let scene = Scene {
        scale,
        open: f.open,
        accent: if f.kind == WheelKind::Convert { render::CONVERT_ACCENT } else { render::MODIFY_ACCENT },
        slices: &views,
        center_hover: f.center_hover,
        center_icon: f.center_icon,
        title: f.title,
        subtitle: f.subtitle,
        morph: f.morph,
        status: f.status,
        status_t: f.status_t,
        time: f.time,
        button_hover: f.button_hover,
        tooltip: f.button_hover.iter().position(|h| *h > 0.02).map(|i| (i, render::BUTTON_TIPS[i])),
    };
    render::draw(&mut wheel, &scene, fonts);
    let mut out = bg.clone();
    let x = (W - size) as i32 / 2;
    let y = (H - size) as i32 / 2;
    out.draw_pixmap(x, y, wheel.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    out
}

fn labels(kind: WheelKind, category: Category, ext: &str) -> Vec<String> {
    formats::slices(kind, category, ext, true, &crystal_converter::config::default_presets()).into_iter().map(|p| p.name).collect()
}

fn main() {
    let out_dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/frames".into()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let fonts = Fonts::load().expect("system fonts");
    let bg = background();
    let video = labels(WheelKind::Convert, Category::Video, "mov");
    let modify = labels(WheelKind::Modify, Category::Video, "mp4");
    let base = Frame {
        kind: WheelKind::Convert,
        labels: video.clone(),
        hover: None,
        appear: 1.0,
        center_hover: 0.0,
        center_icon: CenterIcon::Drop,
        title: "Video",
        subtitle: "MOV",
        open: 1.0,
        morph: 0.0,
        status: None,
        status_t: 0.0,
        time: 0.0,
        button_hover: [0.0; 2],
    };
    let stills = [
        ("convert", Frame { hover: Some(0), ..base_clone(&base) }),
        ("modify", Frame { kind: WheelKind::Modify, labels: modify.clone(), hover: Some(1), title: "Video", subtitle: "MP4", ..base_clone(&base) }),
        ("converting", Frame { morph: 1.0, status: Some(Status::Converting(Some(0.42))), title: "Converting", subtitle: "holiday.mov", time: 1.3, ..base_clone(&base) }),
        ("done", Frame { morph: 1.0, status: Some(Status::Done), status_t: 2.0, title: "Done", subtitle: "holiday.mp4", button_hover: [1.0, 0.0], ..base_clone(&base) }),
    ];
    for (name, frame) in &stills {
        render(&fonts, &bg, frame).save_png(out_dir.join(format!("{name}.png"))).unwrap();
    }

    let fps = 30.0;
    let mut n = 0;
    let mut push = |f: Frame| {
        render(&fonts, &bg, &f).save_png(out_dir.join(format!("anim-{n:04}.png"))).unwrap();
        n += 1;
    };
    let empty: Vec<String> = Vec::new();
    for i in 0..8 {
        let t = i as f32 / fps;
        push(Frame { labels: empty.clone(), open: (t / 0.22).min(1.0), title: "Drop here", subtitle: "Convert", ..base_clone(&base) });
    }
    for i in 0..10 {
        let t = i as f32 / fps;
        push(Frame { appear: (t / 0.22).min(1.0), ..base_clone(&base) });
    }
    let hovers = [7usize, 7, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0];
    for (i, h) in hovers.iter().enumerate() {
        push(Frame { hover: Some(*h), time: i as f32 / fps, ..base_clone(&base) });
    }
    for i in 0..12 {
        let t = i as f32 / fps;
        let m = render::ease_in_out_cubic(t / 0.34);
        push(Frame { hover: Some(0), morph: m, status: Some(Status::Preparing), title: "Preparing", subtitle: "holiday.mov", time: t, ..base_clone(&base) });
    }
    for i in 0..45 {
        let t = i as f32 / fps;
        push(Frame {
            morph: 1.0,
            status: Some(Status::Converting(Some((t / 1.5).min(1.0)))),
            title: "Converting",
            subtitle: "holiday.mov",
            time: t,
            ..base_clone(&base)
        });
    }
    for i in 0..40 {
        let t = i as f32 / fps;
        let bh = if t > 0.8 { ((t - 0.8) * 6.0).min(1.0) } else { 0.0 };
        push(Frame { morph: 1.0, status: Some(Status::Done), status_t: t, title: "Done", subtitle: "holiday.mp4", button_hover: [bh, 0.0], ..base_clone(&base) });
    }
    for i in 0..6 {
        let t = i as f32 / fps;
        push(Frame { morph: 1.0, status: Some(Status::Done), status_t: 2.0, open: 1.0 - render::ease_out_cubic(t / 0.18), title: "Done", subtitle: "holiday.mp4", ..base_clone(&base) });
    }
    for _ in 0..8 {
        push(Frame { open: 0.0, ..base_clone(&base) });
    }
    println!("{n} animation frames written to {}", out_dir.display());
}

fn base_clone<'a>(f: &Frame<'a>) -> Frame<'a> {
    Frame { labels: f.labels.clone(), ..*f }
}

impl Clone for Frame<'_> {
    fn clone(&self) -> Self {
        base_clone(self)
    }
}
