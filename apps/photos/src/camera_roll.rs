//! The camera roll: the JPEGs the Camera app writes, put on the wall.
//!
//! Camera writes every shot into `<makepad home>/photos/camera` (whole, under
//! a `.part` name first; apps/camera/src/roll.rs keeps the same path), named
//! by capture time so names sort in shot order. The wall looks at that
//! folder when it opens and then once a second (one `stat` of the folder;
//! the listing only when the folder changed), and bakes the shots it has not
//! seen into the open library — or, with no library anywhere, into the
//! home's own (`<makepad home>/photos/image-tiles/photos`), so a phone's
//! first shot makes the wall. The bake is the same one `photos.add` runs;
//! a shot already in the library keeps its pixels.

use makepad_image_tiles::bake::Source;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The camera roll under `home` (the makepad home).
pub fn roll_dir_in(home: &Path) -> PathBuf {
    home.join("photos").join("camera")
}

/// The camera roll of this user.
pub fn roll_dir() -> PathBuf {
    roll_dir_in(&makepad_widgets::makepad_platform::home::makepad_home())
}

/// Whether `name` is a finished shot (not a `.part` being written).
pub fn is_shot(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".jpg") || lower.ends_with(".jpeg")) && !name.starts_with('.')
}

/// What the wall knows of the roll: the folder's last change and the shots
/// it has handed to a bake.
#[derive(Default)]
pub struct RollWatch {
    stamp: Option<SystemTime>,
    seen: HashSet<String>,
}

impl RollWatch {
    /// The shots in `dir` the wall has not seen, in shot order; they count
    /// as seen from now on. Empty without a listing when the folder has not
    /// changed since the last look (or is not there).
    pub fn new_shots(&mut self, dir: &Path) -> Vec<PathBuf> {
        let Ok(stamp) = std::fs::metadata(dir).and_then(|m| m.modified()) else { return Vec::new() };
        // A folder changed within the last two seconds is listed again: a
        // coarse filesystem clock can give two changes one stamp.
        let settled = SystemTime::now().duration_since(stamp).is_ok_and(|age| age.as_secs_f64() > 2.0);
        if self.stamp == Some(stamp) && settled {
            return Vec::new();
        }
        self.stamp = Some(stamp);
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| is_shot(name) && !self.seen.contains(name))
            .collect();
        names.sort();
        self.seen.extend(names.iter().cloned());
        names.into_iter().map(|name| dir.join(name)).collect()
    }

    /// Forget `paths` again (their bake could not start): the next look
    /// offers them once more.
    pub fn unsee(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                self.seen.remove(name);
            }
            self.stamp = None;
        }
    }
}

/// The bake sources for shots: the path is the URL and the link (the wall
/// finds a shot by it), the file name the title.
pub fn sources(shots: &[PathBuf]) -> Vec<Source> {
    shots
        .iter()
        .map(|path| {
            let text = path.to_string_lossy().to_string();
            let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| text.clone());
            Source { url: text.clone(), title, link: text }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roll_sits_beside_the_photo_libraries() {
        assert_eq!(roll_dir_in(Path::new("/h/.makepad")), Path::new("/h/.makepad/photos/camera"));
    }

    #[test]
    fn new_shots_come_once_in_shot_order_and_half_written_ones_never() {
        let dir = std::env::temp_dir().join(format!("photos-roll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut watch = RollWatch::default();
        // No roll yet: nothing, and no error.
        assert!(watch.new_shots(&dir).is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IMG_20261013_100002_000.jpg"), b"b").unwrap();
        std::fs::write(dir.join("IMG_20261013_100001_000.jpg"), b"a").unwrap();
        std::fs::write(dir.join("IMG_20261013_100003_000.jpg.part"), b"half").unwrap();
        let first = watch.new_shots(&dir);
        let names: Vec<_> = first.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["IMG_20261013_100001_000.jpg", "IMG_20261013_100002_000.jpg"]);
        // Nothing new (the folder was just written: it is listed again).
        assert!(watch.new_shots(&dir).is_empty());
        // The half-written shot is finished (a rename changes the folder).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::rename(dir.join("IMG_20261013_100003_000.jpg.part"), dir.join("IMG_20261013_100003_000.jpg")).unwrap();
        let next = watch.new_shots(&dir);
        assert_eq!(next, vec![dir.join("IMG_20261013_100003_000.jpg")]);
        // A bake that could not start offers its shots again.
        watch.unsee(&next);
        assert_eq!(watch.new_shots(&dir), next);
        let sources = sources(&next);
        assert_eq!(sources[0].title, "IMG_20261013_100003_000");
        assert_eq!(sources[0].url, sources[0].link);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
