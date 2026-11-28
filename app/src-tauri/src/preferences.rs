use std::io::Write as _;
use std::path::Path;

use atomicwrites::{AtomicFile, OverwriteBehavior};
use serde::{Deserialize, Serialize};

/// Overlay preferences the backend owns, so the overlays never read browser storage for them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Preferences {
    #[serde(default = "marks_on")]
    pub mastery_marks: bool,
    /// The Ducat Kiosk price hotkey as an accelerator (`F8`, `Ctrl+Shift+P`). None until the
    /// player picks one: any default would be a key some player already uses in the game.
    #[serde(default)]
    pub kiosk_hotkey: Option<String>,
}

fn marks_on() -> bool {
    true
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            mastery_marks: true,
            kiosk_hotkey: None,
        }
    }
}

/// A missing, unreadable, or corrupt file reads as marks on. The marks are the default
/// experience, and a broken file must not silently turn a feature off.
pub fn load_preferences(path: &Path) -> Preferences {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Written atomically like the setup status: a crash mid-write leaves the previous file,
/// never half a JSON document.
pub fn save_preferences(path: &Path, preferences: &Preferences) -> std::io::Result<()> {
    let serialized = serde_json::to_vec(preferences).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    AtomicFile::new(path, OverwriteBehavior::AllowOverwrite)
        .write(|file| file.write_all(&serialized).and_then(|_| file.sync_all()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_corrupt_file_reads_as_marks_on() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tennoscope-preferences.json");
        assert!(load_preferences(&path).mastery_marks);
        std::fs::write(&path, b"{not json").unwrap();
        assert!(load_preferences(&path).mastery_marks);
        std::fs::write(&path, b"{}").unwrap();
        assert!(
            load_preferences(&path).mastery_marks,
            "a missing key keeps the default"
        );
    }

    #[test]
    fn a_saved_choice_reads_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tennoscope-preferences.json");
        save_preferences(
            &path,
            &Preferences {
                mastery_marks: false,
                kiosk_hotkey: Some("Ctrl+Shift+P".to_owned()),
            },
        )
        .unwrap();
        let loaded = load_preferences(&path);
        assert!(!loaded.mastery_marks);
        assert_eq!(loaded.kiosk_hotkey.as_deref(), Some("Ctrl+Shift+P"));
    }

    /// Files written before the hotkey existed hold only the marks; they must keep the marks
    /// choice and come back with no hotkey, not fall back to defaults wholesale.
    #[test]
    fn a_file_from_before_the_hotkey_keeps_its_marks_and_has_no_hotkey() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tennoscope-preferences.json");
        std::fs::write(&path, br#"{"mastery_marks":false}"#).unwrap();
        let loaded = load_preferences(&path);
        assert!(!loaded.mastery_marks);
        assert_eq!(loaded.kiosk_hotkey, None);
    }
}
