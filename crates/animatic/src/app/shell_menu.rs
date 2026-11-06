//! Options row for the file-menu entry (see `animatic_core::shell_integration`).
//!
//! The entry's presence is read from the system, never from the setting: after an AOM settings
//! import that stored `integrate_into_shell = true` Options shows "not registered" until the
//! user saves. Reading and changing it happen on a short-lived thread, not on the UI thread.

use std::path::Path;
use std::sync::Arc;

use animatic_core::shell_integration::{Removal, ShellIntegration, Status};
use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui::{self, Color32, RichText};

/// What a finished check or change reports.
pub struct Finished {
    /// The registration as it is now.
    pub status: Result<Status, String>,
    /// A change that was made or refused (`Ok` is a log line, `Err` an error), if one was asked.
    pub change: Option<Result<String, String>>,
}

pub struct ShellUi {
    integration: Arc<ShellIntegration>,
    status: Option<Result<Status, String>>,
    pending: Option<Receiver<Finished>>,
    /// Options was opened and the current status was requested.
    probed: bool,
}

impl ShellUi {
    pub fn system() -> Self {
        Self::new(ShellIntegration::system())
    }

    pub fn new(integration: ShellIntegration) -> Self {
        Self {
            integration: Arc::new(integration),
            status: None,
            pending: None,
            probed: false,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Forget the shown status; the next time Options opens it is read again.
    pub fn options_closed(&mut self) {
        self.probed = false;
    }

    /// Reads the current registration once per opening of Options.
    pub fn ensure_probed(&mut self, exe: Option<&Path>) {
        if self.probed || self.pending.is_some() {
            return;
        }
        let Some(exe) = exe else { return };
        self.probed = true;
        self.status = None;
        self.spawn(exe, None);
    }

    /// An Options save: registers when `enabled`, otherwise removes Animatic's own entry.
    pub fn apply(&mut self, enabled: bool, exe: Option<&Path>) {
        let Some(exe) = exe else { return };
        self.probed = true;
        self.spawn(exe, Some(enabled));
    }

    fn spawn(&mut self, exe: &Path, change: Option<bool>) {
        let integration = self.integration.clone();
        let exe = exe.to_path_buf();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("animatic-shell".into())
            .spawn(move || {
                let _ = sender.send(run(&integration, &exe, change));
            });
        match spawned {
            Ok(_) => self.pending = Some(receiver),
            Err(e) => {
                self.status = Some(Err(format!("could not start the check: {e}")));
            }
        }
    }

    /// The outcome of the last check or change, once it finished.
    pub fn poll(&mut self) -> Option<Finished> {
        let receiver = self.pending.as_ref()?;
        let finished = match receiver.try_recv() {
            Ok(finished) => finished,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Finished {
                status: Err("the check stopped unexpectedly".into()),
                change: None,
            },
        };
        self.pending = None;
        self.status = Some(finished.status.clone());
        Some(finished)
    }

    /// The checkbox and what is really registered.
    pub fn options_ui(&self, ui: &mut egui::Ui, enabled: &mut bool, exe: Option<&Path>) {
        let unavailable = self
            .integration
            .unavailable()
            .map(str::to_string)
            .or_else(|| {
                exe.is_none()
                    .then(|| "the program's own location could not be identified".to_string())
            });
        ui.add_enabled_ui(unavailable.is_none(), |ui| {
            ui.checkbox(
                enabled,
                format!(
                    "Add \"{}\" to the file menu",
                    animatic_core::shell_integration::LABEL
                ),
            )
            .on_hover_text(self.integration.location());
        });
        if let Some(reason) = unavailable {
            ui.label(RichText::new(format!("Not available: {reason}.")).weak());
            return;
        }
        let (text, color) = self.status_text(*enabled);
        ui.label(RichText::new(text).color(color));
    }

    fn status_text(&self, enabled: bool) -> (String, Color32) {
        let weak = Color32::GRAY;
        if self.pending.is_some() {
            return ("Checking…".into(), weak);
        }
        match &self.status {
            None => ("Checking…".into(), weak),
            Some(Err(e)) => (format!("Could not check the registration: {e}"), Color32::YELLOW),
            Some(Ok(Status::Registered)) if enabled => {
                ("Registered for the current user.".into(), Color32::LIGHT_GREEN)
            }
            Some(Ok(Status::Registered)) => (
                "Registered for the current user; Save options to remove it.".into(),
                Color32::YELLOW,
            ),
            Some(Ok(Status::NotRegistered)) if enabled => (
                "Not registered yet; Save options to add it.".into(),
                Color32::YELLOW,
            ),
            Some(Ok(Status::NotRegistered)) => ("Not registered.".into(), weak),
            Some(Ok(Status::Elsewhere { command })) if enabled => (
                format!(
                    "Registered for another copy of Animatic ({command}); Save options to \
                     register this copy instead."
                ),
                Color32::YELLOW,
            ),
            Some(Ok(Status::Elsewhere { command })) => (
                format!(
                    "Registered for another copy of Animatic ({command}); Save options to \
                     remove it."
                ),
                Color32::YELLOW,
            ),
            Some(Ok(Status::Foreign)) => (
                "An entry with this name exists that Animatic did not create; it is left unchanged."
                    .into(),
                Color32::YELLOW,
            ),
        }
    }
}

/// Blocking work of one check or change.
fn run(integration: &ShellIntegration, exe: &Path, change: Option<bool>) -> Finished {
    let change = change.and_then(|enabled| {
        if enabled {
            match integration.status(exe) {
                Ok(Status::Registered) => return None,
                Err(e) => return Some(Err(e.to_string())),
                Ok(_) => {}
            }
            Some(
                integration
                    .register(exe)
                    .map(|()| {
                        format!(
                            "Added \"{}\" to the file menu",
                            animatic_core::shell_integration::LABEL
                        )
                    })
                    .map_err(|e| e.to_string()),
            )
        } else {
            match integration.unregister() {
                Ok(Removal::Removed) => Some(Ok(format!(
                    "Removed \"{}\" from the file menu",
                    animatic_core::shell_integration::LABEL
                ))),
                Ok(Removal::NotRegistered) => None,
                Ok(Removal::LeftForeign) => Some(Ok(
                    "A file-menu entry with Animatic's name exists that Animatic did not create; \
                     it was left unchanged"
                        .into(),
                )),
                Err(e) => Some(Err(e.to_string())),
            }
        }
    });
    Finished {
        status: integration.status(exe).map_err(|e| e.to_string()),
        change,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animatic_core::shell_integration::DESKTOP_FILE;
    use std::path::PathBuf;

    fn exe() -> PathBuf {
        std::env::temp_dir()
            .join("animatic-test-root")
            .join("animatic")
    }

    fn wait(ui: &mut ShellUi) -> Finished {
        let started = std::time::Instant::now();
        loop {
            if let Some(finished) = ui.poll() {
                return finished;
            }
            assert!(started.elapsed().as_secs() < 10, "no result");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn saving_the_toggle_on_and_off_registers_and_removes_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = ShellUi::new(ShellIntegration::desktop_entry(dir.path(), false));

        ui.ensure_probed(Some(&exe()));
        assert_eq!(wait(&mut ui).status, Ok(Status::NotRegistered));
        assert!(!dir.path().join(DESKTOP_FILE).exists());

        ui.apply(true, Some(&exe()));
        let on = wait(&mut ui);
        assert_eq!(on.status, Ok(Status::Registered));
        assert!(on.change.unwrap().is_ok());
        assert!(dir.path().join(DESKTOP_FILE).is_file());

        // Saving again with the toggle still on changes nothing and says nothing.
        ui.apply(true, Some(&exe()));
        assert!(wait(&mut ui).change.is_none());

        ui.apply(false, Some(&exe()));
        let off = wait(&mut ui);
        assert_eq!(off.status, Ok(Status::NotRegistered));
        assert!(off.change.unwrap().is_ok());
        assert!(!dir.path().join(DESKTOP_FILE).exists());
    }

    #[test]
    fn opening_options_only_reads_the_registration() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = ShellUi::new(ShellIntegration::desktop_entry(dir.path(), false));
        // Whatever the stored setting says, a check never registers.
        ui.ensure_probed(Some(&exe()));
        wait(&mut ui);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_foreign_entry_is_reported_and_kept_when_the_toggle_is_saved_off() {
        let dir = tempfile::tempdir().unwrap();
        let foreign = dir.path().join(DESKTOP_FILE);
        std::fs::write(&foreign, "[Desktop Entry]\nName=Somebody else\n").unwrap();
        let mut ui = ShellUi::new(ShellIntegration::desktop_entry(dir.path(), false));

        ui.apply(false, Some(&exe()));
        let off = wait(&mut ui);
        assert_eq!(off.status, Ok(Status::Foreign));
        assert!(off.change.unwrap().is_ok());
        assert_eq!(
            std::fs::read_to_string(&foreign).unwrap(),
            "[Desktop Entry]\nName=Somebody else\n"
        );

        ui.apply(true, Some(&exe()));
        assert!(wait(&mut ui).change.unwrap().is_err());
    }
}
