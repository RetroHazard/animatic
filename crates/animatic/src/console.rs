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

/// Report an error that stops Animatic before or instead of its window.
pub fn report_error(message: &str) {
    eprintln!("Error: {message}");
    if !HAS_CONSOLE.load(Ordering::Relaxed) {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Animatic")
            .set_description(format!("Animatic could not start:\n\n{message}"))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}
