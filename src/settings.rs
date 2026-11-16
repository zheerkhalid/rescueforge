//! User settings, stored as JSON in `%LOCALAPPDATA%\RescueForge\settings.json`.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Dark,
    Light,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Start with "Deep scan" selected.
    pub deep_default: bool,
    /// Deep scan: skip space that a working file system reports as in use.
    pub free_only: bool,
    /// Deep scan: look for partitions that are no longer listed.
    pub find_partitions: bool,
    /// List files that were never deleted as well.
    pub include_existing: bool,
    /// Ids of file types the deep scan does not look for.
    pub carve_off: Vec<String>,
    /// Decode pictures for thumbnails and the preview panel.
    pub thumbnails: bool,
    /// Recreate the original folders when recovering.
    pub keep_folders: bool,
    /// Read recovered files back to check them.
    pub verify: bool,
    /// The first-run guide has been shown.
    pub welcome_seen: bool,
    /// Disk images opened recently (newest first).
    pub recent_images: Vec<String>,
    pub last_destination: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Dark,
            deep_default: false,
            free_only: true,
            find_partitions: true,
            include_existing: false,
            carve_off: crate::carve::default_off(),
            thumbnails: true,
            keep_folders: true,
            verify: true,
            welcome_seen: false,
            recent_images: Vec::new(),
            last_destination: String::new(),
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Settings {
        std::fs::read_to_string(dir.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("settings.json"), json)
            .map_err(|e| format!("Could not save settings: {e}"))
    }

    pub fn remember_image(&mut self, path: &str) {
        self.recent_images.retain(|p| !p.eq_ignore_ascii_case(path));
        self.recent_images.insert(0, path.to_string());
        self.recent_images.truncate(6);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerant_loading() {
        let dir = std::env::temp_dir().join(format!("rescueforge-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Settings::load(&dir), Settings::default());
        let mut s = Settings {
            theme: ThemeChoice::Light,
            deep_default: true,
            carve_off: vec!["exe".into()],
            ..Default::default()
        };
        s.remember_image("E:\\a.img");
        s.remember_image("E:\\b.img");
        s.remember_image("e:\\A.IMG");
        assert_eq!(s.recent_images, vec!["e:\\A.IMG", "E:\\b.img"]);
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        // Partial files keep defaults for the rest; broken files fall back entirely.
        std::fs::write(
            dir.join("settings.json"),
            r#"{"theme":"System","future_option":true}"#,
        )
        .unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.theme, ThemeChoice::System);
        assert!(loaded.free_only);
        std::fs::write(dir.join("settings.json"), "{{{").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
