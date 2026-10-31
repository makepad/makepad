//! The camera roll: the folder the shots go to and their names.
//!
//! The roll is `<makepad home>/photos/camera`, next to the Photos app's own
//! libraries (`<makepad home>/photos/image-tiles`), so every makepad app of
//! the person reaches it: on a phone the WM's apps all run in one process
//! with one home (the app's private files), on a desktop it is `~/.makepad`.
//! Photos reads the same folder (apps/photos/src/camera_roll.rs keeps the
//! same path) and puts every new shot on its wall.
//!
//! A shot is named by its capture time on the local wall clock,
//! `IMG_20261013_104512_345.jpg` (date, time, milliseconds), so the names
//! sort in the order the shots were taken. A shot is written whole under a
//! `.part` name first and renamed, so a reader never sees half a JPEG.

use makepad_civil_time::LocalDateTime;
use std::path::{Path, PathBuf};

/// The camera roll under `home` (the makepad home).
pub fn roll_dir_in(home: &Path) -> PathBuf {
    home.join("photos").join("camera")
}

/// The camera roll of this user.
pub fn roll_dir() -> PathBuf {
    roll_dir_in(&makepad_widgets::makepad_platform::home::makepad_home())
}

/// The name a shot taken at `time` (plus `millis` into that second) gets.
pub fn shot_name(time: &LocalDateTime, millis: u32) -> String {
    format!(
        "IMG_{:04}{:02}{:02}_{:02}{:02}{:02}_{:03}.jpg",
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
        millis.min(999)
    )
}

/// A path in `dir` for `name` that no shot has yet: `name` itself, else
/// `<stem>_2.jpg`, `<stem>_3.jpg`, ... (two shots in one millisecond).
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = name.strip_suffix(".jpg").unwrap_or(name);
    (2..)
        .map(|n| dir.join(format!("{stem}_{n}.jpg")))
        .find(|path| !path.exists())
        .unwrap()
}

/// Whether `name` is a finished shot (not a `.part` being written).
pub fn is_shot(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.ends_with(".jpg") || lower.ends_with(".jpeg")) && !name.starts_with('.')
}

/// The newest shot in `dir`: the last name in sort order.
pub fn latest_shot(dir: &Path) -> Option<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_shot(name))
        .collect();
    names.sort();
    names.pop().map(|name| dir.join(name))
}

/// Write `jpeg` as a new shot named for `time` into `dir` (made when
/// missing): whole under a `.part` name, then renamed into place.
pub fn save_shot(dir: &Path, time: &LocalDateTime, millis: u32, jpeg: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make the camera roll {}: {e}", dir.display()))?;
    let path = free_path(dir, &shot_name(time, millis));
    let mut part = path.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, jpeg).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("cannot finish {}: {e}", path.display())
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, minute: u32, second: u32) -> LocalDateTime {
        LocalDateTime { year: 2026, month: 10, day: 13, hour, minute, second, offset: 7200, zoned: true }
    }

    #[test]
    fn names_carry_the_capture_time_and_sort_in_shot_order() {
        assert_eq!(shot_name(&at(10, 45, 12), 345), "IMG_20261013_104512_345.jpg");
        assert_eq!(shot_name(&at(9, 5, 2), 7), "IMG_20261013_090502_007.jpg");
        let mut names = vec![shot_name(&at(10, 45, 12), 345), shot_name(&at(9, 5, 2), 7), shot_name(&at(10, 45, 12), 9)];
        names.sort();
        assert_eq!(names, ["IMG_20261013_090502_007.jpg", "IMG_20261013_104512_009.jpg", "IMG_20261013_104512_345.jpg"]);
    }

    #[test]
    fn the_roll_sits_beside_the_photo_libraries() {
        let home = Path::new("/h/.makepad");
        assert_eq!(roll_dir_in(home), Path::new("/h/.makepad/photos/camera"));
    }

    #[test]
    fn a_shot_is_written_whole_and_never_overwrites_one() {
        let dir = std::env::temp_dir().join(format!("camera-roll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(latest_shot(&dir), None);
        let a = save_shot(&dir, &at(10, 0, 0), 1, b"first").unwrap();
        let b = save_shot(&dir, &at(10, 0, 0), 1, b"second").unwrap();
        let c = save_shot(&dir, &at(10, 0, 1), 0, b"third").unwrap();
        assert_eq!(c.file_name().unwrap(), "IMG_20261013_100001_000.jpg");
        assert_eq!(a.file_name().unwrap(), "IMG_20261013_100000_001.jpg");
        assert_eq!(b.file_name().unwrap(), "IMG_20261013_100000_001_2.jpg");
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        assert_eq!(std::fs::read(&b).unwrap(), b"second");
        // A shot still being written is not a shot.
        std::fs::write(dir.join("IMG_20261013_235959_999.jpg.part"), b"half").unwrap();
        assert_eq!(latest_shot(&dir), Some(c));
        // A second shot of one millisecond sorts after the first.
        std::fs::remove_file(dir.join("IMG_20261013_100001_000.jpg")).unwrap();
        assert_eq!(latest_shot(&dir), Some(b.clone()));
        assert!(!is_shot("IMG_1.jpg.part"));
        assert!(is_shot("IMG_1.JPG"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
