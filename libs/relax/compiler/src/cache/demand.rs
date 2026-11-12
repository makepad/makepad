//! Demand parts reference exact sections of per-crate front images. A planning
//! stream can differ from the parser's stream (or belong to an unselected file);
//! only those missing sections are written, in the same existing crate store.
use super::*;
use crate::crate_image::{DemandSlice, DemandWriter};
use crate::image::{Reader, Writer};
use crate::lexer::Tok;
use std::sync::Arc;

/// The stores one parallel job's crates read, each opened once: every crate's part names the same
/// run store, and opening one copies its index.
pub(super) struct OpenStores(relax_session::sync::Mutex<HashMap<String, Arc<Store>>>);

impl OpenStores {
    pub(super) fn new() -> Self {
        Self(relax_session::sync::Mutex::new(HashMap::new()))
    }

    /// The store `name` under `dir` with `key`'s place in its index. A key it lacks may have been
    /// committed since it was opened: only then is it opened again.
    fn find(&self, dir: &std::path::Path, name: &str, key: u128) -> Option<(Arc<Store>, usize)> {
        let at = |st: &Store| st.index.binary_search_by_key(&key, |e| e.key).ok();
        let mut stores = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(st) = stores.get(name) {
            if let Some(i) = at(st) { return Some((st.clone(), i)); }
        }
        let st = Arc::new(Store::open(&dir.join(name))?);
        stores.insert(name.to_owned(), st.clone());
        let i = at(&st)?;
        Some((st, i))
    }
}

pub(super) struct Images<'a> {
    name: String,
    path: PathBuf,
    store: Arc<Store>,
    pending: Vec<NewEntry>,
    keys: HashSet<u128>,
    definitions: HashMap<usize, (u128, usize)>,
    /// per file image key: the store and entry holding it (the crate store's own, or the run
    /// store its reference names, mapped here: never the process's zone-scoped front mappings)
    main: HashMap<u128, Option<(Arc<Store>, usize)>>,
    runs: &'a OpenStores,
    reused_src: usize,
    written: usize,
}

impl<'a> Images<'a> {
    pub(super) fn open(dir: &std::path::Path, name: String, runs: &'a OpenStores) -> Option<Self> {
        let path = dir.join(&name);
        Some(Self { name, store: Arc::new(Store::open(&path)?), path,
            pending: Vec::new(), keys: HashSet::new(), definitions: HashMap::new(), main: HashMap::new(), runs, reused_src: 0, written: 0 })
    }

    fn add(&mut self, key: u128, image: Writer<'_>) {
        if self.keys.insert(key) && !self.store.index.binary_search_by_key(&key, |e| e.key).ok()
            .is_some_and(|ix| self.store.index[ix].kind == KIND_FRONT && self.store.verify_at(ix)) {
            let payload = image.finish();
            self.written += payload.len();
            self.pending.push(NewEntry { key, kind: KIND_FRONT, payload });
        }
    }

    /// The file image `key` of this crate store: inline, or in the run store its reference
    /// names; checksummed.
    fn file_image(&mut self, key: u128) -> Option<(Arc<Store>, usize)> {
        let ix = self.store.index.binary_search_by_key(&key, |e| e.key).ok()?;
        let (kind, b) = self.store.get_at(ix, key)?;
        let (st, i) = match kind {
            KIND_FRONT => (self.store.clone(), ix),
            KIND_FRONT_REF => {
                let (name, k) = front_run::decode_ref(b)?;
                if !crate::store::name_ours(name) { return None; }
                self.runs.find(self.path.parent()?, name, k)?
            }
            _ => return None,
        };
        (st.index[i].kind == KIND_FRONT && st.verify_at(i)).then_some((st, i))
    }

    pub(super) fn finish(self, profile: bool, name: &str) -> Option<()> {
        if profile {
            eprintln!("rust-demand-image-bytes: crate={name} reused_src={} added={}", self.reused_src, self.written);
        }
        if !self.pending.is_empty() { crate::store::append(&self.path, self.pending, false).ok()?; }
        Some(())
    }
}

impl DemandWriter for Images<'_> {
    fn store_name(&self) -> &str { &self.name }

    fn surface(&mut self, path: &str, hash: u128, src: &[u8], toks: &[Tok]) -> Option<(DemandSlice, DemandSlice)> {
        // The source is the file's own image's (a crate store reference to it, front_run.rs)
        // when that holds the same bytes; the scan's tokens are always stored here: a file image
        // keeps its tokens packed (PackedToks), the planner reads plain `Tok` slices.
        let main = Cache::front_key(path, hash);
        if !self.main.contains_key(&main) {
            let found = self.file_image(main);
            self.main.insert(main, found);
        }
        let same_src = self.main[&main].as_ref().is_some_and(|(st, i)| {
            let Some((_, b)) = st.get_at(*i, st.index[*i].key) else { return false };
            // SAFETY: only read here, while `st` (held in self.main) keeps the mapping
            let b: &'static [u8] = unsafe { std::slice::from_raw_parts(b.as_ptr(), b.len()) };
            Reader::new(b).and_then(|r| r.slice::<u8>(0, 1)).is_some_and(|s| s.starts_with(src))
        });
        let src_hash = crate::crate_image::file_hash("", src);
        let toks_hash = crate::crate_image::demand_token_hash(toks);
        let mut key = Sip128::new();
        key.str("demand-front-surface-v2"); key.str(&crate::dirs::anchored(path)); key.u128(hash);
        // the worktree: payloads still hold its absolute paths (Target B removes this per payload)
        key.u128(crate::dirs::anchors_key());
        key.u128(src_hash); key.u128(toks_hash);
        key.u8(same_src as u8);
        let key = key.finish128();
        let mut part = Writer::default();
        part.add(1, if same_src { &[] } else { src });
        part.add(2, toks);
        self.add(key, part);
        if same_src { self.reused_src += src.len(); }
        Some((DemandSlice { image: if same_src { main } else { key }, start: 0, len: src.len(), hash: src_hash },
            DemandSlice { image: key, start: 0, len: toks.len(), hash: toks_hash }))
    }

    fn definition(&mut self, def: &crate::macros::MacroDef) -> Option<(u128, usize)> {
        // Definitions are immutable and owned by the part for this entire encode.
        // Share repeated deferred-table references without serializing them again.
        let identity = def as *const crate::macros::MacroDef as usize;
        if let Some(value) = self.definitions.get(&identity) { return Some(*value); }
        let bytes = crate::crate_image::encode(def);
        let key = crate::crate_image::file_hash("demand-front-definition-v1", &bytes);
        let value = (key, bytes.len());
        let mut image = Writer::default();
        image.add_bytes(3, bytes);
        self.add(key, image);
        self.definitions.insert(identity, value);
        Some(value)
    }
}

/// Each part reader owns its memo. A retained part names the original store, not the
/// edited graph's new fingerprint. Missing/pruned images invalidate that part; stale parts
/// can still be read solely for boundary comparison.
pub(super) struct ReadImages<'a> {
    stores: &'a OpenStores,
    images: HashMap<(String, u128), &'static [u8]>,
}
impl<'a> ReadImages<'a> {
    pub(super) fn new(stores: &'a OpenStores) -> Self {
        Self { stores, images: HashMap::new() }
    }

    pub(super) fn get(&mut self, dir: &std::path::Path, name: &str, key: u128) -> Option<&'static [u8]> {
        let id = (name.to_owned(), key);
        if let Some(bytes) = self.images.get(&id) { return Some(*bytes); }
        // The encoded name is a basename, never a path supplied to the filesystem.
        if !name.ends_with(".rcs") || std::path::Path::new(name).components().count() != 1
            || !matches!(std::path::Path::new(name).components().next(), Some(std::path::Component::Normal(_))) { return None; }
        let (store, ix) = self.stores.find(dir, name, key)?;
        // Do not expose unchecked native token discriminants to a planning worker.
        let bytes = checked_front_entry(&store, ix, key)?;
        self.images.insert(id, bytes);
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One open for a whole job, however many crates ask; a key committed after that open is found.
    #[test]
    fn a_store_is_opened_once_and_again_for_a_newer_key() {
        let dir = std::env::temp_dir().join(format!("relax-open-stores-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = "front_run-0123456789abcdef.rcs";
        let put = |key: u128| crate::store::append(&dir.join(name), vec![NewEntry { key, kind: KIND_FRONT, payload: vec![key as u8; 16] }], false).unwrap();
        put(1);
        let stores = OpenStores::new();
        let (a, _) = stores.find(&dir, name, 1).unwrap();
        let (b, _) = stores.find(&dir, name, 1).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(stores.find(&dir, name, 2).is_none());
        put(2);
        let (c, i) = stores.find(&dir, name, 2).unwrap();
        assert_eq!(c.get_at(i, 2).unwrap().1, [2u8; 16]);
        drop((a, b, c, stores));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
