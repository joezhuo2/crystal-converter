#![windows_subsystem = "windows"]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--settings") => crystal_converter::settings::run(args.get(1).map(String::as_str)),
        Some("--open") => crystal_converter::app::run(args[1..].iter().map(Into::into).collect()),
        _ => crystal_converter::app::run(Vec::new()),
    }
}
