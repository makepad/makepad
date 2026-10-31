//! Authored source identities. IDs belong to one load, never to parser byte offsets.
use std::sync::{Arc, OnceLock};
use relax_session::sync::Mutex;

/// The compact, file-aware part of a proc-macro span. File zero means the current file.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceSpan {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

impl From<crate::pm_tokens::Span> for SourceSpan {
    fn from(s: crate::pm_tokens::Span) -> Self { Self { file: s.file, lo: s.lo, hi: s.hi } }
}

impl SourceSpan {
    pub fn join(self, other: Self) -> Self {
        if self.file == other.file {
            Self { file: self.file, lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
        } else { self }
    }
}

/// One output token's entire authored span, including synthesized tokens of a different size.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenOrigin {
    pub lo: u32,
    pub hi: u32,
    pub source: SourceSpan,
}

// SAFETY: repr(C), five u32s, no padding, pointers or invalid bit patterns.
unsafe impl crate::image::Pod for TokenOrigin {}

impl TokenOrigin {
    pub fn shifted(self, base: u32) -> Self { Self { lo: self.lo + base, hi: self.hi + base, ..self } }
}

pub fn origin_at(origins: &[TokenOrigin], pos: u32) -> Option<SourceSpan> {
    let i = origins.partition_point(|o| o.lo <= pos).checked_sub(1)?;
    let o = origins[i];
    // AST ends (for example Block.hi) are exclusive. If no next token starts here,
    // that boundary still belongs to the preceding token's authored construct.
    (pos <= o.hi).then_some(o.source)
}

pub fn line_starts(src: &[u8]) -> Vec<u32> {
    let mut starts = vec![0];
    starts.extend(src.iter().enumerate().filter_map(|(i, c)| (*c == b'\n').then_some(i as u32 + 1)));
    starts
}

/// Diagnostics and the editor protocol use 1-based UTF-8 byte columns.
pub fn line_col(src: &[u8], starts: &[u32], pos: u32) -> (usize, usize) {
    let pos = pos.min(src.len() as u32);
    let line = starts.partition_point(|s| *s <= pos) - 1;
    (line + 1, (pos - starts[line]) as usize + 1)
}

/// Authored source bytes: shared heap bytes, or a view of a mapped image that outlives the load.
#[derive(Clone)]
pub enum SourceBytes { Shared(Arc<[u8]>), Mapped(&'static [u8]) }

impl std::ops::Deref for SourceBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] { match self { Self::Shared(src) => src, Self::Mapped(src) => src } }
}

impl From<Arc<[u8]>> for SourceBytes {
    fn from(src: Arc<[u8]>) -> Self { Self::Shared(src) }
}

/// Registry entries whose owned bytes differed from their image's source (kept owned).
pub static MAPPED_SOURCE_MISMATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Registry entries switched from owned bytes to their image's source.
pub static MAPPED_SOURCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// `map_authored` calls whose path has no registry entry.
pub static MAPPED_SOURCE_NOT_FOUND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// `map_authored` calls whose registry entry holds no snapshot (never read, or an error).
pub static MAPPED_SOURCE_NO_SNAPSHOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub struct SourceFile {
    pub path: String,
    pub src: SourceBytes,
    pub lines: OnceLock<Vec<u32>>,
    hash: OnceLock<u128>,
}

impl SourceFile {
    pub fn new(path: String, src: SourceBytes) -> Self {
        Self { path, src, lines: OnceLock::new(), hash: OnceLock::new() }
    }

    /// A source snapshot is immutable, including its path identity. Shared
    /// schema readers hash it once, outside their dependency-recording lock.
    pub fn hash(&self) -> u128 {
        *self.hash.get_or_init(|| crate::crate_image::file_hash(&self.path, &self.src))
    }

    pub fn line_col(&self, pos: u32) -> (usize, usize) {
        line_col(&self.src, self.lines.get_or_init(|| line_starts(&self.src)), pos)
    }

    /// Proc-macro columns count characters and are 1-based in relax-lex.
    pub fn token_span(&self, span: SourceSpan) -> crate::pm_tokens::Span {
        let lc = |pos: u32| {
            let (line, _) = self.line_col(pos);
            let lo = self.lines.get().unwrap()[line - 1] as usize;
            let hi = (pos as usize).min(self.src.len());
            let column = self.src[lo..hi].iter().filter(|b| **b & 0xc0 != 0x80).count() + 1;
            crate::pm_tokens::LineColumn { line, column }
        };
        crate::pm_tokens::Span { file: span.file, lo: span.lo, hi: span.hi, start: lc(span.lo), end: lc(span.hi) }
    }
}

struct Entry {
    path: String,
    state: Mutex<EntryState>,
}

#[derive(Default)]
struct EntryState {
    source: Option<Arc<SourceFile>>,
    snapshot: Option<SourceBytes>,
}

impl Entry {
    fn new(path: &str, snapshot: Option<SourceBytes>) -> Arc<Self> {
        Arc::new(Self { path: path.into(), state: Mutex::new(EntryState { source: None, snapshot }) })
    }
    fn state(&self) -> relax_session::sync::MutexGuard<'_, EntryState> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }
}

struct Registry {
    paths: crate::fxhash::HashMap<String, u32>,
    entries: Vec<Arc<Entry>>,
    /// Image-local source IDs can be sparse; preserve them without allocating their gaps.
    imported: crate::fxhash::HashMap<u32, Arc<Entry>>,
    first: u32,
}
impl Default for Registry {
    fn default() -> Self {
        Self { paths: Default::default(), entries: Vec::new(), imported: Default::default(), first: 1 }
    }
}

/// Shared by Program and parsing workers. Snapshots are immutable for the entire load, with one
/// exception: once a file's image is mapped (`map_authored`), the file's bytes are the image's and
/// its read-snapshot entry is released, so an include!/include_str! of that same module file from
/// a later lazy expansion reads the disk again.
#[derive(Default)]
pub struct SourceRegistry {
    identities: Arc<Mutex<Registry>>,
    overrides: crate::fxhash::HashMap<u32, Arc<Entry>>,
}

impl SourceRegistry {
    pub fn overridden_paths(&self) -> Vec<String> {
        self.overrides.values().map(|entry| entry.path.clone()).collect()
    }

    /// Owned authored snapshots, including discovery-only inputs; counted once per registry.
    pub fn heap_bytes(&self) -> u64 {
        let r = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        r.entries.iter().enumerate().map(|(i, e)| (i as u32 + r.first, e))
            .chain(r.imported.iter().map(|(id, e)| (*id, e))).map(|(id, e)| {
            let e = self.overrides.get(&id).unwrap_or(e);
            let st = e.state();
            let owned = |b: &SourceBytes| if let SourceBytes::Shared(src) = b { src.len() } else { 0 };
            let source = st.source.as_ref();
            let bytes = match &st.snapshot {
                Some(snapshot) => owned(snapshot),
                None => source.map_or(0, |s| owned(&s.src)),
            };
            (bytes + source.map_or(0, |s| s.lines.get().map_or(0, |lines| lines.capacity() * 4)) + e.path.capacity()) as u64
        }).sum()
    }

    /// A candidate version keeps stable IDs for existing paths while owning its
    /// replacement snapshots. Publishing or rejecting it cannot alter an old parse.
    pub fn overlay(&self, path: &str, src: Arc<[u8]>) -> Self {
        let id = self.source_id(path);
        let mut overrides = self.overrides.clone();
        let entry = Entry::new(path, Some(src.into()));
        overrides.insert(id, entry);
        Self { identities: self.identities.clone(), overrides }
    }
    /// Reserve a stable, 1-based ID, including for sources whose module is not loaded yet.
    /// Reading the snapshot is deferred until `get`, or supplied explicitly by `register`.
    pub fn source_id(&self, path: &str) -> u32 {
        let mut r = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = r.paths.get(path) { return *id; }
        let id = u32::try_from(r.entries.len()).ok().and_then(|n| r.first.checked_add(n)).expect("source ID overflow");
        r.entries.push(Entry::new(path, None));
        r.paths.insert(path.into(), id);
        id
    }

    pub fn find(&self, path: &str) -> Option<u32> {
        self.identities.lock().unwrap_or_else(|e| e.into_inner()).paths.get(path).copied()
    }

    fn entry(&self, id: u32) -> Result<Arc<Entry>, String> {
        if let Some(entry) = self.overrides.get(&id) { return Ok(entry.clone()); }
        let r = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        r.imported.get(&id).or_else(|| id.checked_sub(r.first).and_then(|i| r.entries.get(i as usize)))
            .cloned().ok_or_else(|| format!("unregistered source ID {id}"))
    }

    /// A certified image's snapshot under its recorded ID: true if `path` already has `id`
    /// (its bytes are not compared: the image is certified), or if `id` is free in the
    /// image-local range and is taken now.
    pub fn adopt_mapped(&self, id: u32, path: &str, src: &'static [u8]) -> bool {
        let mut r = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&have) = r.paths.get(path) { return have == id; }
        if id == 0 || id >= r.first || r.imported.contains_key(&id) { return false; }
        r.paths.insert(path.into(), id);
        r.imported.insert(id, Entry::new(path, Some(SourceBytes::Mapped(src))));
        true
    }

    /// Restore image-local identities without visiting or rewriting every token origin.
    /// Only the supplied sparse IDs exist; new sources receive IDs above that range.
    pub fn from_mapped(snapshots: impl IntoIterator<Item = (u32, String, &'static [u8])>) -> Option<Self> {
        let mut r = Registry::default();
        for (id, path, src) in snapshots {
            if id == 0 || r.imported.contains_key(&id) || r.paths.contains_key(&path) { return None; }
            r.first = r.first.max(id.checked_add(1)?);
            r.paths.insert(path.clone(), id);
            r.imported.insert(id, Entry::new(&path, Some(SourceBytes::Mapped(src))));
        }
        Some(Self { identities: Arc::new(Mutex::new(r)), overrides: Default::default() })
    }

    /// Publish an in-memory/VFS snapshot before parsing. A load never silently changes it.
    pub fn register(&self, path: &str, src: Arc<[u8]>) -> Result<u32, String> {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Source);
        let id = self.source_id(path);
        let entry = self.entry(id)?;
        let mut st = entry.state();
        let saved = st.snapshot.get_or_insert_with(|| SourceBytes::Shared(src.clone()));
        if **saved != *src { return Err(format!("source snapshot changed during load: {path}")); }
        Ok(id)
    }

    /// Restore an immutable snapshot from a pinned image. Diagnostics and macro consumers
    /// materialize its shared bytes and line index only when they actually request it.
    pub fn register_mapped(&self, path: &str, src: &'static [u8]) -> Result<u32, String> {
        let id = self.source_id(path);
        let entry = self.entry(id)?;
        let mut st = entry.state();
        let saved = st.snapshot.get_or_insert(SourceBytes::Mapped(src));
        if **saved != *src {
            return Err(format!("source snapshot changed during load: {path}"));
        }
        Ok(id)
    }

    pub fn get(&self, id: u32) -> Result<Arc<SourceFile>, String> {
        let _mem = crate::memstat::tag(crate::memstat::Tag::Source);
        let entry = self.entry(id)?;
        let mut st = entry.state();
        if let Some(source) = &st.source { return Ok(source.clone()); }
        let src = match &st.snapshot {
            Some(src) => src.clone(),
            None => {
                let read = crate::vfs::read_shared(&entry.path).map_err(|e| format!("{}: {e}", entry.path))?;
                st.snapshot.insert(SourceBytes::Shared(read)).clone()
            }
        };
        let source = Arc::new(SourceFile::new(entry.path.clone(), src));
        st.source = Some(source.clone());
        Ok(source)
    }

    /// The one switch from owned to mapped authored bytes: `path`'s file image is now mapped
    /// and its authored source is `src`. The registry entry (and its cached SourceFile) drops
    /// the owned bytes after they are shown identical; otherwise it stays owned and is counted.
    /// Holders of an earlier `Arc<SourceFile>` keep that file's bytes only while they hold it.
    /// Returns true when the entry's authored bytes are now (or already were) the mapped `src`:
    /// only then may the caller release the file's read snapshot.
    pub fn map_authored(&self, path: &str, src: &'static [u8]) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(id) = self.find(path) else { MAPPED_SOURCE_NOT_FOUND.fetch_add(1, Relaxed); return false };
        let Ok(entry) = self.entry(id) else { MAPPED_SOURCE_NOT_FOUND.fetch_add(1, Relaxed); return false };
        let mut st = entry.state();
        match &st.snapshot {
            Some(SourceBytes::Shared(owned)) if **owned == *src => {
                st.snapshot = Some(SourceBytes::Mapped(src));
                st.source = None;
                MAPPED_SOURCES.fetch_add(1, Relaxed);
                true
            }
            Some(SourceBytes::Shared(_)) => {
                MAPPED_SOURCE_MISMATCH.fetch_add(1, Relaxed);
                if std::env::var_os("MAKEPAD_RUN_LAZY_VERIFY").is_some() { panic!("mapped source differs from the registry's bytes: {path}"); }
                false
            }
            Some(_) => true,
            None => { MAPPED_SOURCE_NO_SNAPSHOT.fetch_add(1, Relaxed); false }
        }
    }
}

pub enum SourceRef<'a> {
    Current { path: &'a str, src: &'a [u8], lines: &'a OnceLock<Vec<u32>> },
    Registered(Arc<SourceFile>),
}

impl SourceRef<'_> {
    pub fn path(&self) -> &str { match self { Self::Current { path, .. } => path, Self::Registered(s) => &s.path } }
    pub fn bytes(&self) -> &[u8] { match self { Self::Current { src, .. } => src, Self::Registered(s) => &s.src } }
    pub fn line_col(&self, pos: u32) -> (usize, usize) {
        match self {
            Self::Current { src, lines, .. } => line_col(src, lines.get_or_init(|| line_starts(src)), pos),
            Self::Registered(s) => s.line_col(pos),
        }
    }
}

pub struct SourceLocation<'a> {
    pub source: SourceRef<'a>,
    pub span: SourceSpan,
}

impl SourceLocation<'_> {
    pub fn diagnostic(&self, phase: &'static str, msg: String) -> crate::diag::Diagnostic {
        let (l, c) = self.source.line_col(self.span.lo);
        let (el, ec) = self.source.line_col(self.span.hi);
        let mut d = crate::diag::Diagnostic::new(phase, self.source.path(), l as u32, c as u32, msg);
        d.end_line = el as u32;
        d.end_col = ec as u32;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_stable_across_workers_and_snapshots_are_immutable() {
        let sources = SourceRegistry::default();
        let id = sources.source_id("/fake/source.rs");
        assert_eq!(id, 1);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8).map(|_| scope.spawn(|| { let _z = crate::zone_alloc::compiler_thread();
                sources.register("/fake/source.rs", Arc::from("é value\n".as_bytes())).unwrap()
            })).collect();
            for job in jobs { assert_eq!(job.join().unwrap(), id); }
        });
        assert!(sources.register("/fake/source.rs", Arc::from(&b"different"[..])).is_err());
        let source = sources.get(id).unwrap();
        assert_eq!(source.line_col(3), (1, 4));
        assert_eq!(source.token_span(SourceSpan { file: id, lo: 3, hi: 8 }).start.column, 3);
        let edit = sources.overlay("/fake/source.rs", Arc::from(&b"new text"[..]));
        assert_eq!(edit.source_id("/fake/source.rs"), id);
        let imported = edit.source_id("/fake/new_import.rs");
        assert_eq!(sources.source_id("/fake/new_import.rs"), imported);
        assert_eq!(&*edit.get(id).unwrap().src, b"new text");
        assert_eq!(&*sources.get(id).unwrap().src, "é value\n".as_bytes());
        assert_eq!(source.hash(), crate::crate_image::file_hash(&source.path, &source.src));
        assert_ne!(edit.get(id).unwrap().hash(), source.hash());
        assert_eq!(std::mem::size_of::<TokenOrigin>(), 20);
    }
}
