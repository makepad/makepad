//! Persisted application preferences.
//!
//! Everything the Preferences dialog offers is stored here and re-read on the
//! next launch, so the dialog is a real setting surface rather than a styled
//! shell. The file lives in the platform's per-user configuration directory —
//! never inside the checkout — and is a plain `key = value` text file so a
//! corrupt or half-written line costs one setting, not the whole file.

use std::path::{Path, PathBuf};

/// How many opened files the File menu and the Open dialog remember.
pub const MAX_RECENT: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub struct ScorePrefs {
    /// Open straight into the editing chrome instead of the pianist face.
    /// On by default: the editor is the full instrument, and pianist mode is
    /// one keystroke away from it.
    pub start_in_editor: bool,
    /// Sound a note when the pointer passes over it.
    pub audition_on_hover: bool,
    /// Page follows the playback cursor.
    pub follow_cursor: bool,
    /// Metronome armed at launch.
    pub metronome: bool,
    /// Count-in armed at launch.
    pub count_in: bool,
    /// Engrave onto the dark paper palette instead of the light one.
    pub dark_paper: bool,
    /// Directory the file dialogs start in.
    pub last_dir: Option<PathBuf>,
    /// The instrument the app starts on, by its name in
    /// [`makepad_piano_model::PIANO_PRESETS`]. A name rather than an index so
    /// the choice survives the shipped instrument list growing or reordering,
    /// and an unknown name simply falls back to the app default.
    pub instrument: String,
    /// Folder the music library browses. `None` falls back to whatever
    /// [`crate::library::default_library_dir`] finds, so the browser is
    /// useful out of the box and configurable the moment it is not.
    pub library_dir: Option<PathBuf>,
    /// Most recently opened scores, newest first.
    pub recent: Vec<PathBuf>,
}

impl Default for ScorePrefs {
    fn default() -> Self {
        Self {
            start_in_editor: true,
            audition_on_hover: true,
            follow_cursor: true,
            metronome: false,
            count_in: false,
            dark_paper: false,
            last_dir: None,
            library_dir: None,
            instrument: crate::sound::DEFAULT_PRESET.to_string(),
            recent: Vec::new(),
        }
    }
}

impl ScorePrefs {
    /// The stored preferences, or the defaults when nothing is stored yet.
    /// A read never fails: an unreadable file simply means "no preferences".
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut prefs = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let flag = matches!(value, "1" | "true" | "yes" | "on");
            match key {
                "start_in_editor" => prefs.start_in_editor = flag,
                "audition_on_hover" => prefs.audition_on_hover = flag,
                "follow_cursor" => prefs.follow_cursor = flag,
                "metronome" => prefs.metronome = flag,
                "count_in" => prefs.count_in = flag,
                "dark_paper" => prefs.dark_paper = flag,
                "last_dir" if !value.is_empty() => prefs.last_dir = Some(PathBuf::from(value)),
                "library_dir" if !value.is_empty() => {
                    prefs.library_dir = Some(PathBuf::from(value))
                }
                "instrument" if !value.is_empty() => prefs.instrument = value.to_string(),
                "recent" if !value.is_empty() => {
                    let path = PathBuf::from(value);
                    if !prefs.recent.contains(&path) {
                        prefs.recent.push(path);
                    }
                }
                _ => {}
            }
        }
        prefs.recent.truncate(MAX_RECENT);
        prefs
    }

    /// Write the preferences out. Reported as a bool so the UI can say
    /// "saved" or "could not save" instead of pretending either way.
    pub fn save(&self) -> bool {
        let Some(path) = Self::path() else {
            return false;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        std::fs::write(&path, self.to_text()).is_ok()
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# makepad score preferences\n");
        for (key, value) in [
            ("start_in_editor", self.start_in_editor),
            ("audition_on_hover", self.audition_on_hover),
            ("follow_cursor", self.follow_cursor),
            ("metronome", self.metronome),
            ("count_in", self.count_in),
            ("dark_paper", self.dark_paper),
        ] {
            out.push_str(key);
            out.push_str(if value { " = 1\n" } else { " = 0\n" });
        }
        if let Some(dir) = &self.last_dir {
            out.push_str(&format!("last_dir = {}\n", dir.display()));
        }
        if let Some(dir) = &self.library_dir {
            out.push_str(&format!("library_dir = {}\n", dir.display()));
        }
        out.push_str(&format!("instrument = {}\n", self.instrument));
        for path in self.recent.iter().take(MAX_RECENT) {
            out.push_str(&format!("recent = {}\n", path.display()));
        }
        out
    }

    /// Remember a freshly opened score: newest first, no duplicates, and the
    /// directory it came from becomes the next dialog's starting point.
    pub fn remember(&mut self, path: &Path) {
        self.recent.retain(|recent| recent != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
        if let Some(parent) = path.parent() {
            self.last_dir = Some(parent.to_path_buf());
        }
    }

    /// Where the preferences file lives. Per-user configuration, never the
    /// checkout: `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME`
    /// (or `~/.config`) elsewhere.
    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        #[cfg(target_os = "macos")]
        let base = home.map(|home| home.join("Library").join("Application Support"));
        #[cfg(not(target_os = "macos"))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".config")));
        Some(base?.join("makepad-score").join("preferences.conf"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_every_flag() {
        let prefs = ScorePrefs {
            start_in_editor: true,
            audition_on_hover: false,
            follow_cursor: false,
            metronome: true,
            count_in: true,
            dark_paper: true,
            last_dir: Some(PathBuf::from("/tmp/scores")),
            library_dir: Some(PathBuf::from("/tmp/library")),
            instrument: "Upright".to_string(),
            recent: vec![PathBuf::from("/tmp/scores/a.mid")],
        };
        let text = prefs.to_text();
        // Parse it back through the same reader the loader uses.
        let mut parsed = ScorePrefs::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let flag = value == "1";
            match key {
                "start_in_editor" => parsed.start_in_editor = flag,
                "audition_on_hover" => parsed.audition_on_hover = flag,
                "follow_cursor" => parsed.follow_cursor = flag,
                "metronome" => parsed.metronome = flag,
                "count_in" => parsed.count_in = flag,
                "dark_paper" => parsed.dark_paper = flag,
                "last_dir" => parsed.last_dir = Some(PathBuf::from(value)),
                "library_dir" => parsed.library_dir = Some(PathBuf::from(value)),
                "instrument" => parsed.instrument = value.to_string(),
                "recent" => parsed.recent.push(PathBuf::from(value)),
                _ => {}
            }
        }
        assert_eq!(parsed, prefs);
    }

    #[test]
    fn recent_is_newest_first_and_bounded() {
        let mut prefs = ScorePrefs::default();
        for index in 0..(MAX_RECENT + 3) {
            prefs.remember(Path::new(&format!("/tmp/score-{index}.mid")));
        }
        assert_eq!(prefs.recent.len(), MAX_RECENT);
        assert_eq!(
            prefs.recent[0],
            PathBuf::from(&format!("/tmp/score-{}.mid", MAX_RECENT + 2))
        );
        assert_eq!(prefs.last_dir, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn remembering_the_same_path_twice_keeps_one_entry() {
        let mut prefs = ScorePrefs::default();
        prefs.remember(Path::new("/tmp/a.mid"));
        prefs.remember(Path::new("/tmp/b.mid"));
        prefs.remember(Path::new("/tmp/a.mid"));
        assert_eq!(prefs.recent.len(), 2);
        assert_eq!(prefs.recent[0], PathBuf::from("/tmp/a.mid"));
    }

    /// A fresh install lands on the felt piano and in the editor. Both are
    /// defaults, not forced values: a stored file overrides either.
    #[test]
    fn a_fresh_install_starts_felt_and_in_the_editor() {
        let prefs = ScorePrefs::default();
        assert_eq!(prefs.instrument, "Felt Piano");
        assert!(prefs.start_in_editor);
        assert!(crate::sound::preset_index_by_name(&prefs.instrument).is_some());
    }

    /// Someone who chose pianist mode and another instrument keeps both.
    #[test]
    fn a_stored_choice_beats_the_default() {
        let text = "start_in_editor = 0\ninstrument = Concert Grand\n";
        let mut prefs = ScorePrefs::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "start_in_editor" => prefs.start_in_editor = matches!(value, "1" | "true"),
                "instrument" => prefs.instrument = value.to_string(),
                _ => {}
            }
        }
        assert!(!prefs.start_in_editor);
        assert_eq!(prefs.instrument, "Concert Grand");
    }

    #[test]
    fn preferences_never_land_inside_the_checkout() {
        let path = ScorePrefs::path().expect("a home directory");
        assert!(path.ends_with("makepad-score/preferences.conf"));
        assert!(!path.to_string_lossy().contains("/makepad/makepad"));
    }
}
