//! Text output of a desktop program.
//!
//! On Windows the executable uses the GUI subsystem, so starting it never opens a console
//! window. When it was started from a terminal, [`attach_parent`] attaches to that terminal's
//! console so `--help` and startup errors are still printed there. Started from Explorer or a
//! shortcut there is no console, and a startup error is shown in a message box instead.

use std::sync::atomic::{AtomicBool, Ordering};

static HAS_CONSOLE: AtomicBool = AtomicBool::new(true);

/// Attach to the console of the process that started Animatic, if it has one.
pub fn attach_parent() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: plain Win32 call without pointers; failure only means there is no console.
        let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0;
        HAS_CONSOLE.store(attached, Ordering::Relaxed);
    }
}

/// The first words of the dialog; an error that already starts with them is shown as it is.
const HEADLINE: &str = "Animatic could not start";

/// What the message box shows for `message`: the headline, then the cause, without repeating
/// the headline when the message already begins with it.
fn dialog_text(message: &str) -> String {
    if message.starts_with(HEADLINE) {
        message.to_string()
    } else {
        format!("{HEADLINE}:\n\n{message}")
    }
}

/// Report an error that stops Animatic before or instead of its window.
pub fn report_error(message: &str) {
    eprintln!("Error: {message}");
    if !HAS_CONSOLE.load(Ordering::Relaxed) {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Animatic")
            .set_description(dialog_text(message))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headline_is_shown_once() {
        assert_eq!(
            dialog_text("Animatic could not start (database: a, settings: b): disk full"),
            "Animatic could not start (database: a, settings: b): disk full"
        );
        assert_eq!(
            dialog_text("no data folder"),
            "Animatic could not start:\n\nno data folder"
        );
    }
}
