//! Demand parts reference exact sections of per-crate front images. A planning
//! stream can differ from the parser's stream (or belong to an unselected file);
//! only those missing sections are written, in the same existing crate store.
use super::*;
use crate::crate_image::{DemandSlice, DemandWriter};
use crate::image::{Reader, Writer};
use crate::lexer::Tok;
use std::sync::Arc;

pub(super) struct Images {
    name: String,
    path: PathBuf,
    store: Arc<Store>,
    pending: Vec<NewEntry>,
    keys: HashSet<u128>,
    definitions: HashMap<usize, (u128, usize)>,
    main: HashMap<u128, Option<(&'static [u8], &'static [Tok])>>,
    reused_src: usize,
    reused_toks: usize,
    written: usize,
}

impl Images {
    pub(super) fn open(dir: &std::path::Path, name: String) -> Option<Self> {
        let path = dir.join(&name);
        Some(Self { name, store: Arc::new(Store::open(&path)?), path,
            pending: Vec::new(), keys: HashSet::new(), definitions: HashMap::new(), main: HashMap::new(), reused_src: 0, reused_toks: 0, written: 0 })
    }

    fn add(&mut self, key: u128, image: Writer<'_>) {
        if self.keys.insert(key) && !self.store.index.binary_search_by_key(&key, |e| e.key).ok()
            .is_some_and(|ix| self.store.index[ix].kind == KIND_FRONT && self.store.verify_at(ix)) {
            let payload = image.finish();
            self.written += payload.len();
            self.pending.push(NewEntry { key, kind: KIND_FRONT, payload });
        }
    }

    pub(super) fn finish(self, profile: bool, name: &str) -> Option<()> {
        if profile {
            eprintln!("rust-demand-image-bytes: crate={name} reused_src={} reused_toks={} added={}",
                self.reused_src, self.reused_toks, self.written);
        }
        if !self.pending.is_empty() { crate::store::append(&self.path, self.pending, false).ok()?; }
        Some(())
    }
}

impl DemandWriter for Images {
    fn store_name(&self) -> &str { &self.name }

    fn surface(&mut self, path: &str, hash: u128, src: &[u8], toks: &[Tok]) -> Option<(DemandSlice, DemandSlice)> {
        let main = Cache::front_key(path, hash);
        let image = *self.main.entry(main).or_insert_with(|| {
            let ix = self.store.index.binary_search_by_key(&main, |e| e.key).ok()?;
            let r = Reader::new(checked_front_entry(&self.store, ix, main)?)?;
            Some((r.slice::<u8>(0, 1)?, r.slice::<Tok>(1, 2)?))
        });
        let src_hash = crate::crate_image::file_hash("", src);
        let toks_hash = crate::crate_image::demand_token_hash(toks);
        let same_src = image.is_some_and(|(s, _)| s.starts_with(src));
        let token_start = image.and_then(|(_, ts)| {
            if toks.is_empty() { return Some(0); }
            // Source positions make the first token selective even for large files.
            // Compare fields, never padding in the native Tok representation.
            ts.iter().enumerate().find_map(|(i, t)| {
                if (t.kind, t.lo, t.hi) != (toks[0].kind, toks[0].lo, toks[0].hi) { return None; }
                let candidate = ts.get(i..i.checked_add(toks.len())?)?;
                candidate.iter().zip(toks).all(|(a, b)| (a.kind, a.lo, a.hi) == (b.kind, b.lo, b.hi)).then_some(i)
            })
        });
        let mut key = Sip128::new();
        key.str("demand-front-surface-v1"); key.str(path); key.u128(hash);
        key.u128(src_hash); key.u128(toks_hash);
        key.u8(same_src as u8); key.u8(token_start.is_some() as u8);
        let key = key.finish128();
        if !same_src || token_start.is_none() {
            let mut image = Writer::default();
            image.add(1, if same_src { &[] } else { src });
            image.add(2, if token_start.is_some() { &[] } else { toks });
            self.add(key, image);
        }
        if same_src { self.reused_src += src.len(); }
        if token_start.is_some() { self.reused_toks += std::mem::size_of_val(toks); }
        Some((DemandSlice { image: if same_src { main } else { key }, start: 0, len: src.len(), hash: src_hash },
            DemandSlice { image: if token_start.is_some() { main } else { key }, start: token_start.unwrap_or(0), len: toks.len(), hash: toks_hash }))
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

/// Each part reader owns its mappings/memo. A retained part names the original
/// store, not the edited graph's new fingerprint. Missing/pruned images invalidate
/// that part; stale parts can still be read solely for boundary comparison.
#[derive(Default)]
pub(super) struct ReadImages {
    stores: HashMap<String, Arc<Store>>,
    images: HashMap<(String, u128), &'static [u8]>,
}
impl ReadImages {
    pub(super) fn get(&mut self, dir: &std::path::Path, name: &str, key: u128) -> Option<&'static [u8]> {
        let id = (name.to_owned(), key);
        if let Some(bytes) = self.images.get(&id) { return Some(*bytes); }
        // The encoded name is a basename, never a path supplied to the filesystem.
        if !name.ends_with(".rcs") || std::path::Path::new(name).components().count() != 1
            || !matches!(std::path::Path::new(name).components().next(), Some(std::path::Component::Normal(_))) { return None; }
        if !self.stores.contains_key(name) {
            self.stores.insert(name.to_owned(), Arc::new(Store::open(&dir.join(name))?));
        }
        let store = self.stores.get(name)?;
        let ix = store.index.binary_search_by_key(&key, |e| e.key).ok()?;
        // Do not expose unchecked native token discriminants to a planning worker.
        let bytes = checked_front_entry(store, ix, key)?;
        self.images.insert(id, bytes);
        Some(bytes)
    }
}
