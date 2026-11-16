use std::f32::consts::{PI, TAU};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Transform};

pub const PALETTE: [(u8, u8, u8); 6] =
    [(125, 211, 252), (129, 140, 248), (192, 132, 252), (244, 114, 182), (251, 191, 36), (110, 231, 183)];

pub fn rgba(c: (u8, u8, u8), a: f32) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

pub fn sector(cx: f32, cy: f32, r0: f32, r1: f32, a0: f32, a1: f32) -> Option<Path> {
    let steps = (((a1 - a0).abs() / (PI / 120.0)).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        let (x, y) = (cx + r1 * a.cos(), cy + r1 * a.sin());
        if i == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    for i in (0..=steps).rev() {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        pb.line_to(cx + r0 * a.cos(), cy + r0 * a.sin());
    }
    pb.close();
    pb.finish()
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.01))
}

pub fn app_icon(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("icon size");
    let s = size as f32;
    let c = s / 2.0;
    if let Some(bg) = circle(c, c, s * 0.48) {
        pm.fill_path(&bg, &paint(rgba((24, 26, 38), 1.0)), FillRule::Winding, Transform::identity(), None);
    }
    let gap = 0.12;
    for (i, col) in PALETTE.iter().enumerate() {
        let a0 = -PI / 2.0 + i as f32 * TAU / 6.0 + gap / 2.0;
        if let Some(p) = sector(c, c, s * 0.2, s * 0.44, a0, a0 + TAU / 6.0 - gap) {
            pm.fill_path(&p, &paint(rgba(*col, 1.0)), FillRule::Winding, Transform::identity(), None);
        }
    }
    if let Some(dot) = circle(c, c, s * 0.1) {
        pm.fill_path(&dot, &paint(rgba((240, 244, 255), 1.0)), FillRule::Winding, Transform::identity(), None);
    }
    pm
}

pub fn unpremultiply(pm: &Pixmap) -> Vec<u8> {
    pm.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}
