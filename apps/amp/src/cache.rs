//! The portable offline library: complete songs with their metadata and
//! artwork in one folder (`local/amp/offline/` in the checkout, see
//! `worker::offline_dir`), described by `songdb.json`. Amp shows and plays
//! these songs without a connection to the music system; copying the folder
//! to another checkout carries them along. Paths in the database are
//! relative to the folder.

use crate::worker::Merge;
use makepad_micro_serde::*;
use makepad_widgets::log;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

/// The library entry that lists the offline songs.
pub const LIBRARY_URI: &str = "amp:offline";
/// Artwork of an offline song: this prefix, then its path in the folder.
pub const ART_URI: &str = "amp-offline-art:";
/// The database file in the offline folder.
const SONG_DB: &str = "songdb.json";

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct Song {
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_uri: String,
    pub artist_uri: String,
    pub release: u32,
    pub duration_ms: u32,
    pub artwork_url: String,
    /// Paths in songdb.json are relative to the portable offline directory.
    pub audio: String,
    pub artwork: String,
    pub rate: u32,
    pub frames: u64,
    pub bytes: u64,
    pub checksum: String,
}

#[derive(Default, SerJson, DeJson)]
struct SongDb {
    version: u32,
    songs: Vec<Song>,
}

#[derive(Clone, Default)]
pub struct Library {
    pub root: PathBuf,
    pub songs: Vec<Song>,
}

/// `name` inside `root`, when it is a plain relative path (no `..`, no
/// absolute path): a database never points outside its folder.
pub fn relative(root: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    (!name.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_))))
        .then(|| root.join(path))
}

/// The songs `root/songdb.json` lists; none when there is no database.
fn load(root: &Path) -> Result<Vec<Song>, String> {
    let text = match fs::read_to_string(root.join(SONG_DB)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read the offline song database: {e}")),
    };
    let db = SongDb::deserialize_json(&text).map_err(|e| format!("invalid offline song database: {e:?}"))?;
    if db.version != 1 {
        return Err(format!("offline song database version {} needs a newer Amp", db.version));
    }
    Ok(db.songs)
}

/// Whether a song's file is there whole: its size as recorded. A file that
/// still fails to play falls back to the music system (`Model::local_failed`).
fn present(root: &Path, song: &Song) -> bool {
    relative(root, &song.audio)
        .and_then(|path| fs::metadata(path).ok())
        .is_some_and(|meta| meta.is_file() && meta.len() == song.bytes)
}

/// Read the offline library once and hand it to the merge worker.
pub fn run_library(root: Option<PathBuf>, merge: SyncSender<Merge>) {
    let Some(root) = root else { return };
    let mut songs = match load(&root) {
        Ok(songs) => songs,
        Err(error) => {
            log!("amp: {error} ({})", root.display());
            return;
        }
    };
    let listed = songs.len();
    songs.retain(|song| present(&root, song));
    if listed > 0 {
        log!("amp: offline library {}: {} songs ({} missing)", root.display(), songs.len(), listed - songs.len());
    }
    let _ = merge.send(Merge::Cached(Arc::new(Library { root, songs })));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_lists_whole_files_inside_its_folder() {
        let dir = std::env::temp_dir().join(format!("amp-offline-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("songs")).unwrap();
        fs::write(dir.join("songs/a.ogg"), b"abcd").unwrap();
        fs::write(dir.join("songs/b.ogg"), b"ab").unwrap();
        let song = |uri: &str, audio: &str| Song { uri: uri.into(), audio: audio.into(), bytes: 4, ..Default::default() };
        let db = SongDb { version: 1, songs: vec![song("t:a", "songs/a.ogg"), song("t:b", "songs/b.ogg"), song("t:c", "../a.ogg")] };
        fs::write(dir.join(SONG_DB), db.serialize_json()).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        run_library(Some(dir.clone()), tx);
        let Ok(Merge::Cached(library)) = rx.recv() else { panic!("no library") };
        let uris: Vec<_> = library.songs.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris, ["t:a"], "a short file and a path outside the folder are left out");
        let _ = fs::remove_dir_all(&dir);
    }
}
