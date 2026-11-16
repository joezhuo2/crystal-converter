use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use std::f32::consts::{PI, TAU};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Point,
    RadialGradient, SpreadMode, Stroke, StrokeDash, Transform,
};

pub const SIZE: f32 = 440.0;
pub const R_OUT: f32 = 170.0;
pub const R_IN: f32 = 62.0;
const STATUS_SHRINK: f32 = 0.22;
const BUTTON_Y: f32 = 74.0;
const BUTTON_DX: f32 = 24.0;
const BUTTON_R: f32 = 17.0;

pub type Rgb = (u8, u8, u8);
pub const ICE: Rgb = (168, 224, 255);
pub const PALETTE: [Rgb; 6] = [
    ICE,
    (245, 245, 245),
    (190, 190, 190),
    (245, 245, 245),
    (190, 190, 190),
    (245, 245, 245),
];
pub const CONVERT_ACCENT: Rgb = ICE;
pub const MODIFY_ACCENT: Rgb = ICE;
const DONE: Rgb = ICE;
const ERROR: Rgb = (245, 245, 245);
const BG: Rgb = (8, 8, 8);
const SLICE: Rgb = (30, 30, 30);
const CENTER: Rgb = (18, 18, 18);
const STATUS_BG: Rgb = (16, 16, 16);
const TEXT: Rgb = (245, 245, 245);
const MUTED: Rgb = (140, 140, 140);
const DARK: Rgb = (0, 0, 0);

pub fn rgba(c: Rgb, a: f32) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.70158;
    1.0 + (c1 + 1.0) * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn sector(cx: f32, cy: f32, r0: f32, r1: f32, a0: f32, a1: f32) -> Option<Path> {
    let steps = (((a1 - a0).abs() / (PI / 120.0)).ceil() as usize).max(2);
    let at = |i: usize| a0 + (a1 - a0) * i as f32 / steps as f32;
    let mut pb = PathBuilder::new();
    pb.move_to(cx + r1 * a0.cos(), cy + r1 * a0.sin());
    for i in 1..=steps {
        pb.line_to(cx + r1 * at(i).cos(), cy + r1 * at(i).sin());
    }
    if r0 > 0.5 {
        for i in (0..=steps).rev() {
            pb.line_to(cx + r0 * at(i).cos(), cy + r0 * at(i).sin());
        }
    } else {
        pb.line_to(cx, cy);
    }
    pb.close();
    pb.finish()
}

fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Option<Path> {
    let steps = (((a1 - a0).abs() / (PI / 90.0)).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        if i == 0 {
            pb.move_to(cx + r * a.cos(), cy + r * a.sin())
        } else {
            pb.line_to(cx + r * a.cos(), cy + r * a.sin())
        }
    }
    pb.finish()
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.01))
}

fn fill(pm: &mut Pixmap, path: Option<Path>, color: Color) {
    if let Some(p) = path {
        pm.fill_path(
            &p,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_path(
    pm: &mut Pixmap,
    path: Option<Path>,
    color: Color,
    width: f32,
    dash: Option<StrokeDash>,
) {
    if let Some(p) = path {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            dash,
            ..Default::default()
        };
        pm.stroke_path(&p, &paint(color), &stroke, Transform::identity(), None);
    }
}

fn polyline(points: &[(f32, f32)], cx: f32, cy: f32, u: f32, closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in points.iter().enumerate() {
        if i == 0 {
            pb.move_to(cx + x * u, cy + y * u)
        } else {
            pb.line_to(cx + x * u, cy + y * u)
        }
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

fn icon(
    pm: &mut Pixmap,
    lines: &[&[(f32, f32)]],
    cx: f32,
    cy: f32,
    u: f32,
    color: Color,
    width: f32,
) {
    for l in lines {
        stroke_path(pm, polyline(l, cx, cy, u, false), color, width * u, None);
    }
}

pub struct Fonts {
    pub bold: FontVec,
    pub regular: FontVec,
}

impl Fonts {
    pub fn load() -> Option<Self> {
        let dir = std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("Fonts");
        let read = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| std::fs::read(dir.join(n)).ok())
                .and_then(|b| FontVec::try_from_vec(b).ok())
        };
        Some(Self {
            bold: read(&["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"])?,
            regular: read(&["segoeui.ttf", "arial.ttf"])?,
        })
    }
}

fn text_width(font: &FontVec, px: f32, s: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let mut prev = None;
    let mut x = 0.0;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        x += sf.h_advance(id);
        prev = Some(id);
    }
    x
}

pub fn ellipsize(font: &FontVec, px: f32, s: &str, max_w: f32) -> String {
    if text_width(font, px, s) <= max_w {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while !chars.is_empty() {
        let keep = chars.len() / 2;
        chars.remove(keep);
        let (a, b) = chars.split_at(chars.len() / 2);
        let candidate = format!(
            "{}…{}",
            a.iter().collect::<String>(),
            b.iter().collect::<String>()
        );
        if text_width(font, px, &candidate) <= max_w {
            return candidate;
        }
    }
    "…".into()
}

#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    pm: &mut Pixmap,
    font: &FontVec,
    px: f32,
    s: &str,
    cx: f32,
    cy: f32,
    color: Rgb,
    alpha: f32,
) {
    if alpha <= 0.01 || s.is_empty() {
        return;
    }
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let x0 = cx - text_width(font, px, s) / 2.0;
    let baseline = cy + (sf.ascent() + sf.descent()) / 2.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut x = x0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(x, baseline));
        x += sf.h_advance(id);
        prev = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let px_x = b.min.x as i32 + gx as i32;
            let px_y = b.min.y as i32 + gy as i32;
            if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                return;
            }
            let a = (cov * alpha).clamp(0.0, 1.0);
            let i = ((px_y * w + px_x) * 4) as usize;
            let inv = 1.0 - a;
            for (k, c) in [color.0, color.1, color.2].into_iter().enumerate() {
                data[i + k] = (f32::from(c) * a + f32::from(data[i + k]) * inv).round() as u8;
            }
            data[i + 3] = (255.0 * a + f32::from(data[i + 3]) * inv).round() as u8;
        });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    None,
    Center,
    Slice(usize),
    Button(usize),
}

pub fn hit_wheel(scale: f32, n: usize, x: f32, y: f32) -> Hit {
    let c = SIZE * scale / 2.0;
    let (dx, dy) = (x - c, y - c);
    let r = dx.hypot(dy) / scale;
    if r <= R_IN {
        return Hit::Center;
    }
    if n == 0 || r > R_OUT + 12.0 {
        return Hit::None;
    }
    let step = TAU / n as f32;
    let a = (dy.atan2(dx) + PI / 2.0 + step / 2.0).rem_euclid(TAU);
    Hit::Slice(((a / step) as usize).min(n - 1))
}

pub fn hit_button(scale: f32, x: f32, y: f32) -> Hit {
    let c = SIZE * scale / 2.0;
    for (i, bx) in [-BUTTON_DX, BUTTON_DX].into_iter().enumerate() {
        if (x - (c + bx * scale)).hypot(y - (c + BUTTON_Y * scale)) <= (BUTTON_R + 3.0) * scale {
            return Hit::Button(i);
        }
    }
    let r = (x - c).hypot(y - c) / scale;
    if r <= R_OUT * (1.0 - STATUS_SHRINK) {
        Hit::Center
    } else {
        Hit::None
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CenterIcon {
    Drop,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Preparing,
    Converting(Option<f32>),
    Done,
    Error,
}

pub struct SliceView<'a> {
    pub label: &'a str,
    pub hover: f32,
    pub appear: f32,
}

pub struct Scene<'a> {
    pub scale: f32,
    pub open: f32,
    pub accent: Rgb,
    pub slices: &'a [SliceView<'a>],
    pub center_hover: f32,
    pub center_icon: CenterIcon,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub morph: f32,
    pub status: Option<Status>,
    pub status_t: f32,
    pub time: f32,
    pub button_hover: [f32; 2],
    pub tooltip: Option<(usize, &'a str)>,
}

pub const BUTTON_TIPS: [&str; 2] = ["Show in folder", "Copy to clipboard"];

pub fn draw(pm: &mut Pixmap, s: &Scene, fonts: &Fonts) {
    pm.fill(Color::TRANSPARENT);
    let ga = s.open.clamp(0.0, 1.0);
    if ga <= 0.0 {
        return;
    }
    let u = s.scale * (0.72 + 0.28 * ease_out_back(s.open));
    let c = SIZE * s.scale / 2.0;
    let m = s.morph.clamp(0.0, 1.0);
    let r_out = R_OUT * (1.0 - STATUS_SHRINK * m) * u;

    if let Some(shader) = RadialGradient::new(
        Point::from_xy(c, c + 6.0 * u),
        r_out * 0.9,
        Point::from_xy(c, c + 6.0 * u),
        r_out + 26.0 * u,
        vec![
            GradientStop::new(0.0, rgba((0, 0, 0), 0.42 * ga)),
            GradientStop::new(1.0, rgba((0, 0, 0), 0.0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        let p = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        if let Some(path) = circle(c, c + 6.0 * u, r_out + 26.0 * u) {
            pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        }
    }
    fill(pm, circle(c, c, r_out), rgba(BG, 0.95 * ga));

    let n = s.slices.len();
    let r0 = (R_IN + 5.0) * u * (1.0 - m);
    let r1_base = r_out - 5.0 * u * (1.0 - m);
    if n == 0 && m < 1.0 {
        fill(
            pm,
            sector(c, c, r0, r1_base, 0.0, TAU),
            rgba(SLICE, ga * (1.0 - m)),
        );
    }
    let step = if n > 0 { TAU / n as f32 } else { 0.0 };
    let gap = 0.03 * (1.0 - m);
    for (i, sl) in s.slices.iter().enumerate() {
        let a0 = -PI / 2.0 - step / 2.0 + i as f32 * step + gap / 2.0;
        let a1 = a0 + step - gap;
        let appear = ease_out_cubic(sl.appear);
        let pop = sl.hover * 8.0 * u * (1.0 - m);
        let r1 = r0 + (r1_base + pop - r0) * appear;
        let base = mix(SLICE, s.accent, sl.hover * 0.92);
        let col = mix(base, STATUS_BG, m);
        fill(
            pm,
            sector(c, c, r0, r1, a0, a1),
            rgba(col, ga * appear.max(m)),
        );
        let label_a = ga * appear * (1.0 - m * 2.5).max(0.0);
        if label_a > 0.0 {
            let mid = (a0 + a1) / 2.0;
            let lr = (r0 + r1) / 2.0 + pop * 0.3;
            let px = 15.0 * u;
            let max_w = (step * lr * 0.86).min((r1 - r0) * 1.6);
            let text = ellipsize(&fonts.bold, px, sl.label, max_w);
            draw_text(
                pm,
                &fonts.bold,
                px,
                &text,
                c + lr * mid.cos(),
                c + lr * mid.sin(),
                mix(TEXT, DARK, sl.hover),
                label_a,
            );
        }
    }

    let wheel_a = ga * (1.0 - m * 2.0).max(0.0);
    if wheel_a > 0.0 {
        let col = mix(CENTER, s.accent, s.center_hover * 0.28);
        fill(pm, circle(c, c, R_IN * u * (1.0 - m)), rgba(col, wheel_a));
        stroke_path(
            pm,
            circle(c, c, R_IN * u),
            rgba(s.accent, wheel_a * (0.15 + 0.6 * s.center_hover)),
            1.5 * u,
            None,
        );
        let ic = rgba(mix(MUTED, s.accent, 0.4 + 0.6 * s.center_hover), wheel_a);
        let iy = c - 20.0 * u;
        match s.center_icon {
            CenterIcon::Drop => icon(
                pm,
                &[
                    &[(0.0, -9.0), (0.0, 3.0)],
                    &[(-5.0, -2.0), (0.0, 3.0), (5.0, -2.0)],
                    &[(-9.0, 2.0), (-9.0, 8.0), (9.0, 8.0), (9.0, 2.0)],
                ],
                c,
                iy,
                u,
                ic,
                2.0,
            ),
            CenterIcon::Cancel => icon(
                pm,
                &[&[(-6.0, -6.0), (6.0, 6.0)], &[(6.0, -6.0), (-6.0, 6.0)]],
                c,
                iy,
                u,
                ic,
                2.0,
            ),
        }
        let t = ellipsize(&fonts.bold, 14.0 * u, s.title, 108.0 * u);
        draw_text(
            pm,
            &fonts.bold,
            14.0 * u,
            &t,
            c,
            c + 10.0 * u,
            TEXT,
            wheel_a,
        );
        let st = ellipsize(&fonts.regular, 11.5 * u, s.subtitle, 100.0 * u);
        draw_text(
            pm,
            &fonts.regular,
            11.5 * u,
            &st,
            c,
            c + 27.0 * u,
            MUTED,
            wheel_a,
        );
    }
    stroke_path(
        pm,
        circle(c, c, r_out - 0.5 * u),
        rgba((255, 255, 255), 0.07 * ga),
        1.0 * u,
        None,
    );

    if let Some(status) = s.status {
        draw_status(
            pm,
            s,
            status,
            fonts,
            c,
            u,
            r_out,
            ga * smoothstep(0.45, 1.0, m),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_status(
    pm: &mut Pixmap,
    s: &Scene,
    status: Status,
    fonts: &Fonts,
    c: f32,
    u: f32,
    r_out: f32,
    a: f32,
) {
    if a <= 0.0 {
        return;
    }
    let t = s.status_t;
    let sub = ellipsize(&fonts.regular, 12.0 * u, s.subtitle, 200.0 * u);
    match status {
        Status::Preparing => {
            let bob = (s.time * 6.0).sin() * 3.0;
            icon(
                pm,
                &[
                    &[(0.0, 10.0), (0.0, -12.0)],
                    &[(-9.0, -3.0), (0.0, -12.0), (9.0, -3.0)],
                ],
                c,
                c - 22.0 * u + bob * u,
                u,
                rgba(s.accent, a),
                3.0,
            );
            icon(
                pm,
                &[&[(-13.0, 16.0), (13.0, 16.0)]],
                c,
                c - 22.0 * u,
                u,
                rgba(MUTED, a * 0.6),
                2.5,
            );
            draw_text(pm, &fonts.bold, 18.0 * u, s.title, c, c + 26.0 * u, TEXT, a);
            draw_text(
                pm,
                &fonts.regular,
                12.0 * u,
                &sub,
                c,
                c + 48.0 * u,
                MUTED,
                a,
            );
        }
        Status::Converting(progress) => {
            let rr = r_out - 12.0 * u;
            stroke_path(
                pm,
                circle(c, c, rr),
                rgba((255, 255, 255), 0.08 * a),
                5.0 * u,
                None,
            );
            let (a0, a1) = match progress {
                Some(p) => (-PI / 2.0, -PI / 2.0 + TAU * p.max(0.004)),
                None => {
                    let base = s.time * 4.2;
                    let len = 0.9 + 0.6 * (s.time * 2.1).sin();
                    (base, base + len)
                }
            };
            stroke_path(pm, arc(c, c, rr, a0, a1), rgba(s.accent, a), 5.0 * u, None);
            let spin = s.time * 5.0;
            stroke_path(
                pm,
                arc(c, c - 26.0 * u, 13.0 * u, spin, spin + 4.4),
                rgba(s.accent, a),
                3.0 * u,
                None,
            );
            let big = progress
                .map(|p| format!("{}%", (p * 100.0).round() as u32))
                .unwrap_or_default();
            if big.is_empty() {
                draw_text(pm, &fonts.bold, 18.0 * u, s.title, c, c + 14.0 * u, TEXT, a);
            } else {
                draw_text(pm, &fonts.bold, 26.0 * u, &big, c, c + 10.0 * u, TEXT, a);
                draw_text(
                    pm,
                    &fonts.regular,
                    12.0 * u,
                    s.title,
                    c,
                    c + 32.0 * u,
                    MUTED,
                    a,
                );
            }
            draw_text(
                pm,
                &fonts.regular,
                12.0 * u,
                &sub,
                c,
                c + 52.0 * u,
                MUTED,
                a,
            );
        }
        Status::Done => {
            let pop = ease_out_back((t / 0.3).min(1.0));
            let cy = c - 28.0 * u;
            fill(pm, circle(c, cy, 25.0 * u * pop), rgba(DONE, a));
            let draw_on = ease_out_cubic(((t - 0.12) / 0.32).clamp(0.0, 1.0));
            let pts = [(-10.0, 0.5), (-3.0, 7.5), (11.0, -7.0)];
            let len = (7.0f32.hypot(7.0) + 14.0f32.hypot(14.5)) * u;
            let dash = StrokeDash::new(vec![len * draw_on + 0.01, len * 2.0], 0.0);
            stroke_path(
                pm,
                polyline(&pts, c, cy, u, false),
                rgba(DARK, a),
                4.0 * u,
                dash,
            );
            draw_text(pm, &fonts.bold, 18.0 * u, s.title, c, c + 16.0 * u, TEXT, a);
            draw_text(
                pm,
                &fonts.regular,
                12.0 * u,
                &sub,
                c,
                c + 38.0 * u,
                MUTED,
                a,
            );
            let by = c + BUTTON_Y * u;
            for (i, bx) in [-BUTTON_DX, BUTTON_DX].into_iter().enumerate() {
                let x = c + bx * u;
                let h = s.button_hover[i];
                fill(
                    pm,
                    circle(x, by, BUTTON_R * u),
                    rgba((255, 255, 255), a * (0.08 + 0.16 * h)),
                );
                let ic = rgba(mix(TEXT, DONE, h), a);
                if i == 0 {
                    stroke_path(
                        pm,
                        polyline(
                            &[
                                (-8.0, -5.5),
                                (-3.0, -5.5),
                                (-1.0, -3.0),
                                (8.0, -3.0),
                                (8.0, 6.0),
                                (-8.0, 6.0),
                            ],
                            x,
                            by,
                            u,
                            true,
                        ),
                        ic,
                        1.8 * u,
                        None,
                    );
                } else {
                    stroke_path(
                        pm,
                        polyline(
                            &[(-6.0, -5.0), (6.0, -5.0), (6.0, 8.0), (-6.0, 8.0)],
                            x,
                            by,
                            u,
                            true,
                        ),
                        ic,
                        1.8 * u,
                        None,
                    );
                    stroke_path(
                        pm,
                        polyline(
                            &[(-3.0, -8.0), (3.0, -8.0), (3.0, -3.5), (-3.0, -3.5)],
                            x,
                            by,
                            u,
                            true,
                        ),
                        ic,
                        1.8 * u,
                        None,
                    );
                }
            }
            if let Some((i, tip)) = s.tooltip {
                let px = 11.5 * u;
                let w = text_width(&fonts.regular, px, tip) + 16.0 * u;
                let x = c + [-BUTTON_DX, BUTTON_DX][i] * u;
                let ty = by + 33.0 * u;
                let a2 = a * s.button_hover[i];
                if let Some(rect) =
                    tiny_skia::Rect::from_xywh(x - w / 2.0, ty - 11.0 * u, w, 22.0 * u)
                {
                    let mut pb = PathBuilder::new();
                    pb.push_rect(rect);
                    fill(pm, pb.finish(), rgba(DARK, 0.92 * a2));
                }
                draw_text(pm, &fonts.regular, px, tip, x, ty, TEXT, a2);
            }
        }
        Status::Error => {
            let shake = (t * 38.0).sin() * (-t * 7.0).exp() * 7.0 * u;
            let cy = c - 26.0 * u;
            fill(pm, circle(c + shake, cy, 25.0 * u), rgba(ERROR, a));
            icon(
                pm,
                &[&[(-8.0, -8.0), (8.0, 8.0)], &[(8.0, -8.0), (-8.0, 8.0)]],
                c + shake,
                cy,
                u,
                rgba(DARK, a),
                4.0,
            );
            draw_text(pm, &fonts.bold, 18.0 * u, s.title, c, c + 18.0 * u, TEXT, a);
            draw_text(
                pm,
                &fonts.regular,
                12.0 * u,
                &sub,
                c,
                c + 40.0 * u,
                MUTED,
                a,
            );
        }
    }
}

pub fn app_icon(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let s = size as f32;
    let c = s / 2.0;
    fill(&mut pm, circle(c, c, s * 0.48), rgba(DARK, 1.0));
    let gap = 0.14;
    for (i, col) in PALETTE.iter().enumerate() {
        let a0 = -PI / 2.0 + i as f32 * TAU / 6.0 + gap / 2.0;
        fill(
            &mut pm,
            sector(c, c, s * 0.21, s * 0.43, a0, a0 + TAU / 6.0 - gap),
            rgba(*col, 1.0),
        );
    }
    fill(&mut pm, circle(c, c, s * 0.1), rgba(ICE, 1.0));
    pm
}

pub fn unpremultiply(pm: &Pixmap) -> Vec<u8> {
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_maps_angles_to_slices() {
        let c = SIZE / 2.0;
        assert_eq!(hit_wheel(1.0, 4, c, c), Hit::Center);
        assert_eq!(hit_wheel(1.0, 4, c, c - 120.0), Hit::Slice(0));
        assert_eq!(hit_wheel(1.0, 4, c + 120.0, c), Hit::Slice(1));
        assert_eq!(hit_wheel(1.0, 4, c, c + 120.0), Hit::Slice(2));
        assert_eq!(hit_wheel(1.0, 4, c - 120.0, c), Hit::Slice(3));
        assert_eq!(hit_wheel(1.0, 4, 2.0, 2.0), Hit::None);
        assert_eq!(
            hit_button(2.0, (c + BUTTON_DX) * 2.0, (c + BUTTON_Y) * 2.0),
            Hit::Button(1)
        );
    }
}
