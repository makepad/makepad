//! The sidebar's bookmarks: folders the user keeps, in their own section.
//!
//! Bookmarks are the one piece of mpfiles state that outlives the process, so
//! the format is the one a person can fix in an editor when it goes wrong: one
//! absolute path per line, in the order the sidebar shows them. That is also
//! what GNOME Files stores (`~/.config/gtk-3.0/bookmarks`), minus the URI
//! scheme nobody here needs.
//!
//! Nothing in this module touches the UI, so all of it is unit-testable.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// How many bookmarks the sidebar has room for. The section is a fixed set of
/// slots in the DSL, so the model has to agree with it: a bookmark past the
/// last slot would be saved and never shown, which is worse than refusing it.
pub const MAX_BOOKMARKS: usize = 12;

/// The bookmarks file for a given home directory.
pub fn config_path(home: &Path) -> PathBuf {
    home.join(".config").join("mpfiles").join("bookmarks")
}

/// The bookmark list, in sidebar order, and where it is persisted.
#[derive(Clone, Debug, Default)]
pub struct Bookmarks {
    file: PathBuf,
    list: Vec<PathBuf>,
}

impl Bookmarks {
    /// Read the user's bookmarks. A missing file is an empty list, not an
    /// error: the first run of a fresh install must not look broken.
    pub fn load(home: &Path) -> Self {
        let file = config_path(home);
        let list = fs::read_to_string(&file)
            .map(|text| parse(&text))
            .unwrap_or_default();
        Self { file, list }
    }

    /// A list held in memory only — for tests, and for a home we cannot write.
    pub fn in_memory(list: Vec<PathBuf>) -> Self {
        Self {
            file: PathBuf::new(),
            list,
        }
    }

    pub fn list(&self) -> &[PathBuf] {
        &self.list
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.list.iter().any(|p| p == path)
    }

    /// Bookmark `path`. False when it is already there or the sidebar is full
    /// — either way nothing was added and the caller should say so.
    pub fn add(&mut self, path: &Path) -> bool {
        if self.contains(path) || self.list.len() >= MAX_BOOKMARKS {
            return false;
        }
        self.list.push(path.to_path_buf());
        self.persist();
        true
    }

    /// Drop `path` from the sidebar. False when it was never there.
    pub fn remove(&mut self, path: &Path) -> bool {
        let Some(at) = self.list.iter().position(|p| p == path) else {
            return false;
        };
        self.list.remove(at);
        self.persist();
        true
    }

    /// Write the list back. A failure is silent by design: a read-only home
    /// must not stop the user from using a bookmark for this session.
    fn persist(&self) {
        if self.file.as_os_str().is_empty() {
            return;
        }
        if let Some(dir) = self.file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&self.file, render(&self.list));
    }
}

/// One path per line; blank lines and `#` comments are skipped so a
/// hand-edited file with a note in it still loads.
fn parse(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = PathBuf::from(line);
        if !out.contains(&path) {
            out.push(path);
        }
        if out.len() >= MAX_BOOKMARKS {
            break;
        }
    }
    out
}

/// The file's text, newline-terminated so appending by hand works.
fn render(list: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in list {
        out.push_str(&path.to_string_lossy());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_path_per_line_skipping_notes() {
        let list = parse("# my folders\n/a/b\n\n/c/d\n/a/b\n");
        assert_eq!(list, [PathBuf::from("/a/b"), PathBuf::from("/c/d")]);
    }

    #[test]
    fn renders_what_it_parses() {
        let list = vec![PathBuf::from("/a/b"), PathBuf::from("/c d/e")];
        assert_eq!(parse(&render(&list)), list);
    }

    #[test]
    fn adds_removes_and_refuses_duplicates() {
        let mut marks = Bookmarks::in_memory(Vec::new());
        assert!(marks.add(Path::new("/a")));
        assert!(!marks.add(Path::new("/a")));
        assert!(marks.contains(Path::new("/a")));
        assert!(marks.remove(Path::new("/a")));
        assert!(!marks.remove(Path::new("/a")));
        assert!(marks.list().is_empty());
    }

    #[test]
    fn stops_at_the_last_sidebar_slot() {
        let mut marks = Bookmarks::in_memory(Vec::new());
        for i in 0..MAX_BOOKMARKS {
            assert!(marks.add(Path::new(&format!("/p{i}"))), "{i}");
        }
        assert!(!marks.add(Path::new("/one-too-many")));
        assert_eq!(marks.list().len(), MAX_BOOKMARKS);
    }

    #[test]
    fn survives_a_round_trip_through_a_real_file() {
        let home = std::env::temp_dir().join("mpfiles-test-bookmarks");
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();

        let mut marks = Bookmarks::load(&home);
        assert!(marks.list().is_empty(), "a fresh home has no bookmarks");
        assert!(marks.add(Path::new("/tmp/one")));
        assert!(marks.add(Path::new("/tmp/two")));

        // A second process sees exactly what the first one saved.
        let reread = Bookmarks::load(&home);
        assert_eq!(
            reread.list(),
            [PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")]
        );
        assert!(config_path(&home).is_file(), "the list is on disk where it says");

        marks.remove(Path::new("/tmp/one"));
        assert_eq!(Bookmarks::load(&home).list(), [PathBuf::from("/tmp/two")]);

        fs::remove_dir_all(&home).ok();
    }
}
