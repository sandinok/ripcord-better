//! Mention notifications: in-app toast + Linux desktop notification with
//! a sound hint (the notification daemon plays it, no audio stack in the
//! app). Prefs are mirrored from the config each frame.

use std::sync::atomic::{AtomicBool, Ordering};

static SOUND: AtomicBool = AtomicBool::new(true);
static DESKTOP: AtomicBool = AtomicBool::new(true);

pub fn set_prefs(sound: bool, desktop: bool) {
    SOUND.store(sound, Ordering::Relaxed);
    DESKTOP.store(desktop, Ordering::Relaxed);
}

/// Fire a mention notification.
pub fn mention(author: &str, channel: &str, snippet: &str) {
    if DESKTOP.load(Ordering::Relaxed) {
        crate::ui::toast::show(
            crate::ui::toast::Kind::Info,
            format!("{author} mentioned you in #{channel}: {snippet}"),
        );
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
    {
        if SOUND.load(Ordering::Relaxed) {
            // The zbus blocking call runs on its own thread so it can never
            // nest a runtime inside the gateway task's runtime.
            let title = format!("{author} mentioned you in #{channel}");
            std::thread::spawn(move || {
                let _ = notify_rust::Notification::new()
                    .summary("Basalt")
                    .body(&title)
                    .icon("dialog-information")
                    .sound_name("message-new-instant")
                    .show();
            });
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
    {
        // Windows/macOS: the toast + title counter carry the notification
        // in this build; no system sound API is linked.
    }
}
