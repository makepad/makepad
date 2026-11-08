//! The pictures the person sent in a conversation, kept with it. Each is a file beside the saved
//! transcript (`<key>.pictures/<sha256>.<ext>`): the picture as it was dropped, at its own size
//! and in its own format, one file however often it is sent. The transcript's lines refer to
//! them ([`PictureRef`]); the model gets a copy fitted to its size with the line, and after that
//! only when it asks (`chat.picture`).
//!
//! A picture's file is written on the host's task pool ([`PictureWrite`], from
//! [`super::EngineEvent::KeepPicture`]), never by the send: until it is written the picture's
//! bytes stay in memory ([`SentPicture::data`]), and what needs the file waits for it
//! ([`SentPicture::kept_path`]).

use crate::pictures::PictureRef;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// A picture for the person's next line ([`super::EngineCore::send_pictures`]).
#[derive(Clone, Debug)]
pub struct NewPicture {
    /// As the person named it (`keys.webp`); the conversation numbers it apart from another
    /// picture of that name.
    pub name: String,
    /// The picture as it was dropped.
    pub original: Arc<[u8]>,
    /// Its file: the SHA-256 of `original` in hex, and its format's extension ([`picture_file`]).
    pub file: String,
    /// The original's pixels.
    pub width: u32,
    pub height: u32,
    /// The copy the model gets with the line: a PNG fitted to its size.
    pub for_model: Arc<[u8]>,
}

/// A picture the person sent in this conversation.
#[derive(Clone, Debug)]
pub struct SentPicture {
    /// Its name in this conversation: no other picture of it has this one.
    pub name: String,
    /// Its file name ([`picture_file`]).
    pub file: String,
    /// Its pixels and its file's bytes, as it was dropped.
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// The person's lines it came with, counted from 1.
    pub turns: Vec<usize>,
    /// Where its bytes are, shared by every copy of it.
    source: Arc<Source>,
}

/// Where a sent picture's bytes are: its file, or this run's memory while there is none.
struct Source {
    state: Mutex<State>,
    /// Told when a file being written is (or will not be).
    settled: Condvar,
}

enum State {
    /// Its file is being written to this path; the bytes stay here until it is.
    Writing(PathBuf, Arc<[u8]>),
    /// Its file holds it.
    Kept(PathBuf),
    /// It has no file (the conversation is not kept, the write failed): this run's bytes, and
    /// why.
    Unkept(Arc<[u8]>, String),
    /// Its chat was cleared, its file with it.
    Gone,
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.state.lock().unwrap() {
            State::Writing(path, _) => write!(f, "writing {}", path.display()),
            State::Kept(path) => write!(f, "kept {}", path.display()),
            State::Unkept(_, why) => write!(f, "unkept: {why}"),
            State::Gone => write!(f, "gone"),
        }
    }
}

impl Source {
    fn new(state: State) -> Arc<Source> {
        Arc::new(Source { state: Mutex::new(state), settled: Condvar::new() })
    }

    fn settle(&self, state: &mut State, to: State) {
        *state = to;
        self.settled.notify_all();
    }
}

/// The same picture, sent with the same lines: one source of its bytes.
impl PartialEq for SentPicture {
    fn eq(&self, other: &SentPicture) -> bool {
        self.name == other.name && self.file == other.file && self.turns == other.turns && Arc::ptr_eq(&self.source, &other.source)
    }
}

impl Eq for SentPicture {}

impl SentPicture {
    /// One sent in this run, its bytes in memory: its file is written to `dir` (none: the
    /// conversation is not kept) by the [`PictureWrite`] that comes with it.
    pub(crate) fn sent(name: String, file: String, width: u32, height: u32, original: Arc<[u8]>, dir: Option<&Path>) -> (SentPicture, Option<PictureWrite>) {
        let bytes = original.len() as u64;
        let source = Source::new(State::Unkept(original, "this conversation is not kept".into()));
        let picture = SentPicture { name, file, width, height, bytes, turns: Vec::new(), source };
        let write = dir.and_then(|dir| picture.keep_in(dir));
        (picture, write)
    }

    /// One a saved conversation names, its file at `path`.
    pub(crate) fn restored(picture: PictureRef, path: PathBuf) -> SentPicture {
        let source = Source::new(State::Kept(path));
        SentPicture { name: picture.name, file: picture.file, width: picture.width, height: picture.height, bytes: picture.bytes, turns: Vec::new(), source }
    }

    /// Its file is to be written to `dir`, if it has none: the write that does it.
    pub(crate) fn keep_in(&self, dir: &Path) -> Option<PictureWrite> {
        let mut state = self.source.state.lock().unwrap();
        let State::Unkept(bytes, _) = &*state else { return None };
        let bytes = bytes.clone();
        *state = State::Writing(dir.join(&self.file), bytes.clone());
        Some(PictureWrite { name: self.name.clone(), dir: dir.to_path_buf(), file: self.file.clone(), bytes, source: self.source.clone() })
    }

    /// Its chat was cleared: its file goes, and a write under way leaves none.
    pub(crate) fn forget(&self) {
        let mut state = self.source.state.lock().unwrap();
        self.source.settle(&mut state, State::Gone);
    }

    /// The picture as it was dropped: this run's bytes while its file is being written (or when
    /// it has none), else its file's (a read of up to tens of MB: off the UI thread).
    pub fn data(&self) -> Result<Arc<[u8]>, String> {
        let path = match &*self.source.state.lock().unwrap() {
            State::Writing(_, bytes) | State::Unkept(bytes, _) => return Ok(bytes.clone()),
            State::Kept(path) => path.clone(),
            State::Gone => return Err(format!("{} went with its chat", self.name)),
        };
        std::fs::read(&path).map(Arc::from).map_err(|e| format!("{}: its file does not read ({e})", self.name))
    }

    /// Its file, once it is written: waits while it is being written (on the task pool, a few
    /// ms a MB). Blocks: never on the UI thread, which asks [`Self::kept_now`].
    pub fn kept_path(&self) -> Result<PathBuf, String> {
        let mut state = self.source.state.lock().unwrap();
        while let State::Writing(..) = &*state {
            state = self.source.settled.wait(state).unwrap();
        }
        self.settled(&state).unwrap()
    }

    /// Its file if it is written, the reason if it has none, or `None` while it is being written.
    pub fn kept_now(&self) -> Option<Result<PathBuf, String>> {
        self.settled(&self.source.state.lock().unwrap())
    }

    fn settled(&self, state: &State) -> Option<Result<PathBuf, String>> {
        match state {
            State::Writing(..) => None,
            State::Kept(path) => Some(Ok(path.clone())),
            State::Unkept(_, why) => Some(Err(format!("{} is not kept on this machine: {why}", self.name))),
            State::Gone => Some(Err(format!("{} went with its chat", self.name))),
        }
    }

    pub fn reference(&self) -> PictureRef {
        PictureRef { name: self.name.clone(), file: self.file.clone(), width: self.width, height: self.height, bytes: self.bytes }
    }
}

/// A sent picture's file to write: the host runs it on its task pool
/// ([`super::EngineEvent::KeepPicture`]). Until it ends the picture's bytes stay in memory, and
/// [`SentPicture::kept_path`] waits for it.
#[derive(Clone)]
pub struct PictureWrite {
    pub name: String,
    dir: PathBuf,
    file: String,
    bytes: Arc<[u8]>,
    source: Arc<Source>,
}

impl fmt::Debug for PictureWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PictureWrite({} -> {})", self.name, self.dir.join(&self.file).display())
    }
}

impl PartialEq for PictureWrite {
    fn eq(&self, other: &PictureWrite) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }
}

impl Eq for PictureWrite {}

impl PictureWrite {
    /// Write the file (beside it, then renamed over it: a crash leaves no half picture under the
    /// name). A chat cleared meanwhile keeps no file of it.
    pub fn run(self) {
        let writing = |state: &State| matches!(state, State::Writing(..));
        if !writing(&self.source.state.lock().unwrap()) {
            return;
        }
        let (path, tmp) = (self.dir.join(&self.file), self.dir.join(format!("{}.tmp", self.file)));
        // A file already there is the same picture: its name is its hash.
        let written = if path.is_file() {
            Ok(false)
        } else {
            std::fs::create_dir_all(&self.dir).and_then(|()| std::fs::write(&tmp, &self.bytes)).map(|()| true)
        };
        let mut state = self.source.state.lock().unwrap();
        if !writing(&state) {
            let _ = std::fs::remove_file(&tmp);
            let _ = std::fs::remove_dir(&self.dir);
            return;
        }
        let kept = written.and_then(|fresh| if fresh { std::fs::rename(&tmp, &path) } else { Ok(()) });
        let to = match kept {
            Ok(()) => State::Kept(path),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                State::Unkept(self.bytes.clone(), format!("{}: {e}", path.display()))
            }
        };
        self.source.settle(&mut state, to);
    }

    /// The host could not run it: the picture stays in memory, without a file.
    pub fn abandon(&self, why: String) {
        let mut state = self.source.state.lock().unwrap();
        if matches!(&*state, State::Writing(..)) {
            self.source.settle(&mut state, State::Unkept(self.bytes.clone(), why));
        }
    }
}

/// A picture's file name: the SHA-256 of its bytes in hex, and `ext` (`png`, `jpg`, `webp`).
pub fn picture_file(bytes: &[u8], ext: &str) -> String {
    format!("{}.{ext}", makepad_core_util::sha256::sha256_hex(bytes))
}

/// A file a pictures folder holds: [`picture_file`]'s name, or one being written (`.tmp`).
pub(crate) fn is_picture_file(name: &str) -> bool {
    let name = name.strip_suffix(".tmp").unwrap_or(name);
    name.split_once('.').is_some_and(|(hash, ext)| {
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}

/// Remove the pictures in `dir` that `keep` does not name (nor the ones being written of
/// them), and the folder once it is empty. Only picture files are touched: anything else put
/// there stays, and so does the folder.
pub(crate) fn sweep(dir: &Path, keep: &HashSet<&str>) {
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    for entry in listing.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_picture_file(name) && !keep.contains(name.strip_suffix(".tmp").unwrap_or(name)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// `name`, or the first of `stem-2.ext`, `stem-3.ext`, ... that `taken` does not hold.
pub(crate) fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2u32..).map(|n| format!("{stem}-{n}{ext}")).find(|candidate| !taken(candidate)).unwrap()
}
