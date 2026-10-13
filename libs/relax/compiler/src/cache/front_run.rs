//! The run's front image store (front.rs): every file this process parses is written once, at
//! its publication, into one run store (`front_run-<16 hex>.rcs`, store.rs RunStore) and read
//! through that mapping from then on; a lazily parsed part likewise before it is published. A
//! crate store (named by the crate fingerprint, which a file's publication does not know yet)
//! refers to the images by (run store, key) entries (KIND_FRONT_REF); warm runs follow those
//! references (`resolve`) and read the same bytes through the same functions.
use super::*;
use crate::parser::{FilePart, ImageLoc, ImageSink, ParsedFile};
use crate::store::{Appended, RunStore};
use std::path::Path;
use std::sync::Arc;
use relax_session::sync::Mutex;

/// index entries per run store (a full one is followed by another)
const RUN_CAP: u32 = 16384;
const RUN_PREFIX: &str = "front_run";

/// The run stores this process writes: committed before a crate store refers to them.
static RUNS: Mutex<Vec<Arc<RunStore>>> = Mutex::new(Vec::new());
static SINKS: Mutex<Option<HashMap<PathBuf, Arc<FrontSink>>>> = Mutex::new(None);
/// Run stores opened to resolve references, and keys found missing in them.
static OPEN: Mutex<Option<(HashMap<PathBuf, Arc<Store>>, HashSet<(PathBuf, u128)>)>> = Mutex::new(None);

pub(super) struct ZoneTaken {
    runs: Vec<Arc<RunStore>>,
    sinks: Option<HashMap<PathBuf, Arc<FrontSink>>>,
    open: Option<(HashMap<PathBuf, Arc<Store>>, HashSet<(PathBuf, u128)>)>,
}

pub(super) fn zone_take() -> ZoneTaken {
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let mut sinks = SINKS.lock().unwrap_or_else(|e| e.into_inner());
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken {
        runs: std::mem::take(&mut *runs),
        sinks: std::mem::take(&mut *sinks),
        open: std::mem::take(&mut *open),
    };
    #[cfg(test)]
    { assert!(runs.is_empty()); assert!(sinks.is_none()); assert!(open.is_none()); }
    taken
}

pub(super) fn zone_put_back(taken: ZoneTaken) {
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *runs, taken.runs));
    let mut sinks = SINKS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *sinks, taken.sinks));
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *open, taken.open));
}

/// The images written so far, per run store of this process: what references made now can
/// name (commit_marks makes exactly those durable).
pub type RunMarks = Vec<(Arc<RunStore>, usize)>;
pub fn run_marks() -> RunMarks {
    RUNS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|r| (r.clone(), r.written())).collect()
}

/// Commits the images `marks` covers (store.rs RunStore::commit_upto): what a crate store is
/// about to refer to is then durable and visible to other processes. A commit made for an
/// earlier or concurrent writer that already covers them costs nothing.
pub fn commit_marks(marks: &RunMarks) -> Result<(), String> {
    for (r, n) in marks {
        r.commit_upto(*n).map_err(|e| format!("{}: {}", r.path.display(), e))?;
    }
    Ok(())
}

/// Every image written so far.
#[cfg(test)]
pub fn commit_runs() -> Result<(), String> {
    commit_marks(&run_marks())
}

/// Writes the images of the files and parts of the cache folder `dir` (ImageSink).
pub struct FrontSink {
    dir: PathBuf,
    cur: Mutex<Option<Arc<RunStore>>>,
    /// a write failed (disk full, ...): later files stay on the heap
    off: std::sync::atomic::AtomicBool,
}

impl FrontSink {
    /// The sink of the cache folder `dir` (one per folder and process).
    pub fn for_dir(dir: &Path) -> Arc<FrontSink> {
        let mut g = SINKS.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashMap::new).entry(dir.to_path_buf())
            .or_insert_with(|| Arc::new(FrontSink { dir: dir.to_path_buf(), cur: Mutex::new(None), off: Default::default() })).clone()
    }

    fn fail(&self, e: impl std::fmt::Display) {
        if !self.off.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("makepad-run front: front images not written ({}): files stay on the heap", e);
        }
    }

    /// Appends one image under `key` and maps it.
    fn write(&self, key: u128, w: &crate::image::Writer<'_>) -> Option<(ImageLoc, &'static [u8])> {
        if self.off.load(std::sync::atomic::Ordering::Relaxed) { return None; }
        let len = w.len();
        loop {
            let run = {
                let mut cur = self.cur.lock().unwrap_or_else(|e| e.into_inner());
                match cur.as_ref() {
                    Some(r) if !r.full() => r.clone(),
                    _ => {
                        let made = pack::ensure_private_dir(&self.dir).map_err(std::io::Error::other)
                            .and_then(|_| RunStore::create(&self.dir, RUN_PREFIX, RUN_CAP));
                        match made {
                            Ok(r) => {
                                let r = Arc::new(r);
                                RUNS.lock().unwrap_or_else(|e| e.into_inner()).push(r.clone());
                                *cur = Some(r.clone());
                                r
                            }
                            Err(e) => { self.fail(e); return None; }
                        }
                    }
                }
            };
            match run.append(key, KIND_FRONT, len, &|out| w.write_to(out)) {
                Appended::Done(key, bytes) => {
                    crate::front::RECORDED_BYTES.fetch_add(len as u64, std::sync::atomic::Ordering::Relaxed);
                    return Some((ImageLoc { store: run.name.clone(), key }, bytes));
                }
                Appended::Full => {
                    let mut cur = self.cur.lock().unwrap_or_else(|e| e.into_inner());
                    if cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, &run)) { *cur = None; }
                }
                Appended::Failed(e) => { self.fail(e); return None; }
            }
        }
    }
}

impl ImageSink for FrontSink {
    fn file(&self, path: &str, pf: &mut ParsedFile) -> bool {
        if pf.src.is_mapped() { return false; }
        let key = Cache::front_key(path, pf.source_hash(path));
        let p = crate::store::profiling();
        let s = crate::load_profile::span(p, "front-image-encode", path);
        let w = crate::front::image(pf);
        drop(s);
        let s = crate::load_profile::span(p, "front-image-append", path);
        let Some((loc, img)) = self.write(key, &w) else { return false };
        drop(s);
        let s = crate::load_profile::span(p, "front-image-free", path);
        drop(w);
        drop(s);
        let _s = crate::load_profile::span(p, "front-image-apply", path);
        if !crate::front::apply(path, pf, img) { return false; }
        crate::front::RECORDED_FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        pf.image = Some(loc);
        true
    }

    fn part(&self, pf: &ParsedFile, fp: &mut FilePart, ast: &mut crate::ast::Ast) {
        // the path the file was published (and its image keyed) under
        let Some((path, hash)) = pf.source_hash.get().cloned() else { return };
        let Some(w) = crate::front::part_image(pf, fp, ast) else { return };
        let Some((loc, img)) = self.write(Cache::front_part_key(&path, hash), &w) else { return };
        drop(w);
        if crate::front::map_part(&path, pf, fp, ast, img) {
            crate::front::PARTS_RECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            fp.image = Some(loc);
        }
    }
}

/// A crate store's reference to an image: the run store's file name and the entry's key.
pub(super) fn encode_ref(loc: &ImageLoc) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + loc.store.len() + 16);
    b.extend_from_slice(&(loc.store.len() as u32).to_le_bytes());
    b.extend_from_slice(loc.store.as_bytes());
    b.extend_from_slice(&loc.key.to_le_bytes());
    b
}

pub(super) fn decode_ref(b: &[u8]) -> Option<(&str, u128)> {
    let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    let name = std::str::from_utf8(b.get(4..4 + n)?).ok()?;
    let key = u128::from_le_bytes(b.get(4 + n..)?.try_into().ok()?);
    Some((name, key))
}

/// The image a crate store's reference entry names (`b`: its payload; `dir`: the folder of both
/// stores), mapped for the rest of the process. `check`: its checksum is verified now (else in
/// the background, verify_front_images, as any mapped front image's).
pub(super) fn resolve(dir: &Path, b: &[u8], check: bool) -> Option<(ImageLoc, &'static [u8])> {
    let (name, key) = decode_ref(b)?;
    if !crate::store::name_ours(name) { return None; }
    // an image this process wrote: its own mapping (not a second one of the same pages)
    let own = RUNS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|r| &*r.name == name).and_then(|r| r.mapped(key));
    if let Some(img) = own {
        return Some((ImageLoc { store: name.into(), key }, img));
    }
    let path = dir.join(name);
    let find = |st: &Store| st.index.binary_search_by(|e| { let k = e.key; k.cmp(&key) }).ok();
    let (st, i) = {
        let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
        let (open, missing) = guard.get_or_insert_with(Default::default);
        if missing.contains(&(path.clone(), key)) { return None; }
        let opened = open.get(&path).cloned();
        match opened.as_ref().and_then(|st| Some((st.clone(), find(st)?))) {
            Some(x) => x,
            None => {
                // A first open, or a key the opened index lacks: the store is mapped again
                // only when its committed header changed since (a newer index); a key it still
                // lacks is remembered as missing (a dangling reference: its crate is parsed).
                let changed = opened.as_ref().is_none_or(|st| crate::store::header_generation(&path) != Some(st.generation));
                let found = if changed {
                    Store::open(&path).map(Arc::new).and_then(|st| {
                        // FRONT_STORES keeps every opened mapping for the rest of the process
                        FRONT_STORES.lock().unwrap_or_else(|x| x.into_inner()).push(st.clone());
                        open.insert(path.clone(), st.clone());
                        Some((st.clone(), find(&st)?))
                    })
                } else {
                    None
                };
                match found {
                    Some(x) => x,
                    None => { missing.insert((path, key)); return None; }
                }
            }
        }
    };
    let (kind, img) = st.get_at(i, key)?;
    if kind != KIND_FRONT || (check && !st.verify_at(i)) { return None; }
    if !check { FRONT_USED.lock().unwrap_or_else(|x| x.into_inner()).push((st.clone(), i)); }
    // SAFETY: FRONT_STORES keeps the store (and its mapping) for the rest of the process, and a
    // committed payload never changes (store.rs)
    let img: &'static [u8] = unsafe { std::slice::from_raw_parts(img.as_ptr(), img.len()) };
    Some((ImageLoc { store: name.into(), key }, img))
}
