mod app;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut screenshot: Option<PathBuf> = None;
    let mut shot_frames = 30u64;
    let mut open_burst: Option<usize> = None;
    let mut auto_track: Option<(f32, f32)> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "--shot-frames" => {
                shot_frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(30)
            }
            "--open-burst" => open_burst = args.next().and_then(|v| v.parse().ok()),
            "--auto-track" => {
                auto_track = args.next().and_then(|v| {
                    let (x, y) = v.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                })
            }
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: fd-gui <image-folder> [--screenshot out.png --shot-frames N]");
        std::process::exit(2);
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 1000.0])
            .with_title("fast_deduplicator"),
        ..Default::default()
    };
    eframe::run_native(
        "fast_deduplicator",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::App::new(
                cc,
                dir,
                screenshot,
                shot_frames,
                open_burst,
                auto_track,
            )))
        }),
    )
}
