mod app;

use std::path::PathBuf;

use fd_core::formats::Source;

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut screenshot: Option<PathBuf> = None;
    let mut shot_frames = 30u64;
    let mut open_burst: Option<usize> = None;
    let mut auto_track: Option<(f32, f32)> = None;
    let mut build_recipe: Option<PathBuf> = None;
    let mut open_harvest = false;
    let mut inspect = false;
    let mut source = Source::default();
    let mut brighten = true;
    let mut peaking = false;
    let mut ui_zoom: Option<f32> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--source" => {
                source = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--source: expected embedded|full");
                    std::process::exit(2);
                })
            }
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "--shot-frames" => {
                shot_frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(30)
            }
            "--open-burst" => open_burst = args.next().and_then(|v| v.parse().ok()),
            "--inspect" => inspect = true,
            "--brighten" => brighten = true,
            "--no-brighten" => brighten = false,
            "--peaking" => peaking = true,
            "--ui-zoom" => ui_zoom = args.next().and_then(|v| v.parse().ok()),
            "--build-recipe" => build_recipe = args.next().map(PathBuf::from),
            "--open-harvest" => open_harvest = true,
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
        eprintln!(
            "usage: fd-gui <image-folder> [--source embedded|full] [--no-brighten] [--peaking] [--ui-zoom Z]\n\
             self-test flags: --screenshot out.png --shot-frames N --open-burst N\n\
             --inspect --auto-track x,y --build-recipe out.json --open-harvest"
        );
        std::process::exit(2);
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_maximized(true)
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
                build_recipe,
                open_harvest,
                inspect,
                source,
                brighten,
                peaking,
                ui_zoom,
            )))
        }),
    )
}

/// eframe's `default_fonts` feature is easy to lose when trimming
/// `default-features`; without it egui draws no text at all and the whole UI
/// goes blank with no error.
#[test]
fn default_fonts_are_embedded() {
    assert!(!eframe::egui::FontDefinitions::default().font_data.is_empty());
}
