//! Local files: music the person owns, in folders they added. The **local**
//! worker walks those folders (MP3, Ogg Vorbis and FLAC), reads each file's
//! tags and length with `makepad-audio-decode`, and keeps what it read in a
//! cache on disk, so a file is read again only when its size or modification
//! time changed; files that went away drop out.
//!
//! What the cache holds becomes an [`Index`] of its own, a "Local Files"
//! folder with one playlist per added folder (its files in path order),
//! which the merge worker lays beside the music system's library
//! ([`Index::merged`]): search, the tables and the sidebar see both, and a
//! file that is a copy of a system track is linked to it.
//!
//! Folders are scanned when added, when the window comes back into use (at
//! most every [`RESCAN_MIN`]) and every [`RESCAN_EVERY`] while the app runs.
//! A scan reads each file's ends only — its tags and header at the start,
//! its last bytes for an Ogg length ([`makepad_audio_decode::probe_ends`])
//! — and goes [`CHUNK`] files at a time: between chunks it takes folder
//! changes, and every [`PUBLISH_EVERY`] files read it saves the cache and
//! hands the library over, so a big first scan shows up as it goes. A
//! folder that cannot be read at all (an unmounted drive, a sleeping NAS)
//! keeps the files it had.

use crate::index::{line, unescape, Index, LOCAL_FILE};
use crate::source::{release_key, LibraryEntry, Playlist, PlaylistTrack, SourceId, TrackDetail};
use crate::worker::{send, Merge, Message};
use makepad_widgets::log;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// First line of the cache file.
const MAGIC: &str = "makepad-music-local 1";
/// The playlist tree's heading over the added folders.
pub const LOCAL_ROOT: &str = "local:";
/// How an added folder's playlist URI starts; the rest is its path.
pub const LOCAL_FOLDER: &str = "local:folder:";
const RESCAN_EVERY: Duration = Duration::from_secs(5 * 60);
/// A rescan asked for sooner than this after the last one waits for it.
const RESCAN_MIN: Duration = Duration::from_secs(30);
/// Bounds on what a scan looks at: folders deep, and files in all.
const MAX_DEPTH: usize = 32;
const MAX_FILES: usize = 200_000;
/// The first bytes read of a file, and the most its head may grow to (an
/// ID3 tag or FLAC picture block holding cover art can be megabytes).
const HEAD_START: usize = 64 * 1024;
const HEAD_MAX: usize = 32 * 1024 * 1024;
/// The last bytes read of a file: an Ogg page is at most 64 KiB.
const TAIL: u64 = 128 * 1024;
/// Files a scan reads before it looks for folder changes again.
const CHUNK: usize = 25;
/// Files read between two hand-overs of the library.
const PUBLISH_EVERY: usize = 200;

/// What the local worker is asked.
pub enum LocalCommand {
    AddFolder(String),
    RemoveFolder(String),
    /// Look for changes now (unless a scan ran very recently).
    Rescan,
}

/// One music file as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalFile {
    pub size: u64,
    /// Modification time, milliseconds since the epoch.
    pub modified: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
    /// `yyyymmdd`, parts not known 0 (see [`release_key`]).
    pub release: u32,
    /// The tempo the file's tags claim (`TBPM`, `BPM`), 0 when none.
    pub bpm: f32,
}

/// The added folders and every music file found in them, by path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalCache {
    pub folders: Vec<String>,
    pub files: BTreeMap<String, LocalFile>,
    /// Files named like music that would not read, with the size and time
    /// they had: not tried again until they change. Not saved; a new run
    /// tries them once.
    unreadable: BTreeMap<String, (u64, u64)>,
}

/// A scan under way: the files found that are new or changed, still to
/// be read, and what was done so far.
#[derive(Debug, Default)]
pub struct Scan {
    todo: Vec<(String, u64, u64)>,
    pub found: usize,
    pub stats: ScanStats,
}

impl Scan {
    pub fn done(&self) -> bool {
        self.todo.is_empty()
    }
}

/// What a scan did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Files read (new or changed).
    pub read: usize,
    /// Files whose cached reading still held.
    pub kept: usize,
    /// Files no longer there.
    pub removed: usize,
}

impl LocalCache {
    /// A whole scan at once: walk, then read everything new or changed.
    #[cfg(test)]
    pub fn scan(&mut self) -> ScanStats {
        let mut scan = self.begin_scan();
        while !scan.done() {
            self.continue_scan(&mut scan, CHUNK);
        }
        scan.stats
    }

    /// Walk every folder: drop what went away, keep what did not change,
    /// and return the rest to be read. The files of a folder that cannot
    /// be read at all are kept as they were: gone for now, not gone.
    pub fn begin_scan(&mut self) -> Scan {
        let mut found: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut away: Vec<String> = Vec::new();
        for folder in &self.folders {
            if std::fs::read_dir(folder).is_err() {
                away.push(folder.clone());
                continue;
            }
            walk(Path::new(folder), 0, &mut found);
        }
        let kept = |path: &str| found.contains_key(path) || away.iter().any(|folder| within(path, folder));
        let mut scan = Scan { found: found.len(), ..Scan::default() };
        let before = self.files.len();
        self.files.retain(|path, _| kept(path));
        self.unreadable.retain(|path, _| kept(path));
        scan.stats.removed = before - self.files.len();
        for (path, (size, modified)) in found {
            let unchanged = self.files.get(&path).is_some_and(|file| file.size == size && file.modified == modified);
            if unchanged || self.unreadable.get(&path) == Some(&(size, modified)) {
                scan.stats.kept += 1;
            } else {
                scan.todo.push((path, size, modified));
            }
        }
        // Read from the front: `todo` is taken from its end.
        scan.todo.reverse();
        scan
    }

    /// Read up to `count` of the files `scan` still has to read.
    pub fn continue_scan(&mut self, scan: &mut Scan, count: usize) {
        for _ in 0..count {
            let Some((path, size, modified)) = scan.todo.pop() else { return };
            match read_file(Path::new(&path), size, modified) {
                Some(file) => {
                    self.unreadable.remove(&path);
                    self.files.insert(path, file);
                }
                // Not music after all, or unreadable: left out until it changes.
                None => {
                    self.files.remove(&path);
                    self.unreadable.insert(path, (size, modified));
                }
            }
            scan.stats.read += 1;
        }
    }

    /// A folder is no longer looked in: the scan under way drops its files.
    fn forget_in_scan(scan: &mut Scan, folder: &str) {
        scan.todo.retain(|(path, _, _)| !within(path, folder));
    }

    /// Stop looking in `folder`; files no other folder holds go.
    pub fn remove_folder(&mut self, folder: &str) {
        self.folders.retain(|f| f != folder);
        let folders = self.folders.clone();
        self.files.retain(|path, _| folders.iter().any(|folder| within(path, folder)));
    }

    /// The library the cache describes: a "Local Files" folder with one
    /// playlist per added folder, its files in path order.
    pub fn index(&self) -> Index {
        let mut index = Index::new(SourceId::Local);
        // The Local Files heading is always there: it is the way to the
        // folders page, where folders are added.
        let mut entries = vec![LibraryEntry { uri: LOCAL_ROOT.into(), name: SourceId::Local.name().into(), is_folder: true, depth: 1 }];
        if self.folders.is_empty() {
            index.set_library(entries);
            return index;
        }
        let names = folder_names(&self.folders);
        entries.extend(self.folders.iter().zip(&names).map(|(folder, name)| LibraryEntry { uri: format!("{LOCAL_FOLDER}{folder}"), name: name.clone(), is_folder: false, depth: 2 }));
        index.set_library(entries);
        let mut details = Vec::new();
        let mut asked = Vec::new();
        let mut tempos = Vec::new();
        for (folder, name) in self.folders.iter().zip(names) {
            let tracks: Vec<PlaylistTrack> = self
                .files
                .iter()
                .filter(|(path, _)| within(path, folder))
                .map(|(path, file)| PlaylistTrack {
                    uri: format!("{LOCAL_FILE}{path}"),
                    name: file.title.clone(),
                    artists: if file.artist.is_empty() { Vec::new() } else { vec![file.artist.clone()] },
                    album: file.album.clone(),
                })
                .collect();
            let count = tracks.len() as u32;
            index.set_playlist(Playlist { uri: format!("{LOCAL_FOLDER}{folder}"), name, owner: String::new(), description: String::new(), track_count: count, tracks }, crate::worker::now_secs());
        }
        for (path, file) in &self.files {
            let uri = format!("{LOCAL_FILE}{path}");
            details.push(TrackDetail { uri: uri.clone(), duration_ms: file.duration_ms, release: file.release, ..TrackDetail::default() });
            if file.bpm > 0.0 {
                tempos.push((uri.clone(), file.bpm));
            }
            asked.push(uri);
        }
        index.set_details(&asked, details);
        index.set_tempos(tempos);
        index
    }

    // ---- on disk ----

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut out = String::with_capacity(64 + self.files.len() * 120);
        out.push_str(MAGIC);
        out.push('\n');
        for folder in &self.folders {
            line(&mut out, 'D', &[folder]);
        }
        for (path, file) in &self.files {
            line(
                &mut out,
                'F',
                &[path, &file.size.to_string(), &file.modified.to_string(), &file.title, &file.artist, &file.album, &file.duration_ms.to_string(), &file.release.to_string(), &file.bpm.to_string()],
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, out)?;
        std::fs::rename(&temp, path)
    }

    pub fn load(path: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(path).ok()?)
    }

    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != MAGIC {
            return None;
        }
        let mut cache = Self::default();
        for raw in lines {
            let fields: Vec<String> = raw.split('\t').map(unescape).collect();
            let field = |at: usize| fields.get(at).map(String::as_str).unwrap_or("");
            match field(0) {
                "D" if !field(1).is_empty() && !cache.folders.iter().any(|f| f == field(1)) => cache.folders.push(field(1).into()),
                "F" if fields.len() >= 9 => {
                    cache.files.insert(
                        field(1).into(),
                        LocalFile {
                            size: field(2).parse().unwrap_or(0),
                            // A line from before the tempo field was read without
                            // it: an unknown time makes the next scan read its
                            // tags again.
                            modified: if fields.len() >= 10 { field(3).parse().unwrap_or(0) } else { 0 },
                            title: field(4).into(),
                            artist: field(5).into(),
                            album: field(6).into(),
                            duration_ms: field(7).parse().unwrap_or(0),
                            release: field(8).parse().unwrap_or(0),
                            bpm: field(9).parse().unwrap_or(0.0),
                        },
                    );
                }
                _ => {}
            }
        }
        Some(cache)
    }
}

/// Whether `path` lies in `folder` (or is it).
fn within(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches('/');
    path.strip_prefix(folder).is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
}

/// The sidebar name of each folder: its last part, or its whole path when
/// two folders end the same.
fn folder_names(folders: &[String]) -> Vec<String> {
    let last = |folder: &str| Path::new(folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| folder.to_string());
    folders
        .iter()
        .map(|folder| {
            let name = last(folder);
            if folders.iter().filter(|other| last(other) == name).count() > 1 { folder.clone() } else { name }
        })
        .collect()
}

/// Whether a file name is one of the kinds the decoders read.
fn is_music(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["mp3", "ogg", "oga", "flac"].iter().any(|ext| lower.rsplit_once('.').is_some_and(|(_, e)| e == *ext))
}

/// Every music file under `dir`: path → (size, modification ms). Hidden
/// files and folders are skipped, and folder links are not followed, so a
/// link back up the tree cannot loop.
fn walk(dir: &Path, depth: usize, found: &mut BTreeMap<String, (u64, u64)>) {
    if depth > MAX_DEPTH || found.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, depth + 1, found);
        } else if is_music(&name) {
            // A file link is read through; a folder link is not walked.
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
            found.insert(path.to_string_lossy().to_string(), (meta.len(), modified));
            if found.len() >= MAX_FILES {
                return;
            }
        }
    }
}

/// A file's first bytes, grown until its tags are in, and its last bytes.
fn read_ends(path: &Path, size: u64) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let mut head = Vec::new();
    (&mut file).take(HEAD_START as u64).read_to_end(&mut head)?;
    while let Some(need) = makepad_audio_decode::head_needed(&head, size) {
        let need = need.min(HEAD_MAX);
        if need <= head.len() {
            break;
        }
        let more = (need - head.len()) as u64;
        let before = head.len();
        (&mut file).take(more).read_to_end(&mut head)?;
        if head.len() == before {
            break;
        }
    }
    let from = size.saturating_sub(TAIL);
    let tail = if from < head.len() as u64 {
        head[from as usize..].to_vec()
    } else {
        file.seek(SeekFrom::Start(from))?;
        let mut tail = Vec::new();
        file.take(TAIL).read_to_end(&mut tail)?;
        tail
    };
    Ok((head, tail))
}

/// Read one file's tags and length from its ends. `None` for a file that
/// is not music the decoders know.
fn read_file(path: &Path, size: u64, modified: u64) -> Option<LocalFile> {
    let (head, tail) = read_ends(path, size).ok()?;
    let (tags, seconds) = makepad_audio_decode::probe_ends(&head, &tail, size).ok()?;
    let bpm = tags.bpm().unwrap_or(0.0) as f32;
    let duration_ms = seconds.map(|secs| (secs * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32).unwrap_or(0);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    Some(LocalFile {
        size,
        modified,
        title: tags.title.unwrap_or(stem),
        artist: tags.artist.unwrap_or_default(),
        album: tags.album.unwrap_or_default(),
        duration_ms,
        release: tags.date.as_deref().map(release_key).unwrap_or(0),
        bpm,
    })
}

/// The local worker: loads the cache, then scans, rescans and changes the
/// folders as asked, handing each new library to the merge worker. A scan
/// goes a chunk at a time, taking commands between chunks.
pub fn run_local(data: Option<PathBuf>, rx: Receiver<LocalCommand>, tx: SyncSender<Message>, merge: SyncSender<Merge>) {
    let file = data.map(|dir| dir.join("local-files.txt"));
    let mut cache = file.as_ref().and_then(|file| LocalCache::load(file)).unwrap_or_default();
    send(&tx, Message::Folders(cache.folders.clone()));
    let _ = merge.send(Merge::Local(Arc::new(cache.index())));
    let publish = |cache: &LocalCache| {
        if let Some(file) = &file {
            if let Err(error) = cache.save(file) {
                log!("amp: cannot save the local files at {}: {error}", file.display());
            }
        }
        let _ = merge.send(Merge::Local(Arc::new(cache.index())));
    };
    let mut next_scan = Some(Instant::now());
    let mut last_scan: Option<Instant> = None;
    let mut scan: Option<(Scan, Instant)> = None;
    // Changes not handed over yet, and files read since the last hand-over.
    let mut unpublished = false;
    let mut read_since = 0;
    loop {
        let received = if scan.is_some() {
            rx.try_recv().map_err(|error| match error {
                TryRecvError::Empty => RecvTimeoutError::Timeout,
                TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
            })
        } else {
            match next_scan {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            }
        };
        let mut folders_changed = false;
        match received {
            Ok(LocalCommand::AddFolder(folder)) => {
                if !cache.folders.contains(&folder) {
                    cache.folders.push(folder);
                    folders_changed = true;
                    // Walked again from the top, the new folder with it.
                    scan = None;
                    next_scan = Some(Instant::now());
                }
            }
            Ok(LocalCommand::RemoveFolder(folder)) => {
                if cache.folders.contains(&folder) {
                    cache.remove_folder(&folder);
                    if let Some((scan, _)) = &mut scan {
                        LocalCache::forget_in_scan(scan, &folder);
                    }
                    folders_changed = true;
                }
            }
            Ok(LocalCommand::Rescan) => {
                let soon = last_scan.map_or(Instant::now(), |at| at + RESCAN_MIN);
                next_scan = Some(next_scan.map_or(soon, |at| at.min(soon)));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if folders_changed {
            send(&tx, Message::Folders(cache.folders.clone()));
            unpublished = true;
        }
        if scan.is_none() && next_scan.is_some_and(|at| Instant::now() >= at) {
            let begun = cache.begin_scan();
            unpublished |= begun.stats.removed > 0;
            scan = Some((begun, Instant::now()));
            next_scan = None;
        }
        if let Some((current, started)) = &mut scan {
            let before = current.stats.read;
            cache.continue_scan(current, CHUNK);
            read_since += current.stats.read - before;
            unpublished |= current.stats.read > before;
            send(&tx, Message::LocalProgress(if current.done() { String::new() } else { format!("Reading local files {}/{}", current.stats.read, current.found) }));
            if current.done() {
                let stats = current.stats;
                log!("amp: local files scanned in {:.1} s: {} read, {} unchanged, {} gone", started.elapsed().as_secs_f64(), stats.read, stats.kept, stats.removed);
                scan = None;
                last_scan = Some(Instant::now());
                next_scan = (!cache.folders.is_empty()).then(|| Instant::now() + RESCAN_EVERY);
            }
        }
        // Hand over when a scan ends, every so many files during one, and
        // at once for a folder change.
        if unpublished && (scan.is_none() || read_since >= PUBLISH_EVERY || folders_changed) {
            publish(&cache);
            unpublished = false;
            read_since = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A fresh folder under the system's temporary directory.
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("music-local-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A short tagged Ogg Vorbis file of a tone, `seconds` long.
    fn song(path: &Path, title: &str, artist: &str, seconds: f32) {
        let rate = 22_050;
        let pcm: Vec<f32> = (0..(rate as f32 * seconds) as usize).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let options = makepad_audio_encode::EncodeOptions { quality: 0.2, threads: 1, tags: vec![("TITLE".into(), title.into()), ("ARTIST".into(), artist.into()), ("ALBUM".into(), "Tests".into()), ("DATE".into(), "2001".into())] };
        let bytes = makepad_audio_encode::encode_vorbis(rate, 1, &pcm, &options).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A scan reads what is new, keeps what is unchanged, reads again what
    /// changed and drops what went away; other files and hidden folders
    /// are not music.
    #[test]
    fn a_scan_reads_only_what_changed() {
        let dir = scratch();
        song(&dir.join("a/one.ogg"), "One", "Tester", 0.5);
        song(&dir.join("a/two.ogg"), "Two", "Tester", 0.5);
        song(&dir.join(".hidden/three.ogg"), "Three", "Tester", 0.5);
        std::fs::write(dir.join("notes.txt"), "not music").unwrap();
        std::fs::write(dir.join("fake.mp3"), "not an mp3 either").unwrap();
        let mut cache = LocalCache { folders: vec![dir.to_string_lossy().to_string()], ..LocalCache::default() };
        let stats = cache.scan();
        assert_eq!(stats, ScanStats { read: 3, kept: 0, removed: 0 }, "two songs and the fake mp3 are read");
        assert_eq!(cache.files.len(), 2, "the fake mp3 is not music");
        let one = &cache.files[&dir.join("a/one.ogg").to_string_lossy().to_string()];
        assert_eq!((one.title.as_str(), one.artist.as_str(), one.album.as_str(), one.release), ("One", "Tester", "Tests", 20_010_000));
        assert!((450..=550).contains(&one.duration_ms), "{} ms", one.duration_ms);

        let again = cache.scan();
        assert_eq!(again, ScanStats { read: 0, kept: 3, removed: 0 }, "nothing changed, the fake mp3 included");
        song(&dir.join("a/two.ogg"), "Two (New Mix)", "Tester", 1.0);
        std::fs::remove_file(dir.join("a/one.ogg")).unwrap();
        let changed = cache.scan();
        assert_eq!((changed.read, changed.removed), (1, 1));
        assert_eq!(cache.files.values().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["Two (New Mix)"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cache reads back as written; the index has the folder heading,
    /// a playlist per folder, and the files with their lengths.
    #[test]
    fn the_cache_reads_back_and_makes_the_library() {
        let mut cache = LocalCache { folders: vec!["/music/a".into(), "/other/a".into()], ..LocalCache::default() };
        cache.files.insert("/music/a/x.ogg".into(), LocalFile { size: 10, modified: 20, title: "X\ttab".into(), artist: "Y".into(), album: "Z".into(), duration_ms: 1500, release: 19_950_000, bpm: 126.5 });
        cache.files.insert("/other/a/w.flac".into(), LocalFile { title: "W".into(), ..LocalFile::default() });
        let dir = scratch();
        let file = dir.join("local-files.txt");
        cache.save(&file).unwrap();
        assert_eq!(LocalCache::load(&file), Some(cache.clone()));
        let index = cache.index();
        let names: Vec<&str> = index.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Local Files", "/music/a", "/other/a"], "folders ending alike are told apart");
        assert!(index.entries[0].is_folder);
        let x = index.track(&format!("{LOCAL_FILE}/music/a/x.ogg")).unwrap();
        assert_eq!((x.name.as_str(), x.duration_ms, x.local_path()), ("X\ttab", 1500, Some("/music/a/x.ogg")));
        assert_eq!(x.bpm, 126.5, "the tag tempo reaches the track");
        cache.remove_folder("/music/a");
        assert_eq!(cache.files.len(), 1);
        assert_eq!(cache.index().entries.len(), 2);
        assert!(LocalCache::parse("something else\n").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_holds_what_lies_in_it() {
        assert!(within("/m/a/b.ogg", "/m/a"));
        assert!(within("/m/a/b.ogg", "/m/a/"));
        assert!(!within("/m/ab/b.ogg", "/m/a"));
        assert!(is_music("x.MP3") && is_music("x.oga") && !is_music("x.wav") && !is_music("mp3"));
    }

    /// A folder that cannot be read at all (an unmounted drive) keeps the
    /// files it had; a folder that can be read loses what went.
    #[test]
    fn an_unreachable_folder_keeps_its_files() {
        let dir = scratch();
        song(&dir.join("here/one.ogg"), "One", "Tester", 0.3);
        let gone = dir.join("unmounted").to_string_lossy().to_string();
        let here = dir.join("here").to_string_lossy().to_string();
        let mut cache = LocalCache { folders: vec![gone.clone(), here.clone()], ..LocalCache::default() };
        cache.files.insert(format!("{gone}/far.flac"), LocalFile { title: "Far".into(), ..LocalFile::default() });
        cache.files.insert(format!("{here}/deleted.ogg"), LocalFile { title: "Deleted".into(), ..LocalFile::default() });
        let stats = cache.scan();
        assert_eq!((stats.read, stats.removed), (1, 1));
        let titles: Vec<&str> = cache.files.values().map(|f| f.title.as_str()).collect();
        assert!(titles.contains(&"Far") && titles.contains(&"One") && !titles.contains(&"Deleted"), "{titles:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scan goes a chunk at a time; a folder removed while it runs is not
    /// read any further.
    #[test]
    fn a_scan_goes_in_chunks_and_drops_a_removed_folder() {
        let dir = scratch();
        for name in ["a/1.ogg", "a/2.ogg", "b/3.ogg", "b/4.ogg"] {
            song(&dir.join(name), name, "Tester", 0.2);
        }
        let (a, b) = (dir.join("a").to_string_lossy().to_string(), dir.join("b").to_string_lossy().to_string());
        let mut cache = LocalCache { folders: vec![a.clone(), b.clone()], ..LocalCache::default() };
        let mut scan = cache.begin_scan();
        assert_eq!(scan.found, 4);
        cache.continue_scan(&mut scan, 1);
        assert_eq!((scan.stats.read, cache.files.len()), (1, 1));
        cache.remove_folder(&b);
        LocalCache::forget_in_scan(&mut scan, &b);
        cache.continue_scan(&mut scan, 10);
        assert!(scan.done());
        assert_eq!(cache.files.len(), 2, "only folder a's files");
        assert!(cache.files.keys().all(|path| within(path, &a)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
