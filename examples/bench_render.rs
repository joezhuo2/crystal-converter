use crystal_converter::render::{self, CenterIcon, Fonts, Scene, SliceView};
use std::time::Instant;
use tiny_skia::Pixmap;

fn main() {
    let fonts = Fonts::load().unwrap();
    let scale = 2.0;
    let size = (render::SIZE * scale) as u32;
    let mut pm = Pixmap::new(size, size).unwrap();
    let labels = ["MP4", "WEBM", "MKV", "AVI", "GIF", "MP3", "M4A"];
    let views: Vec<SliceView> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| SliceView {
            label: l,
            hover: if i == 0 { 1.0 } else { 0.0 },
            appear: 1.0,
        })
        .collect();
    let scene = Scene {
        scale,
        open: 1.0,
        accent: render::CONVERT_ACCENT,
        slices: &views,
        center_hover: 0.0,
        center_icon: CenterIcon::Drop,
        title: "Video",
        subtitle: "MOV",
        morph: 0.0,
        status: None,
        status_t: 0.0,
        time: 0.0,
        button_hover: [0.0; 2],
        tooltip: None,
    };
    let n = 60;
    let base = render::background(scale, 0.0);
    for (name, b) in [("full", None), ("cached base", base.as_ref())] {
        let t = Instant::now();
        for _ in 0..n {
            render::draw(&mut pm, &scene, &fonts, b);
        }
        println!(
            "{name}: {:.2} ms/frame",
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        );
    }
}
