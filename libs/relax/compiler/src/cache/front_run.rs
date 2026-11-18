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
static OPEN: Mutex<Option<Open>> = Mutex::new(None);
type Open = (HashMap<PathBuf, Arc<Store>>, HashSet<(PathBuf, u128)>);

/// `f` over OPEN.
fn with_open<R>(f: impl FnOnce(&mut HashMap<PathBuf, Arc<Store>>, &mut HashSet<(PathBuf, u128)>) -> R) -> R {
    let mut guard = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let (open, missing) = guard.get_or_insert_with(Default::default);
    f(open, missing)
}

/// The images written so far, per run store of this process: what references made now can
/// name (commit_marks makes exactly those durable).
pub type RunMarks = Vec<(Arc<RunStore>, usize)>;
pub fn run_marks() -> RunMarks {
    RUNS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|r| (r.clone(), r.written())).collect()
}

/// Commits the images `marks` covers (store.rs RunStore::commit_upto): what a crate store is
/// about to refer to is then visible to other processes, and durable once the seal thread seals
/// it (until then an open verifies it). A commit made for an earlier or concurrent writer that
/// already covers them costs nothing.
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
        if !self.off.swap(true, std::sync::atomic::Ordering::Relaxed) && crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("relax front: front images not written ({}): files stay on the heap", e);
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
    fn file(&self, path: &str, pf: &mut ParsedFile) -> Option<usize> {
        if pf.src.is_mapped() { return None; }
        let key = Cache::front_key(path, pf.source_hash(path));
        let p = crate::store::profiling();
        let s = crate::load_profile::span(p, "front-image-encode", path);
        let w = crate::front::image(pf);
        drop(s);
        let s = crate::load_profile::span(p, "front-image-append", path);
        let (loc, img) = self.write(key, &w)?;
        drop(s);
        let s = crate::load_profile::span(p, "front-image-free", path);
        drop(w);
        drop(s);
        let _s = crate::load_profile::span(p, "front-image-apply", path);
        if !crate::front::apply(path, pf, img) { return None; }
        crate::front::RECORDED_FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        pf.image = Some(loc);
        Some(img.len())
    }

    fn part(&self, pf: &ParsedFile, fp: &mut FilePart, ast: &mut crate::ast::Ast) -> usize {
        // the path the file was published (and its image keyed) under
        let Some((path, hash)) = pf.source_hash.get().cloned() else { return 0 };
        let Some(w) = crate::front::part_image(pf, fp, ast) else { return 0 };
        let Some((loc, img)) = self.write(Cache::front_part_key(&path, hash), &w) else { return 0 };
        drop(w);
        if crate::front::map_part(&path, pf, fp, ast, img) {
            crate::front::PARTS_RECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            fp.image = Some(loc);
        }
        img.len()
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
/// stores), mapped for the rest of the process. `check`: its checksum is verified now (else it is
/// proven by Store::get_at, as any mapped front image's: its epoch, or a check at its first read).
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
    let (st, i) = with_open(|open, missing| {
        if missing.contains(&(path.clone(), key)) { return None; }
        let opened = open.get(&path).cloned();
        match opened.as_ref().and_then(|st| Some((st.clone(), find(st)?))) {
            Some(x) => Some(x),
            None => {
                // A first open, or a key the opened index lacks: the store is mapped again
                // only when its committed header changed since (a newer index); a key it still
                // lacks is remembered as missing (a dangling reference: its crate is parsed).
                let changed = opened.as_ref().is_none_or(|st| crate::store::header_generation(&path) != Some(st.generation));
                let found = if changed {
                    Store::open(&path).map(Arc::new).and_then(|st| {
                        // FRONT_STORES keeps every opened mapping for the rest of the process
                        pin_front(&st);
                        open.insert(path.clone(), st.clone());
                        Some((st.clone(), find(&st)?))
                    })
                } else {
                    None
                };
                match found {
                    Some(x) => Some(x),
                    None => { missing.insert((path, key)); None }
                }
            }
        }
    })?;
    let (kind, img) = st.get_at(i, key)?;
    if kind != KIND_FRONT || (check && !st.verify_at(i)) { return None; }
    // SAFETY: FRONT_STORES keeps the store (and its mapping) for the rest of the process, and a
    // committed payload never changes (store.rs)
    let img: &'static [u8] = unsafe { std::slice::from_raw_parts(img.as_ptr(), img.len()) };
    Some((ImageLoc { store: name.into(), key }, img))
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&OPEN, Mutex::new(None));
    reset(&RUNS, Mutex::new(Vec::new()));
    reset(&SINKS, Mutex::new(None));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Ix, Store};
    use std::io::{Seek, Write};

    /// The folder the writer (this test's child process) writes in.
    const WRITER: &str = "RELAX_TEST_RUN_STORE_WRITER";
    /// A folder per way the writer's second commit reached the disk: all of it (only the process
    /// died), and three ways a machine crash leaves it.
    const SHAPES: [&str; 4] = ["whole", "zeroed", "cut", "stale"];
    const CRATE_STORE: &str = "core-00112233445566ee.rcs";
    const IMAGE: usize = 3000;
    /// The writer's epoch (store.rs set_epoch): an earlier boot's, to the test's process.
    const WRITER_EPOCH: u64 = 0x5eed_0001;

    fn image(b: u8) -> Vec<u8> {
        (0..IMAGE).map(|i| b.wrapping_add(i as u8)).collect()
    }

    /// The writer: in each shape's folder a run store with two commits (image 1, then image 2,
    /// committed as a crate store's write commits the images its references name: cache.rs
    /// append_store), a crate store referring to both, and on stdout what a collected program
    /// would certify of the run store's commit (its index and generation).
    fn write_and_end(root: &Path) {
        crate::store::set_epoch(WRITER_EPOCH);
        // (the harness's "test <name> ... " has no line end of its own)
        println!();
        for shape in SHAPES {
            let dir = root.join(shape);
            std::fs::create_dir_all(&dir).unwrap();
            let run = RunStore::create(&dir, RUN_PREFIX, RUN_CAP).unwrap();
            let put = |b: u8| match run.append(b as u128, KIND_FRONT, IMAGE, &|out| out.write_all(&image(b))) {
                Appended::Done(key, _) => key,
                _ => panic!("the image append"),
            };
            let one = put(1);
            run.commit().unwrap();
            let two = put(2);
            run.commit_upto(run.written()).unwrap();
            let refs = [(1u128, one), (2, two)].into_iter()
                .map(|(k, key)| NewEntry { key: k, kind: KIND_FRONT_REF, payload: encode_ref(&ImageLoc { store: run.name.to_string().into(), key }) })
                .collect();
            crate::store::append(&dir.join(CRATE_STORE), refs, false).unwrap();
            let st = Store::open(&run.path).unwrap();
            let ix: Vec<String> = st.index.iter().map(|e| {
                let (key, off, len, kind, sum) = (e.key, e.off, e.len, e.kind, e.sum);
                format!("{key:x}:{off}:{len}:{kind}:{sum:x}")
            }).collect();
            println!("certificate {shape} {} {one:x} {two:x} {}", st.generation, ix.join(","));
        }
    }

    /// A run store's commits wait on no barrier (one on the cache writer, under SPILL_IO, once
    /// stalled every spill behind it for 11.6 s). A process of a later boot (here: of another epoch
    /// than the writer's) that follows a crate store's reference into the run store, or whose
    /// collected program certified the commit, checks each image at its first read. One whose
    /// payload did not reach the disk before its machine went down (zeroed: the writes landed out
    /// of order; the file cut at the first commit's end; or stale bytes in its place) is a miss
    /// there, never a bad read: the reference gives way (its file is parsed again) and so does the
    /// certified index's entry, while the first commit's image stays warm.
    #[test]
    fn an_earlier_boots_run_store_image_is_checked_and_a_lost_one_is_a_miss() {
        if let Some(root) = std::env::var_os(WRITER) {
            write_and_end(Path::new(&root));
            return;
        }
        let root = std::env::temp_dir().join(format!("relax-run-seal-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cache::front_run::tests::an_earlier_boots_run_store_image_is_checked_and_a_lost_one_is_a_miss", "--nocapture", "--test-threads=1"])
            .env(WRITER, &root));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && stdout.contains(" 1 passed;"), "the writer: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        for shape in SHAPES {
            let line = stdout.lines().find_map(|l| l.strip_prefix(&format!("certificate {shape} ")))
                .unwrap_or_else(|| panic!("the writer's certificate of {shape}: {stdout}"));
            let w: Vec<&str> = line.split(' ').collect();
            let generation: u64 = w[0].parse().unwrap();
            let (one, two) = (u128::from_str_radix(w[1], 16).unwrap(), u128::from_str_radix(w[2], 16).unwrap());
            let index: Vec<Ix> = w[3].split(',').map(|e| {
                let f: Vec<&str> = e.split(':').collect();
                Ix { key: u128::from_str_radix(f[0], 16).unwrap(), off: f[1].parse().unwrap(), len: f[2].parse().unwrap(), kind: f[3].parse().unwrap(), sum: u64::from_str_radix(f[4], 16).unwrap() }
            }).collect();
            let at = |key: u128| index.iter().find(|e| { let k = e.key; k == key }).map(|e| (e.off, e.len as u64)).unwrap();
            let ((one_off, one_len), (two_off, two_len)) = (at(one), at(two));
            let dir = root.join(shape);
            let run = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path())
                .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(RUN_PREFIX)).unwrap();
            let mut f = std::fs::OpenOptions::new().write(true).open(&run).unwrap();
            let mut put = |off: u64, b: &[u8]| {
                f.seek(std::io::SeekFrom::Start(off)).unwrap();
                f.write_all(b).unwrap();
            };
            match shape {
                "zeroed" => put(two_off, &vec![0; two_len as usize]),
                "stale" => put(two_off, &image(1)),
                _ => {}
            }
            if shape == "cut" {
                // the old tail: the sealed commit's end (the header lies before it)
                f.set_len(one_off + one_len).unwrap();
            }
            drop(f);
            let whole = shape == "whole";
            // a collected program's certified index of the commit
            let certified = Store::open_certified(&run, Box::leak(index.clone().into_boxed_slice()), generation);
            if shape == "cut" {
                assert!(certified.is_none(), "cut: a certified commit past the file's end was opened");
            } else {
                let certified = certified.expect("the certified commit");
                assert_eq!(certified.get(one).unwrap().1, &image(1)[..], "{shape}: the first commit's certified image");
                let got = certified.get(two).map(|x| x.1.to_vec());
                assert_eq!(got, whole.then(|| image(2)), "{shape}: the certified image of the second commit");
            }
            // a crate store's references into it
            let st = Arc::new(Store::open(&dir.join(CRATE_STORE)).unwrap());
            let entry = |k: u128, check: bool| {
                let e = st.index.binary_search_by(|x| { let kx = x.key; kx.cmp(&k) }).unwrap();
                front_entry(&st, e, k, check).map(|(_, img)| img.to_vec())
            };
            if whole {
                assert_eq!(entry(2, false), Some(image(2)), "the image of an earlier boot's commit, checked at its read");
            } else {
                assert_eq!(entry(2, false), None, "{shape}: an image that did not reach the disk was served");
                assert_eq!(entry(2, true), None, "{shape}: an image that did not reach the disk was served, checked");
            }
            assert_eq!(entry(1, true), Some(image(1)), "{shape}: the first commit's image stays warm");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A warm start in the epoch that wrote its cache reads no front-image payload: the images a
    /// crate store's references name are served unchecked (the kernel that took their writes still
    /// runs), and the background pass after the start has nothing to check. The next boot's first
    /// start checks each image and reference once, at its first use, and the background pass then
    /// seals them (it reads them again, from the disk), so the start after reads none either.
    #[test]
    fn a_warm_start_in_the_caches_epoch_reads_no_front_image_payload() {
        isolated_test!(a_warm_start_in_the_caches_epoch_reads_no_front_image_payload);
        let dir = std::env::temp_dir().join(format!("relax-warm-reads-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::store::set_epoch(0xa1);
        let run = RunStore::create(&dir, RUN_PREFIX, RUN_CAP).unwrap();
        let keys: Vec<u128> = [1u8, 2].into_iter().map(|b| match run.append(b as u128, KIND_FRONT, IMAGE, &|out| out.write_all(&image(b))) {
            Appended::Done(key, _) => key,
            _ => panic!("the image append"),
        }).collect();
        run.commit().unwrap();
        let refs = keys.iter().zip(1u128..).map(|(&key, k)| NewEntry { key: k, kind: KIND_FRONT_REF, payload: encode_ref(&ImageLoc { store: run.name.to_string().into(), key }) }).collect();
        crate::store::append(&dir.join(CRATE_STORE), refs, false).unwrap();
        drop(run);
        // a start, as a later process makes it: nothing of this process's writes is held
        let start = |when: &str| {
            unsafe { super::reset_statics() };
            crate::store::payload_reads();
            let files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            let st = Arc::new(Store::open(&dir.join(CRATE_STORE)).unwrap_or_else(|| panic!("{when}: the crate store does not open; the folder holds {files:?}")));
            for k in 1..=2u128 {
                let e = st.index.binary_search_by(|x| { let kx = x.key; kx.cmp(&k) }).unwrap();
                assert_eq!(front_entry(&st, e, k, false).map(|(_, img)| img.to_vec()), Some(image(k as u8)), "{when}: image {k}");
            }
            let at_start = crate::store::payload_reads();
            super::super::background_checks();
            (at_start, crate::store::payload_reads())
        };
        assert_eq!(start("the cache's epoch"), (0, 0), "a warm start in the cache's epoch read front-image payloads (at its start, after it)");
        crate::store::set_epoch(0xb1);
        assert_eq!(start("the next boot"), (4, 4), "the next boot: each image and reference checked once at its first use, then sealed");
        assert_eq!(start("after the seal"), (0, 0), "the start after the seal read front-image payloads");
        let _ = std::fs::remove_dir_all(&dir);
    }
}