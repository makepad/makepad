//! Where songs come from. Every source reads its songs through the importer
//! a person's own files go through in Stage (`midi_library::import` for
//! MIDI, `midi_library::chip::import` for tracker and SID music), so the
//! corpus is the music as Stage reads it: an [`Arrangement`].
//!
//! - [`Lakh`]: the Lakh MIDI Dataset's clean subset as `midi_library`
//!   installed it (the archive and its `index.tsv`, with the metadata).
//! - [`Folder`]: any folder of the person's own `.mid` files and the
//!   tracker and chip files `midi_library::chip` reads (`.mod`, `.xm`,
//!   `.s3m`, `.it`, `.sid`).
//!
//! A song carries its source and licence id from here to every record made
//! from it.

use makepad_midi_library::chip::import as chip;
use makepad_midi_library::arrangement::Arrangement;
use makepad_midi_library::import::{self, About, Options};
use makepad_midi_library::index::{self, Entry};
use makepad_midi_library::{analyse, dataset, md5, tar};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A dataset as the manifest lists it.
#[derive(Clone, Debug)]
pub struct Dataset {
    /// A short stable id (`lmd-clean`, `user`).
    pub id: String,
    pub name: String,
    /// The licence id every record carries.
    pub licence: String,
    pub credit: String,
    pub notice: String,
    /// Where it was read from.
    pub location: String,
}

/// One song before it is imported.
#[derive(Clone, Debug, Default)]
pub struct SongRef {
    /// The source's index in the build.
    pub source: usize,
    /// The source's own handle for it.
    pub key: usize,
    /// `<source>:<id>`, stable across builds.
    pub id: String,
    pub artist: String,
    pub title: String,
    pub year: Option<u16>,
    pub genres: Vec<String>,
    pub tags: Vec<(String, f32)>,
    /// The file's hash (MD5 hex): the exact duplicate check.
    pub hash: String,
    pub path: String,
    /// 0..100 from `midi_library::analyse` (None: not analysed).
    pub quality: Option<u8>,
    /// "4/4".
    pub meter: String,
    pub licence: String,
}

impl SongRef {
    pub fn decade(&self) -> Option<u16> {
        self.year.map(|y| y / 10 * 10)
    }
}

/// A song library the corpus reads.
pub trait Source: Sync {
    fn dataset(&self) -> Dataset;
    /// Every song, with what is known before importing it.
    fn songs(&self, source: usize) -> Result<Vec<SongRef>, String>;
    /// The song as Stage imports it.
    fn arrangement(&self, song: &SongRef) -> Result<Arrangement, String>;
}

fn about_of(song: &SongRef) -> About {
    About { title: song.title.clone(), artist: song.artist.clone(), year: song.year, id: song.id.clone(), path: song.path.clone(), genre: song.genres.first().cloned().unwrap_or_default(), tags: song.tags.clone() }
}

// ------------------------------------------------------------------ Lakh

/// The Lakh MIDI clean subset as `midi_library install` and `index` left
/// it: `clean_midi.tar.gz` and `index.tsv` in one folder.
pub struct Lakh {
    pub dir: PathBuf,
    entries: Vec<Entry>,
    tar: Vec<u8>,
    files: HashMap<String, (usize, usize)>,
}

pub const LAKH_LICENCE: &str = "CC-BY-4.0 (transcription); the song: its owners' copyright";

impl Lakh {
    /// `dir` holds the archive and the index (default: where
    /// `midi_library` installs them).
    pub fn open(dir: Option<&Path>) -> Result<Lakh, String> {
        let dir = dir.map(Path::to_path_buf).unwrap_or_else(dataset::dir);
        let entries = index::load(&dir.join("index.tsv"))?;
        let gz = std::fs::read(dir.join("clean_midi.tar.gz")).map_err(|e| format!("{}: {e} (midi_library install)", dir.join("clean_midi.tar.gz").display()))?;
        let tar = tar::gunzip(&gz)?;
        drop(gz);
        let files = tar::entries(&tar).into_iter().map(|e| (e.path, (e.offset, e.len))).collect();
        Ok(Lakh { dir, entries, tar, files })
    }
}

impl Source for Lakh {
    fn dataset(&self) -> Dataset {
        Dataset { id: "lmd-clean".into(), name: "Lakh MIDI Dataset v0.1, clean subset".into(), licence: LAKH_LICENCE.into(), credit: dataset::CREDIT.into(), notice: dataset::NOTICE.into(), location: self.dir.display().to_string() }
    }

    fn songs(&self, source: usize) -> Result<Vec<SongRef>, String> {
        Ok(self
            .entries
            .iter()
            .enumerate()
            .map(|(key, e)| SongRef {
                source,
                key,
                id: format!("lmd:{}", e.id),
                artist: e.artist.clone(),
                title: e.title.clone(),
                year: e.year,
                genres: e.genres.iter().map(|g| g.trim_start_matches('~').to_string()).collect(),
                tags: e.tags.clone(),
                hash: e.md5.clone(),
                path: e.path.clone(),
                quality: Some(e.features.quality),
                meter: e.features.sig.clone(),
                licence: LAKH_LICENCE.into(),
            })
            .collect())
    }

    fn arrangement(&self, song: &SongRef) -> Result<Arrangement, String> {
        let &(at, len) = self.files.get(&song.path).ok_or("not in the archive")?;
        Ok(import::import(&self.tar[at..at + len], &about_of(song), &Options::default())?.arrangement)
    }
}

// ------------------------------------------------------------------ folder

pub const USER_LICENCE: &str = "user-own (the person's own files; not redistributed)";

const MIDI_EXT: [&str; 3] = ["mid", "midi", "kar"];

/// A folder of the person's own files (searched recursively).
pub struct Folder {
    pub dir: PathBuf,
    files: Vec<PathBuf>,
}

impl Folder {
    pub fn open(dir: &Path) -> Result<Folder, String> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let read = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
            for entry in read.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if ext_of(&p).is_some_and(|e| MIDI_EXT.contains(&e.as_str())) || chip::is_supported(&p.to_string_lossy()) {
                    files.push(p);
                }
            }
        }
        files.sort();
        Ok(Folder { dir: dir.to_path_buf(), files })
    }
}

fn ext_of(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

impl Source for Folder {
    fn dataset(&self) -> Dataset {
        Dataset { id: "user".into(), name: format!("your files in {}", self.dir.display()), licence: USER_LICENCE.into(), credit: "your own files".into(), notice: "Your own files: used to build your training data on your machine, never sent anywhere.".into(), location: self.dir.display().to_string() }
    }

    fn songs(&self, source: usize) -> Result<Vec<SongRef>, String> {
        let mut out = Vec::new();
        for (key, p) in self.files.iter().enumerate() {
            let Ok(bytes) = std::fs::read(p) else { continue };
            let hash = md5::hex(&bytes);
            let rel = p.strip_prefix(&self.dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            // A second version of a song is `Title.1.mid`.
            let stem = match stem.rsplit_once('.') {
                Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head.to_string(),
                _ => stem,
            };
            // `Artist - Title.mid`, or the folder as the artist.
            let (artist, title) = match stem.split_once(" - ") {
                Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
                None => (p.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(), stem.clone()),
            };
            let mut song = SongRef { source, key, id: format!("user:{}", &hash[..8]), artist, title, hash, path: rel, licence: USER_LICENCE.into(), ..SongRef::default() };
            if ext_of(p).is_some_and(|e| MIDI_EXT.contains(&e.as_str())) {
                if let Ok(s) = analyse::read(&bytes) {
                    let f = analyse::features(&s);
                    song.quality = Some(f.quality);
                    song.meter = f.sig;
                }
            }
            out.push(song);
        }
        Ok(out)
    }

    fn arrangement(&self, song: &SongRef) -> Result<Arrangement, String> {
        let p = &self.files[song.key];
        let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
        if ext_of(p).is_some_and(|e| MIDI_EXT.contains(&e.as_str())) {
            Ok(import::import(&bytes, &about_of(song), &Options::default())?.arrangement)
        } else {
            let about = chip::About { title: song.title.clone(), artist: song.artist.clone(), year: song.year, ..chip::About::default() };
            Ok(chip::import(&bytes, &p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &about)?.arrangement)
        }
    }
}
