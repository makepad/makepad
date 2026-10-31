//! The open score as a document, and the scores on disk. A score is a
//! `.score` file (its Splash) in the library folder (`scores/`); the open
//! one keeps a draft (`drafts/<id>.score` beside a `<id>.meta`) once it has
//! changes, so a restart finds it as it was; `drafts/open` names the
//! document open at the last stop.

use crate::paths;
use std::path::{Path, PathBuf};

/// The open score as a document.
#[derive(Clone, Debug, Default)]
pub struct Document {
    /// Its draft's id.
    pub id: String,
    pub title: String,
    /// The library file it was opened from or last saved as.
    pub path: Option<PathBuf>,
    /// The source as last opened or saved (Edited is a difference from it).
    pub saved: Option<String>,
}

/// A new draft id: the time, in ms, and a counter.
fn new_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("d{ms}-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// A library file's draft id: its file name.
fn id_of(path: &Path) -> String {
    format!("f-{}", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

impl Document {
    pub fn new(title: &str) -> Document {
        Document { id: new_id(), title: title.to_string(), path: None, saved: None }
    }

    /// A document whose text was given (a batch's song): not edited until
    /// it changes, and never in the library until saved.
    pub fn unsaved(id: String, title: &str, source: &str) -> Document {
        Document { id, title: title.to_string(), path: None, saved: Some(source.to_string()) }
    }

    pub fn edited(&self, source: &str) -> bool {
        self.saved.as_deref() != Some(source)
    }

    /// Nothing to keep: unchanged, or a new blank score.
    fn pristine(&self, source: &str) -> bool {
        !self.edited(source) || (self.path.is_none() && Some(source) == makepad_score_live::templates::template("blank"))
    }

    /// Keep the document: its draft once it has changes (a draft of an
    /// unchanged score is removed), and which document is open.
    pub fn keep(&self, source: &str) {
        let dir = paths::drafts_dir();
        let (text, meta) = (dir.join(format!("{}.score", self.id)), dir.join(format!("{}.meta", self.id)));
        if self.pristine(source) {
            let _ = std::fs::remove_file(&text);
            let _ = std::fs::remove_file(&meta);
        } else {
            let path = self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::write(&text, source);
            let _ = std::fs::write(&meta, format!("title={}\npath={path}\n", self.title.replace('\n', " ")));
        }
        let open = match &self.path {
            Some(path) if self.pristine(source) => format!("file {}", path.to_string_lossy()),
            _ => format!("draft {}", self.id),
        };
        let _ = std::fs::write(dir.join("open"), open);
    }

    /// What was open at the last stop, with its text.
    pub fn last() -> Option<(Document, String)> {
        let dir = paths::drafts_dir();
        let open = std::fs::read_to_string(dir.join("open")).ok()?;
        let (kind, rest) = open.trim().split_once(' ')?;
        match kind {
            "file" => Document::open_file(Path::new(rest)).ok(),
            "draft" => Document::draft(rest),
            _ => None,
        }
    }

    /// A draft by id: its document and text.
    pub fn draft(id: &str) -> Option<(Document, String)> {
        let dir = paths::drafts_dir();
        let source = std::fs::read_to_string(dir.join(format!("{id}.score"))).ok()?;
        let meta = std::fs::read_to_string(dir.join(format!("{id}.meta"))).unwrap_or_default();
        let field = |key: &str| meta.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).map(str::to_string).filter(|v| !v.is_empty());
        let path = field("path").map(PathBuf::from);
        // A draft of a library file: Edited against the file as it is.
        let saved = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        Some((Document { id: id.to_string(), title: field("title").unwrap_or_else(|| "Untitled".into()), path, saved }, source))
    }

    /// A library file opened: its draft if it has unsaved changes, else
    /// its text.
    pub fn open_file(path: &Path) -> Result<(Document, String), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let id = id_of(path);
        let title = title_of(&text, path);
        let doc = Document { id: id.clone(), title, path: Some(path.to_path_buf()), saved: Some(text.clone()) };
        let draft = std::fs::read_to_string(paths::drafts_dir().join(format!("{id}.score"))).ok();
        Ok((doc, draft.unwrap_or(text)))
    }
}

impl Document {
    /// Save `source`: into its library file, or (`name`, or never saved) a
    /// new file named for `name` or its title. From then on it is that file,
    /// and its draft goes.
    pub fn save(&mut self, source: &str, name: Option<&str>) -> Result<PathBuf, String> {
        let path = match (&self.path, name) {
            (Some(path), None) => path.clone(),
            _ => free_path(name.unwrap_or(&self.title)),
        };
        self.save_to(source, path)
    }

    /// Save `source` as the file `path` (Save as): from then on it is that file.
    pub fn save_to(&mut self, source: &str, path: PathBuf) -> Result<PathBuf, String> {
        std::fs::write(&path, source).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = paths::drafts_dir();
        let _ = std::fs::remove_file(dir.join(format!("{}.score", self.id)));
        let _ = std::fs::remove_file(dir.join(format!("{}.meta", self.id)));
        self.id = id_of(&path);
        self.path = Some(path.clone());
        self.saved = Some(source.to_string());
        self.keep(source);
        Ok(path)
    }
}

/// A file name for a title: lower case, words joined by `_`.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() { "untitled".into() } else { out }
}

/// A library file for a new score titled `title` that does not exist yet.
fn free_path(title: &str) -> PathBuf {
    let dir = paths::scores_dir();
    let base = slug(title);
    let mut path = dir.join(format!("{base}.score"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}_{n}.score"));
        n += 1;
    }
    path
}

/// The drafts kept: (id, title, the library file it is a draft of), newest
/// first.
pub fn drafts() -> Vec<(String, String, Option<PathBuf>)> {
    let Ok(dir) = std::fs::read_dir(paths::drafts_dir()) else { return Vec::new() };
    let mut metas: Vec<(std::time::SystemTime, PathBuf)> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "meta"))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)).collect();
    metas.sort_by(|a, b| b.0.cmp(&a.0));
    metas.into_iter().filter_map(|(_, p)| {
        let id = p.file_stem()?.to_string_lossy().into_owned();
        let meta = std::fs::read_to_string(&p).ok()?;
        let field = |key: &str| meta.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).map(str::to_string).filter(|v| !v.is_empty());
        Some((id, field("title").unwrap_or_else(|| "Untitled".into()), field("path").map(PathBuf::from)))
    }).collect()
}

/// The audio a score can play by name (`asset("<file>", "file")`) and the
/// SoundFonts it can load: the files in the samples folder, by name.
pub fn samples() -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(paths::samples_dir()) else { return Vec::new() };
    let mut out: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| {
        p.extension().map(|e| e.to_string_lossy().to_lowercase()).is_some_and(|e| matches!(e.as_str(), "wav" | "mp3" | "ogg" | "flac" | "sf2"))
    }).collect();
    out.sort();
    out
}

/// The revision a local file's reference names (`asset("kick.wav", "file")`).
pub const FILE_REV: &str = "file";

/// A library score by its file name (with or without `.score`) or title.
pub fn find(name: &str) -> Option<PathBuf> {
    let want = name.trim().trim_end_matches(".score");
    saved_scores().into_iter().find(|(title, path)| path.file_stem().is_some_and(|s| s == want) || title.eq_ignore_ascii_case(want)).map(|(_, p)| p)
}

/// A score's title: its `label`, else its file's name.
pub fn title_of(source: &str, path: &Path) -> String {
    makepad_score_live::ScoreDoc::load(source).ok().map(|d| d.label).filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_else(|| "Untitled".into()))
}

/// The library's scores: (title, file), newest first.
pub fn saved_scores() -> Vec<(String, PathBuf)> {
    let Ok(dir) = std::fs::read_dir(paths::scores_dir()) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "score"))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)).collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().map(|(_, p)| {
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        (title_of(&text, &p), p)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_label_else_the_file_name() {
        let blank = makepad_score_live::templates::template("techno").unwrap();
        let label = makepad_score_live::ScoreDoc::load(blank).unwrap().label;
        assert_eq!(title_of(blank, Path::new("x.score")), if label.is_empty() { "x".to_string() } else { label });
        assert_eq!(title_of("not a score", Path::new("/a/night_drive.score")), "night drive");
    }

    #[test]
    fn file_names_are_slugs_of_titles() {
        assert_eq!(slug("Night Drive (v2)!"), "night_drive_v2");
        assert_eq!(slug("  "), "untitled");
    }
}
