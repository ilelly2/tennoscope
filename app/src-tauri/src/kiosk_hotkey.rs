//! The Ducat Kiosk price hotkey.
//!
//! The overlay already shows itself when the kiosk opens and re-reads the grid whenever it
//! settles, so the key exists for the moments that loop gets wrong: prices that faded and did
//! not come back, or prices the player wants out of the way. One press brings the prices up and
//! asks the poller for a read right now, skipping the settle wait; the next press hides them.
//! Hiding lasts for the visit: the next kiosk visit starts shown, as the overlay always has.
//!
//! Registered from Rust, not the webview, so the key works while the main window is closed and
//! no JS binding or capability is involved. The key is the player's to choose; there is no
//! default, because any key is one some player already has bound in the game.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcut, Shortcut, ShortcutState};

use crate::kiosk_view::KioskState;
use crate::overlay_window;

/// Where a kiosk visit's prices stand with respect to the hotkey.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    /// The visit as the overlay runs it on its own: shown, reading on every settle.
    #[default]
    Automatic,
    /// The player pressed the key to bring the prices up.
    Shown,
    /// The player pressed the key to put them away.
    Hidden,
}

/// What one press of the key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// No kiosk on screen: the key belongs to whatever the player is doing instead.
    Ignore,
    Hide,
    ShowAndRefresh,
}

/// The hotkey's half of a kiosk visit, shared by the monitor (which opens visits) and the key
/// handler. `refresh` is the poller's read-now request.
#[derive(Default)]
pub struct KioskToggle {
    mode: Mutex<Mode>,
    refresh: Arc<AtomicBool>,
}

impl KioskToggle {
    /// A new visit starts the way the overlay always has, shown, and with no stale read-now
    /// request left over from the last one. The returned flag is the new poller's.
    pub fn begin_visit(&self) -> Arc<AtomicBool> {
        if let Ok(mut mode) = self.mode.lock() {
            *mode = Mode::Automatic;
        }
        self.refresh.store(false, Ordering::Release);
        Arc::clone(&self.refresh)
    }

    /// The player put the prices away; nothing else may bring them back this visit.
    pub fn hidden(&self) -> bool {
        self.mode.lock().is_ok_and(|mode| *mode == Mode::Hidden)
    }

    /// Advance on a key press. The first press of a visit always shows and re-reads, since the
    /// usual reason to reach for the key is prices that are missing, not prices to hide.
    pub fn press(&self, visit_live: bool) -> Press {
        if !visit_live {
            return Press::Ignore;
        }
        let Ok(mut mode) = self.mode.lock() else {
            return Press::Ignore;
        };
        let press = match *mode {
            Mode::Automatic | Mode::Hidden => Press::ShowAndRefresh,
            Mode::Shown => Press::Hide,
        };
        *mode = match press {
            Press::ShowAndRefresh => Mode::Shown,
            Press::Hide => Mode::Hidden,
            Press::Ignore => *mode,
        };
        if press == Press::ShowAndRefresh {
            self.refresh.store(true, Ordering::Release);
        }
        press
    }
}

/// A stored or typed accelerator, trimmed; blank means no hotkey.
pub fn normalize(accelerator: Option<&str>) -> Option<String> {
    accelerator
        .map(str::trim)
        .filter(|accelerator| !accelerator.is_empty())
        .map(str::to_owned)
}

/// Whether the key parses as something Windows can register, before anything is unregistered.
pub fn parse(accelerator: &str) -> Result<Shortcut, String> {
    Shortcut::from_str(accelerator).map_err(|_| format!("\"{accelerator}\" is not a usable key"))
}

/// Make `accelerator` the one kiosk hotkey, or remove it with `None`. The app holds only this
/// shortcut, so clearing all of them is clearing the previous key.
pub fn register(app: &AppHandle, accelerator: Option<&str>) -> Result<(), String> {
    let shortcut = accelerator.map(parse).transpose()?;
    let Some(shortcuts) = app.try_state::<GlobalShortcut<tauri::Wry>>() else {
        return match shortcut {
            None => Ok(()),
            Some(_) => Err("hotkeys are not available on this system".to_owned()),
        };
    };
    let _ = shortcuts.unregister_all();
    let Some(shortcut) = shortcut else {
        return Ok(());
    };
    shortcuts
        .on_shortcut(shortcut, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                on_press(app);
            }
        })
        .map_err(|_| "that key is already taken by Windows or another program".to_owned())
}

fn on_press(app: &AppHandle) {
    // Off the event loop: hiding waits on the main thread, and this runs on it.
    let app = app.clone();
    std::thread::spawn(move || {
        let visit_live = app
            .try_state::<KioskState>()
            .is_some_and(|kiosk| kiosk.active_session().is_some());
        let Some(toggle) = app.try_state::<KioskToggle>() else {
            return;
        };
        let press = toggle.press(visit_live);
        log::debug!("[DEBUG-kiosk] hotkey {press:?}");
        match press {
            Press::Ignore => {}
            Press::Hide => overlay_window::hide_kiosk_overlay(&app),
            Press::ShowAndRefresh => overlay_window::show_kiosk_overlay(&app),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_press_of_a_visit_shows_and_reads_then_presses_alternate() {
        let toggle = KioskToggle::default();
        let refresh = toggle.begin_visit();
        assert_eq!(toggle.press(true), Press::ShowAndRefresh);
        assert!(
            refresh.swap(false, Ordering::AcqRel),
            "the poller is asked to read now"
        );
        assert!(!toggle.hidden());
        assert_eq!(toggle.press(true), Press::Hide);
        assert!(toggle.hidden());
        assert!(!refresh.load(Ordering::Acquire), "hiding asks for no read");
        assert_eq!(toggle.press(true), Press::ShowAndRefresh);
        assert!(!toggle.hidden());
        assert!(refresh.load(Ordering::Acquire));
    }

    #[test]
    fn the_key_does_nothing_without_a_kiosk_on_screen() {
        let toggle = KioskToggle::default();
        let refresh = toggle.begin_visit();
        assert_eq!(toggle.press(false), Press::Ignore);
        assert!(!refresh.load(Ordering::Acquire));
        assert_eq!(
            toggle.press(true),
            Press::ShowAndRefresh,
            "an ignored press does not advance the toggle"
        );
    }

    #[test]
    fn a_new_visit_starts_shown_with_no_leftover_read_request() {
        let toggle = KioskToggle::default();
        let refresh = toggle.begin_visit();
        toggle.press(true);
        toggle.press(true);
        assert!(toggle.hidden());
        refresh.store(true, Ordering::Release);

        let next = toggle.begin_visit();
        assert!(!toggle.hidden(), "hiding lasts one visit");
        assert!(!next.load(Ordering::Acquire));
        assert_eq!(toggle.press(true), Press::ShowAndRefresh);
    }

    #[test]
    fn keys_parse_and_nonsense_is_refused_in_words() {
        for key in ["F8", "Ctrl+Shift+P", "Alt+KeyK", "Numpad5"] {
            assert!(parse(key).is_ok(), "{key} parses");
        }
        assert_eq!(
            parse("Ctrl+Banana").unwrap_err(),
            "\"Ctrl+Banana\" is not a usable key"
        );
        assert_eq!(normalize(Some("  F8 ")).as_deref(), Some("F8"));
        assert_eq!(normalize(Some("   ")), None);
        assert_eq!(normalize(None), None);
    }
}
