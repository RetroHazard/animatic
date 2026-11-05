mod app;
mod worker;

use std::path::PathBuf;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut db_override = std::env::var_os("ANIMATIC_DB").map(PathBuf::from);
    let settings_override = std::env::var_os("ANIMATIC_SETTINGS").map(PathBuf::from);
    let mut files = Vec::new();
    let mut full_update = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        match text.as_ref() {
            "--database" | "-d" | "/database" => {
                db_override =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--database requires a file name")
                    })?));
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

    // The default per-user directories are only consulted (never created here) when no
    // override names the location; the database and settings writers create their own parents.
    let default_dirs = || {
        directories::ProjectDirs::from("net", "AniDB", "Animatic")
            .ok_or_else(|| anyhow::anyhow!("unable to determine the default application directory; use --database and ANIMATIC_SETTINGS"))
    };
    let db_path = match db_override {
        Some(path) => path,
        None => default_dirs()?.data_dir().join("animatic.sqlite3"),
    };
    let settings_path = match settings_override {
        Some(path) => path,
        None => default_dirs()?.config_dir().join("settings.json"),
    };
    let app = app::AnimaticApp::new(db_path.clone(), settings_path.clone(), files, full_update)
        .with_context(|| {
            format!(
                "Animatic could not start (database: {}, settings: {})",
                db_path.display(),
                settings_path.display()
            )
        })?;
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
    .map_err(|e| anyhow::anyhow!("the window system failed: {e}"))
}
