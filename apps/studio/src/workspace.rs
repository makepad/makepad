//! The workspace mode shared by the shell and its presentations, persisted
//! per state directory, and the camera the zoomable presentations project
//! through. Documents, terminals and processes are owned by the Dock items;
//! nothing here refers to them.

use makepad_widgets::makepad_micro_serde::*;
use std::path::Path;

pub const MIN_ZOOM: f64 = 0.00001;
pub const MAX_ZOOM: f64 = 8.0;
const MAX_COORD: f64 = 10_000_000.0;
const STATE_FILE: &str = "workspace.ron";
const MAX_STATE_BYTES: usize = 4096;

/// The four workspace modes: the docked IDE, the tasks view (agent lanes
/// with their terminals on the screen host), the Architecture map, and Disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerRon, DeRon)]
pub enum Mode {
    #[default]
    Structured,
    Tasks,
    Architecture,
    Disk,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structured => "structured",
            Self::Tasks => "tasks",
            Self::Architecture => "architecture",
            Self::Disk => "disk",
        }
    }
}

/// Pan is in viewport pixels; world coordinates are in points.
/// screen = viewport_origin + pan + world * zoom.
#[derive(Clone, Copy, Debug, PartialEq, SerRon, DeRon)]
pub struct Camera {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan_x: 32.0,
            pan_y: 32.0,
            zoom: 0.7,
        }
    }
}

impl Camera {
    pub fn is_valid(self) -> bool {
        self.pan_x.is_finite()
            && self.pan_y.is_finite()
            && self.zoom.is_finite()
            && self.pan_x.abs() <= MAX_COORD
            && self.pan_y.abs() <= MAX_COORD
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
    }
}

/// A world-space rectangle a zoomable presentation projects through its camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(SerRon, DeRon)]
struct StoredWorkspace {
    version: u32,
    mode: Mode,
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub mode: Mode,
}

impl Workspace {
    pub fn encode(&self) -> String {
        StoredWorkspace {
            version: 3,
            mode: self.mode,
        }
        .serialize_ron()
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        if text.len() > MAX_STATE_BYTES {
            return Err("Workspace state is too large".into());
        }
        // Unlike the convenience deserialize_ron method, reject trailing data.
        let mut state = DeRonState::default();
        let mut chars = text.chars();
        state.next(&mut chars);
        state.next_tok(&mut chars).map_err(|e| format!("{e:?}"))?;
        let stored =
            StoredWorkspace::de_ron(&mut state, &mut chars).map_err(|e| format!("{e:?}"))?;
        if state.tok != DeRonTok::Eof {
            return Err("Unexpected data after workspace state".into());
        }
        if stored.version != 2 && stored.version != 3 {
            return Err("Unsupported workspace state version".into());
        }
        Ok(Self { mode: stored.mode })
    }

    /// Corrupt or missing state starts in Structured mode.
    pub fn load(dir: &Path) -> Self {
        use std::io::Read;
        let text = (|| -> std::io::Result<String> {
            let file = std::fs::File::open(dir.join(STATE_FILE))?;
            let mut text = String::new();
            file.take((MAX_STATE_BYTES + 1) as u64)
                .read_to_string(&mut text)?;
            Ok(text)
        })();
        text.ok()
            .and_then(|text| Self::decode(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind, Write};
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let text = self.encode();
        Self::decode(&text).map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!(
            ".{STATE_FILE}.{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let result = (|| {
            file.write_all(text.as_bytes())?;
            drop(file);
            std::fs::rename(&path, dir.join(STATE_FILE))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_round_trips_and_rejects_foreign_state() {
        for mode in [Mode::Structured, Mode::Tasks, Mode::Architecture, Mode::Disk] {
            let text = Workspace { mode }.encode();
            assert_eq!(Workspace::decode(&text).unwrap().mode, mode);
        }
        assert_eq!(
            Workspace::decode("(version:2,mode:Tasks)").unwrap().mode,
            Mode::Tasks
        );
        assert!(Workspace::decode("(version:1,mode:Structured)").is_err());
        assert!(Workspace::decode("(version:2,mode:Tasks)garbage").is_err());
        assert_eq!(Workspace::decode("nonsense").err().is_some(), true);
    }

    #[test]
    fn load_falls_back_to_structured() {
        let dir = std::env::temp_dir().join(format!("studio-workspace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Workspace::load(&dir).mode, Mode::Structured);
        Workspace { mode: Mode::Tasks }.save(&dir).unwrap();
        assert_eq!(Workspace::load(&dir).mode, Mode::Tasks);
        std::fs::write(dir.join(STATE_FILE), "(version:9,mode:Tasks)").unwrap();
        assert_eq!(Workspace::load(&dir).mode, Mode::Structured);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
