mod app;
mod worker;

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dirs = directories::ProjectDirs::from("net", "AniDB", "Animatic")
        .ok_or_else(|| anyhow::anyhow!("unable to determine application data directory"))?;
    let data = dirs.data_dir();
    std::fs::create_dir_all(data)?;
    let mut db_path = std::env::var_os("ANIMATIC_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("animatic.sqlite3"));
    let settings_path = std::env::var_os("ANIMATIC_SETTINGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.config_dir().join("settings.json"));
    let mut files = Vec::new();
    let mut full_update = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        match text.as_ref() {
            "--database" | "-d" | "/database" => {
                db_path = PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--database requires a file name"))?,
                );
            }
            "--full-update" | "/fullupdate" => full_update = true,
            "--help" | "-h" | "/?" => {
                println!("Animatic [--database FILE] [--full-update] [MEDIA FILES...]");
                return Ok(());
            }
            "/utf8" | "/ucs2" => {}
            _ if text.starts_with('-') => return Err(anyhow::anyhow!("unknown option: {text}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }

    let app = app::AnimaticApp::new(db_path, settings_path, files, full_update)?;
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Animatic — AniDB library manager")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Animatic",
        native,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
