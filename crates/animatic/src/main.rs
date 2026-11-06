mod app;
mod hash_jobs;
mod media;
mod scheduler;
mod table;
mod tray;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    // Captured first, before anything can replace the file: the updater installs over exactly
    // this file (checked by identity, not just by path) and the restart starts exactly this path.
    let installation = match animatic_core::update::InstallationTarget::capture() {
        Ok(target) => Some(Arc::new(target)),
        Err(e) => {
            log::warn!("updates are unavailable, the executable could not be identified: {e}");
            None
        }
    };
    let mut db_override = std::env::var_os("ANIMATIC_DB").map(PathBuf::from);
    let settings_override = std::env::var_os("ANIMATIC_SETTINGS")
        .map(PathBuf::from)
        .map(|path| std::path::absolute(&path).unwrap_or(path));
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
    // Absolute from here on, so the restart opens the very same database even if a file dialog
    // changed the working directory meanwhile.
    let db_path = match db_override {
        Some(path) => path,
        None => default_dirs()?.data_dir().join("animatic.sqlite3"),
    };
    let db_path = std::path::absolute(&db_path).unwrap_or(db_path);
    // Media files go to the Animatic that already has this database open. Without files (or
    // with `--full-update`, which a handoff cannot carry) nothing is forwarded; this process
    // then owns the lock, or starts without one if another instance holds it.
    let handoff: &[PathBuf] = if full_update { &[] } else { &files };
    let primary = match animatic_core::instance::start(&db_path, handoff) {
        animatic_core::instance::Launch::Forwarded => return Ok(()),
        animatic_core::instance::Launch::Primary(primary) => Some(primary),
        animatic_core::instance::Launch::Standalone => None,
    };
    let relaunch_settings = settings_override.clone();
    let settings_path = match settings_override {
        Some(path) => path,
        None => default_dirs()?.config_dir().join("settings.json"),
    };
    let restart = Arc::new(AtomicBool::new(false));
    let app = app::AnimaticApp::new(app::Startup {
        db_path: db_path.clone(),
        settings_path: settings_path.clone(),
        files,
        full_update,
        installation: installation.clone(),
        restart: restart.clone(),
        credentials: animatic_core::secrets::Credentials::system(),
        incoming: primary.as_ref().map(|primary| primary.incoming()),
    })
    .with_context(|| {
        format!(
            "Animatic could not start (database: {}, settings: {})",
            db_path.display(),
            settings_path.display()
        )
    })?;
    let native = eframe::NativeOptions {
        viewport: app.viewport(),
        ..Default::default()
    };
    // The app (and with it the worker and database connections) is dropped when this returns.
    eframe::run_native(
        "Animatic",
        native,
        Box::new(|cc| {
            app.apply_visuals(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("the window system failed: {e}"))?;
    // Release the lock before the updated version starts, or it would hand its files to us.
    drop(primary);
    if restart.load(Ordering::SeqCst) {
        let target = installation.context("the executable was not identified at startup")?;
        relaunch(target.path(), &db_path, relaunch_settings.as_deref()).context(
            "Animatic was updated but could not start the new version; start it manually",
        )?;
    }
    Ok(())
}

/// Starts the updated executable against the database that was open. Nothing else is passed on:
/// not the media files (they would be hashed again), not `--full-update`. A settings location
/// given through `ANIMATIC_SETTINGS` is handed on as the absolute path that was in use.
fn relaunch(executable: &Path, db_path: &Path, settings: Option<&Path>) -> std::io::Result<()> {
    let mut command = std::process::Command::new(executable);
    command.arg("--database").arg(db_path);
    if let Some(settings) = settings {
        command.env("ANIMATIC_SETTINGS", settings);
    }
    command.spawn().map(|_| ())
}
