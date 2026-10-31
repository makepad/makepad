//! A git repository as ONE file (local/plans/run-from-git.md): what `makepad run <git url>` reads its
//! sources from, without a working tree, a .git folder, packs or loose objects on disk.
//!
//! ```text
//! "RELAXGIT" u32 version                                     header
//! 'O' kind u8, oid [20], size varint, len varint, deflate    an object (commit 1, tree 2, blob 3, tag 4), undeltified
//! 'I' u32 len, body                                          the index of the whole file as of this append
//! 'T' "RGITEND1" u64 index offset, u64 index len, [20] sha1  trailer
//! ```
//! Appends add objects the file does not have yet, then a new index and trailer; opening reads the
//! last trailer (a tail without one, an interrupted append, is cut back on the next write). The
//! index holds the sorted object table, the kept refs and one flat path table per kept commit
//! (sorted paths -> mode, blob), so a file read is a binary search and one inflate, and a folder
//! listing is a prefix range. Compaction writes `<file>.new` with the objects the kept commits
//! reach and renames it over the store.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"RELAXGIT";
const VERSION: u32 = 1;
const TRAILER: &[u8; 8] = b"RGITEND1";
const TRAILER_LEN: usize = 1 + 8 + 8 + 8 + 20;

pub type Oid = [u8; 20];

pub const COMMIT: u8 = 1;
pub const TREE: u8 = 2;
pub const BLOB: u8 = 3;
pub const TAG: u8 = 4;

pub fn hex(o: &Oid) -> String {
    o.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Oid> {
    if s.len() != 40 {
        return None;
    }
    let mut o = [0u8; 20];
    for i in 0..20 {
        o[i] = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(o)
}

/// A kept ref: what it was fetched as (`<url>#<ref>`), its commit and when.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub key: String,
    pub commit: Oid,
    pub time: u64,
}

/// One file of a commit's tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// git's mode: 0o100644 file, 0o100755 executable, 0o120000 symlink, 0o160000 submodule
    pub mode: u32,
    pub oid: Oid,
}

#[derive(Default, Clone, Debug)]
pub struct Index {
    /// sorted by oid
    pub objects: Vec<(Oid, u64)>,
    pub refs: Vec<Ref>,
    /// commit -> its files, sorted by path
    pub tables: Vec<(Oid, Vec<Entry>)>,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get_varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        v |= ((x & 0x7f) as u64) << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn get_bytes<'a>(b: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
    let s = b.get(*at..*at + n)?;
    *at += n;
    Some(s)
}

fn get_oid(b: &[u8], at: &mut usize) -> Option<Oid> {
    get_bytes(b, at, 20)?.try_into().ok()
}

fn get_str(b: &[u8], at: &mut usize) -> Option<String> {
    let n = get_varint(b, at)? as usize;
    String::from_utf8(get_bytes(b, at, n)?.to_vec()).ok()
}

impl Index {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, self.objects.len() as u64);
        for (oid, off) in &self.objects {
            out.extend_from_slice(oid);
            put_varint(&mut out, *off);
        }
        put_varint(&mut out, self.refs.len() as u64);
        for r in &self.refs {
            put_varint(&mut out, r.key.len() as u64);
            out.extend_from_slice(r.key.as_bytes());
            out.extend_from_slice(&r.commit);
            put_varint(&mut out, r.time);
        }
        put_varint(&mut out, self.tables.len() as u64);
        for (commit, entries) in &self.tables {
            out.extend_from_slice(commit);
            put_varint(&mut out, entries.len() as u64);
            for e in entries {
                put_varint(&mut out, e.path.len() as u64);
                out.extend_from_slice(e.path.as_bytes());
                put_varint(&mut out, e.mode as u64);
                out.extend_from_slice(&e.oid);
            }
        }
        out
    }

    fn decode(b: &[u8]) -> Option<Index> {
        let mut at = 0;
        let n = get_varint(b, &mut at)? as usize;
        let mut objects = Vec::with_capacity(n.min(1 << 24));
        for _ in 0..n {
            let oid = get_oid(b, &mut at)?;
            objects.push((oid, get_varint(b, &mut at)?));
        }
        let n = get_varint(b, &mut at)? as usize;
        let mut refs = Vec::new();
        for _ in 0..n {
            refs.push(Ref { key: get_str(b, &mut at)?, commit: get_oid(b, &mut at)?, time: get_varint(b, &mut at)? });
        }
        let n = get_varint(b, &mut at)? as usize;
        let mut tables = Vec::new();
        for _ in 0..n {
            let commit = get_oid(b, &mut at)?;
            let m = get_varint(b, &mut at)? as usize;
            let mut entries = Vec::with_capacity(m.min(1 << 22));
            for _ in 0..m {
                entries.push(Entry { path: get_str(b, &mut at)?, mode: get_varint(b, &mut at)? as u32, oid: get_oid(b, &mut at)? });
            }
            tables.push((commit, entries));
        }
        (at == b.len()).then_some(Index { objects, refs, tables })
    }

    pub fn offset(&self, oid: &Oid) -> Option<u64> {
        self.objects.binary_search_by(|(o, _)| o.cmp(oid)).ok().map(|i| self.objects[i].1)
    }

    pub fn table(&self, commit: &Oid) -> Option<&[Entry]> {
        self.tables.iter().find(|(c, _)| c == commit).map(|(_, t)| t.as_slice())
    }
}

/// The store file, read side.
pub struct Store {
    pub path: PathBuf,
    file: File,
    pub index: Index,
    /// (offset, len) when the store sits inside a bigger file (`open_range`): read-only, every
    /// offset in the store is relative to `offset`
    pub range: Option<(u64, u64)>,
}

/// SHA-1 (git's object ids; the trailer's index check).
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = crate::git::sha1::Sha1::new();
    h.update(data);
    h.finalize()
}

impl Store {
    /// The store at `path`, or None when there is none (or it is not a store).
    pub fn open(path: &Path) -> std::io::Result<Option<Store>> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // shared with other readers, never while an append writes: an append first cuts the old
        // index off the end (`append`), so a reader in that moment would see an older index or
        // none (a commit another process has just mounted "is not kept")
        file.lock_shared()?;
        let r = (|| -> std::io::Result<Option<Index>> {
            let len = file.metadata()?.len();
            let mut head = [0u8; 12];
            if len < 12 || read_at(&file, &mut head, 0)? != 12 || &head[..8] != MAGIC {
                return Ok(None);
            }
            // the last valid trailer: at the end, else (an interrupted append) searched backwards
            Ok(Some(Self::last_trailer(&file, 0, len)?.map(|(_, index)| index).unwrap_or_default()))
        })();
        let _ = file.unlock();
        match r? {
            None => Ok(None),
            Some(index) => Ok(Some(Store { path: path.to_path_buf(), file, index, range: None })),
        }
    }

    /// The store that sits at bytes `offset..offset + len` of the file at `path` (an APK asset
    /// that is stored uncompressed, a store appended to another file; a plain store file is
    /// `(0, size)`). Read-only: it ends exactly at its trailer, there is no append or compaction
    /// of a range, and nothing is locked. None when the range is not a store.
    pub fn open_range(path: &Path, offset: u64, len: u64) -> std::io::Result<Option<Store>> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        if offset.checked_add(len).map_or(true, |end| end > size) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{}: range {offset}+{len} is outside the file", path.display())));
        }
        let mut head = [0u8; 12];
        if len < 12 || read_at(&file, &mut head, offset)? != 12 || &head[..8] != MAGIC {
            return Ok(None);
        }
        // a range ends at its trailer: no backwards search over bytes that belong to the next thing
        let Some(index) = Self::trailer_at(&file, offset, len.saturating_sub(TRAILER_LEN as u64))? else { return Ok(None) };
        Ok(Some(Store { path: path.to_path_buf(), file, index, range: Some((offset, len)) }))
    }

    fn trailer_at(file: &File, base: u64, at: u64) -> std::io::Result<Option<Index>> {
        let mut t = [0u8; TRAILER_LEN];
        if read_at(file, &mut t, base + at)? != TRAILER_LEN || t[0] != b'T' || &t[1..9] != TRAILER {
            return Ok(None);
        }
        let off = u64::from_le_bytes(t[9..17].try_into().unwrap());
        let ilen = u64::from_le_bytes(t[17..25].try_into().unwrap());
        if off + ilen > at || ilen > (1 << 32) {
            return Ok(None);
        }
        let mut body = vec![0u8; ilen as usize];
        if read_at(file, &mut body, base + off)? != body.len() {
            return Ok(None);
        }
        if sha1(&body) != t[25..45] {
            return Ok(None);
        }
        Ok(Index::decode(&body))
    }

    /// The last valid trailer of the `len` bytes at `base`: (end of the store, index).
    fn last_trailer(file: &File, base: u64, len: u64) -> std::io::Result<Option<(u64, Index)>> {
        if len >= TRAILER_LEN as u64 + 12 {
            if let Some(i) = Self::trailer_at(file, base, len - TRAILER_LEN as u64)? {
                return Ok(Some((len, i)));
            }
        }
        // backwards in 1 MB windows for the magic
        let mut hi = len;
        while hi > 12 {
            let lo = hi.saturating_sub(1 << 20).max(12);
            let mut buf = vec![0u8; (hi - lo) as usize + TRAILER_LEN];
            let n = read_at(file, &mut buf, base + lo)?;
            buf.truncate(n);
            let mut i = buf.len().saturating_sub(9);
            loop {
                if buf[i] == b'T' && buf.get(i + 1..i + 9) == Some(TRAILER) {
                    let at = lo + i as u64;
                    if let Some(idx) = Self::trailer_at(file, base, at)? {
                        return Ok(Some((at + TRAILER_LEN as u64, idx)));
                    }
                }
                if i == 0 {
                    break;
                }
                i -= 1;
            }
            hi = lo;
        }
        Ok(None)
    }

    /// An object's kind and bytes.
    pub fn object(&self, oid: &Oid) -> std::io::Result<Option<(u8, Vec<u8>)>> {
        let Some(off) = self.index.offset(oid) else { return Ok(None) };
        self.object_at(off).map(Some)
    }

    fn object_at(&self, off: u64) -> std::io::Result<(u8, Vec<u8>)> {
        let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: damaged object at {off}", self.path.display()));
        let base = self.range.map_or(0, |r| r.0);
        let mut head = [0u8; 1 + 1 + 20 + 20];
        let n = read_at(&self.file, &mut head, base + off)?;
        if n < 24 || head[0] != b'O' {
            return Err(bad());
        }
        let kind = head[1];
        let mut at = 22;
        let size = get_varint(&head[..n], &mut at).ok_or_else(bad)? as usize;
        let clen = get_varint(&head[..n], &mut at).ok_or_else(bad)? as usize;
        let mut comp = vec![0u8; clen];
        if read_at(&self.file, &mut comp, base + off + at as u64)? != clen {
            return Err(bad());
        }
        let data = crate::inflate::inflate::decompress_to_vec(&comp).map_err(|_| bad())?;
        if data.len() != size {
            return Err(bad());
        }
        Ok((kind, data))
    }

    /// An object's size without reading its bytes (the record header).
    pub fn object_size(&self, oid: &Oid) -> std::io::Result<Option<u64>> {
        let Some(off) = self.index.offset(oid) else { return Ok(None) };
        let mut head = [0u8; 1 + 1 + 20 + 10];
        let n = read_at(&self.file, &mut head, self.range.map_or(0, |r| r.0) + off)?;
        let mut at = 22;
        Ok(if n > 22 && head[0] == b'O' { get_varint(&head[..n], &mut at) } else { None })
    }

    /// An object's compressed size in the file (its record's payload, from the header).
    pub fn stored_size(&self, oid: &Oid) -> std::io::Result<Option<u64>> {
        let Some(off) = self.index.offset(oid) else { return Ok(None) };
        let mut head = [0u8; 1 + 1 + 20 + 20];
        let n = read_at(&self.file, &mut head, self.range.map_or(0, |r| r.0) + off)?;
        let mut at = 22;
        if n < 24 || head[0] != b'O' || get_varint(&head[..n], &mut at).is_none() {
            return Ok(None);
        }
        Ok(get_varint(&head[..n], &mut at))
    }

    pub fn contains(&self, oid: &Oid) -> bool {
        self.index.offset(oid).is_some()
    }

    /// Bytes of objects the kept commits do not reach.
    pub fn garbage(&self) -> u64 {
        // an upper bound from the index: objects not in any kept path table and not a kept commit or tree
        let live: HashSet<&Oid> = self.index.tables.iter().flat_map(|(c, t)| std::iter::once(c).chain(t.iter().map(|e| &e.oid))).collect();
        let mut offs: Vec<(u64, bool)> = self.index.objects.iter().map(|(o, off)| (*off, live.contains(o))).collect();
        offs.sort();
        let mut dead = 0;
        for w in offs.windows(2) {
            if !w[0].1 {
                dead += w[1].0 - w[0].0;
            }
        }
        dead
    }
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    let mut n = 0;
    while n < buf.len() {
        let k = f.read_at(&mut buf[n..], off + n as u64)?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    let mut n = 0;
    while n < buf.len() {
        let k = f.seek_read(&mut buf[n..], off + n as u64)?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

/// A commit's tree as its flat file list (sorted by path), walking trees through `get`.
pub fn path_table(commit: &Oid, get: &mut dyn FnMut(&Oid) -> Option<(u8, Vec<u8>)>) -> Result<Vec<Entry>, String> {
    let (kind, data) = get(commit).ok_or_else(|| format!("commit {} is missing", hex(commit)))?;
    if kind != COMMIT {
        return Err(format!("{} is not a commit", hex(commit)));
    }
    let text = String::from_utf8_lossy(&data);
    let tree = text.lines().next().and_then(|l| l.strip_prefix("tree ")).and_then(from_hex).ok_or("commit without a tree")?;
    let mut out = Vec::new();
    walk(&tree, "", get, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(tree: &Oid, prefix: &str, get: &mut dyn FnMut(&Oid) -> Option<(u8, Vec<u8>)>, out: &mut Vec<Entry>) -> Result<(), String> {
    let (kind, data) = get(tree).ok_or_else(|| format!("tree {} is missing", hex(tree)))?;
    if kind != TREE {
        return Err(format!("{} is not a tree", hex(tree)));
    }
    let mut at = 0;
    while at < data.len() {
        let sp = data[at..].iter().position(|b| *b == b' ').ok_or("bad tree")? + at;
        let mode = u32::from_str_radix(std::str::from_utf8(&data[at..sp]).map_err(|_| "bad tree")?, 8).map_err(|_| "bad tree mode")?;
        let nul = data[sp..].iter().position(|b| *b == 0).ok_or("bad tree")? + sp;
        let name = std::str::from_utf8(&data[sp + 1..nul]).map_err(|_| "a path that is not UTF-8")?;
        let oid: Oid = data.get(nul + 1..nul + 21).ok_or("bad tree")?.try_into().unwrap();
        at = nul + 21;
        let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        if mode == 0o40000 {
            walk(&oid, &path, get, out)?;
        } else {
            out.push(Entry { path, mode, oid });
        }
    }
    Ok(())
}

/// Every tree and the commit a kept commit needs, besides its files (compaction keeps them).
fn reachable_trees(commit: &Oid, get: &mut dyn FnMut(&Oid) -> Option<(u8, Vec<u8>)>, out: &mut HashSet<Oid>) -> Result<(), String> {
    out.insert(*commit);
    let (_, data) = get(commit).ok_or("commit missing")?;
    let text = String::from_utf8_lossy(&data);
    let tree = text.lines().next().and_then(|l| l.strip_prefix("tree ")).and_then(from_hex).ok_or("commit without a tree")?;
    let mut stack = vec![tree];
    while let Some(t) = stack.pop() {
        if !out.insert(t) {
            continue;
        }
        let (_, d) = get(&t).ok_or("tree missing")?;
        let mut at = 0;
        while at < d.len() {
            let sp = d[at..].iter().position(|b| *b == b' ').ok_or("bad tree")? + at;
            let nul = d[sp..].iter().position(|b| *b == 0).ok_or("bad tree")? + sp;
            let mode = u32::from_str_radix(std::str::from_utf8(&d[at..sp]).unwrap_or("0"), 8).unwrap_or(0);
            let oid: Oid = d.get(nul + 1..nul + 21).ok_or("bad tree")?.try_into().unwrap();
            if mode == 0o40000 {
                stack.push(oid);
            }
            at = nul + 21;
        }
    }
    Ok(())
}

fn encode_object(kind: u8, oid: &Oid, data: &[u8]) -> Vec<u8> {
    let comp = crate::inflate::deflate_compress(data, 6);
    let mut out = Vec::with_capacity(comp.len() + 40);
    out.push(b'O');
    out.push(kind);
    out.extend_from_slice(oid);
    put_varint(&mut out, data.len() as u64);
    put_varint(&mut out, comp.len() as u64);
    out.extend_from_slice(&comp);
    out
}

fn write_trailer(out: &mut Vec<u8>, index_off: u64, body: &[u8]) {
    out.push(b'T');
    out.extend_from_slice(TRAILER);
    out.extend_from_slice(&index_off.to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha1(body));
}

/// What one fetch adds: its objects, the ref it was fetched as and that commit's files.
pub struct Append<'a> {
    pub objects: &'a [(Oid, u8, Vec<u8>)],
    pub reference: Ref,
}

/// Appends a fetch to the store (creating it), keeping `keep` refs per key prefix policy: the
/// caller passes the refs to keep afterwards (their commits' path tables stay; others go from the
/// index; their bytes stay until compaction). Returns the store reopened.
pub fn append(path: &Path, add: Append, keep: &dyn Fn(&[Ref]) -> Vec<Ref>) -> Result<Store, String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(err)?;
    file.lock().map_err(err)?;
    let len = file.metadata().map_err(err)?.len();
    let (mut end, mut index) = if len == 0 {
        file.write_all(MAGIC).map_err(err)?;
        file.write_all(&VERSION.to_le_bytes()).map_err(err)?;
        (12u64, Index::default())
    } else {
        let mut head = [0u8; 12];
        file.seek(SeekFrom::Start(0)).map_err(err)?;
        file.read_exact(&mut head).map_err(err)?;
        if &head[..8] != MAGIC {
            return Err(format!("{}: not a makepad run git store", path.display()));
        }
        match Store::last_trailer(&file, 0, len).map_err(err)? {
            Some((e, i)) => (e, i),
            None => (12, Index::default()),
        }
    };
    // an interrupted append is cut off
    file.set_len(end).map_err(err)?;
    let mut have: HashSet<Oid> = index.objects.iter().map(|(o, _)| *o).collect();
    // the new objects, each once
    let todo: Vec<&(Oid, u8, Vec<u8>)> = add.objects.iter().filter(|(o, _, _)| have.insert(*o)).collect();
    // compressed on every core (the fetch's CPU time is this), written in order
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 32);
    // biggest first, taken one at a time (a few large blobs dominate)
    let mut order: Vec<usize> = (0..todo.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(todo[k].2.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::OnceLock<Vec<u8>>> = (0..todo.len()).map(|_| std::sync::OnceLock::new()).collect();
    std::thread::scope(|sc| {
        for _ in 0..threads.min(todo.len().max(1)) {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&i) = order.get(k) else { break };
                let (oid, kind, data) = todo[i];
                let _ = slots[i].set(encode_object(*kind, oid, data));
            });
        }
    });
    let encoded: Vec<Vec<u8>> = slots.into_iter().map(|s| s.into_inner().ok_or_else(|| format!("{}: compressing objects failed", path.display()))).collect::<Result<_, _>>()?;
    let mut new_objects = Vec::with_capacity(todo.len());
    let mut buf = Vec::new();
    for (o, rec) in todo.iter().zip(&encoded) {
        new_objects.push((o.0, end + buf.len() as u64));
        buf.extend_from_slice(rec);
        if buf.len() > 8 << 20 {
            file.seek(SeekFrom::Start(end)).map_err(err)?;
            file.write_all(&buf).map_err(err)?;
            end += buf.len() as u64;
            buf.clear();
        }
    }
    drop(encoded);
    file.seek(SeekFrom::Start(end)).map_err(err)?;
    file.write_all(&buf).map_err(err)?;
    end += buf.len() as u64;
    index.objects.extend(new_objects);
    index.objects.sort_by(|a, b| a.0.cmp(&b.0));
    // the commit's files, from the store as written now plus this fetch's objects
    let fetched: HashMap<Oid, (u8, &Vec<u8>)> = add.objects.iter().map(|(o, k, d)| (*o, (*k, d))).collect();
    let table = {
        let store = Store { path: path.to_path_buf(), file: file.try_clone().map_err(err)?, index: index.clone(), range: None };
        let mut get = |o: &Oid| fetched.get(o).map(|(k, d)| (*k, (*d).clone())).or_else(|| store.object(o).ok().flatten());
        path_table(&add.reference.commit, &mut get)?
    };
    // the same fetch again replaces its record; other fetches of the ref stay for `keep`
    index.refs.retain(|r| !(r.key == add.reference.key && r.commit == add.reference.commit));
    index.refs.push(add.reference.clone());
    index.refs = keep(&index.refs);
    if !index.tables.iter().any(|(c, _)| *c == add.reference.commit) {
        index.tables.push((add.reference.commit, table));
    }
    let kept: HashSet<Oid> = index.refs.iter().map(|r| r.commit).collect();
    index.tables.retain(|(c, _)| kept.contains(c));
    let body = index.encode();
    let mut tail = Vec::with_capacity(body.len() + 64);
    tail.push(b'I');
    tail.extend_from_slice(&(body.len() as u32).to_le_bytes());
    let index_off = end + tail.len() as u64;
    tail.extend_from_slice(&body);
    write_trailer(&mut tail, index_off, &body);
    // at `end`, where index_off was counted from: the table above read objects through a clone
    // of this handle, and on Windows a positional read (seek_read) moves the shared cursor --
    // the index then landed in the middle of the file and the store opened empty ("commit .. is
    // not kept" on DESKTOP-2); unix read_at leaves the cursor alone
    file.seek(SeekFrom::Start(end)).map_err(err)?;
    file.write_all(&tail).map_err(err)?;
    file.sync_all().map_err(err)?;
    let _ = file.unlock();
    drop(file);
    let store = Store::open(path).map_err(err)?.ok_or_else(|| format!("{}: unreadable after the append", path.display()))?;
    // compaction when the unreachable bytes exceed the reachable ones
    let size = std::fs::metadata(path).map_err(err)?.len();
    if store.garbage() * 2 > size {
        compact(&store)?;
        return Store::open(path).map_err(err)?.ok_or_else(|| format!("{}: unreadable after compaction", path.display()));
    }
    Ok(store)
}

/// Rewrites the store with only what its kept commits reach: `<store>.new`, then renamed over.
pub fn compact(store: &Store) -> Result<(), String> {
    if store.range.is_some() {
        return Err(format!("{}: a store inside another file is read-only", store.path.display()));
    }
    let path = &store.path;
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut live = HashSet::new();
    let mut get = |o: &Oid| store.object(o).ok().flatten();
    for (c, t) in &store.index.tables {
        reachable_trees(c, &mut get, &mut live)?;
        live.extend(t.iter().map(|e| e.oid));
    }
    let tmp = path.with_extension("store.new");
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let mut file = File::create(&tmp).map_err(err)?;
    let mut objects = Vec::new();
    let mut offset = out.len() as u64;
    file.write_all(&out).map_err(err)?;
    out.clear();
    // in file order (old offsets), so related objects stay together
    let mut order: Vec<&(Oid, u64)> = store.index.objects.iter().filter(|(o, _)| live.contains(o)).collect();
    order.sort_by_key(|(_, off)| *off);
    for (oid, off) in order {
        let (kind, data) = store.object_at(*off).map_err(err)?;
        let rec = encode_object(kind, oid, &data);
        objects.push((*oid, offset));
        offset += rec.len() as u64;
        file.write_all(&rec).map_err(err)?;
    }
    objects.sort_by(|a, b| a.0.cmp(&b.0));
    let index = Index { objects, refs: store.index.refs.clone(), tables: store.index.tables.clone() };
    let body = index.encode();
    let mut tail = Vec::new();
    tail.push(b'I');
    tail.extend_from_slice(&(body.len() as u32).to_le_bytes());
    let index_off = offset + tail.len() as u64;
    tail.extend_from_slice(&body);
    write_trailer(&mut tail, index_off, &body);
    file.write_all(&tail).map_err(err)?;
    file.sync_all().map_err(err)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(err)?;
    Ok(())
}

/// The store file of a repository URL in `cache`: `git-<name>-<hash8>.store`.
/// makepad run's own folder: $MAKEPAD_RUN_HOME, else ~/.makepad-run (%USERPROFILE%\.makepad-run);
/// its stores are in `cache/` there.
pub fn relax_home() -> PathBuf {
    if let Some(h) = std::env::var_os("MAKEPAD_RUN_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(|| ".".into());
    home.join(".makepad-run")
}

/// One file of `url`'s repository at `commit`, from its store (None: no store, the commit is
/// not kept, or no such file).
pub fn store_file(url: &str, commit: &Oid, path: &str) -> Option<Vec<u8>> {
    let s = Store::open(&store_path(&relax_home().join("cache"), url)).ok()??;
    let t = s.index.table(commit)?;
    let e = &t[t.binary_search_by(|e| e.path.as_str().cmp(path)).ok()?];
    s.object(&e.oid).ok()?.map(|(_, d)| d)
}

pub fn store_path(cache: &Path, url: &str) -> PathBuf {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or("repo");
    let name: String = last.trim_end_matches(".git").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let h = sha1(url.trim_end_matches('/').as_bytes());
    cache.join(format!("git-{}-{}.store", if name.is_empty() { "repo" } else { &name }, &hex(&h)[..8]))
}

/// BTreeMap of a path table's folders (for read_dir/is_dir): each folder -> its direct children.
pub fn folders(entries: &[Entry]) -> BTreeMap<String, Vec<(String, bool)>> {
    let mut out: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    out.entry(String::new()).or_default();
    for e in entries {
        let mut parent = String::new();
        let parts: Vec<&str> = e.path.split('/').collect();
        for (i, p) in parts.iter().enumerate() {
            let is_dir = i + 1 < parts.len();
            let me = if parent.is_empty() { p.to_string() } else { format!("{parent}/{p}") };
            let kids = out.entry(parent.clone()).or_default();
            if !kids.iter().any(|(n, _)| n == p) {
                kids.push((p.to_string(), is_dir));
            }
            if is_dir {
                out.entry(me.clone()).or_default();
            }
            parent = me;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn obj(kind: &str, data: &[u8]) -> Oid {
        let mut h = format!("{kind} {}\0", data.len()).into_bytes();
        h.extend_from_slice(data);
        sha1(&h)
    }

    fn tree(entries: &[(&str, &str, Oid)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (mode, name, oid) in entries {
            out.extend_from_slice(format!("{mode} {name}\0").as_bytes());
            out.extend_from_slice(oid);
        }
        out
    }

    #[test]
    fn vfs_mounts_serve_the_store() {
        let dir = std::env::temp_dir().join(format!("relax-vfs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mk = |name: &str, files: &[(&str, &[u8])]| -> (PathBuf, Oid) {
            // one level of folders is enough here
            let mut objects = Vec::new();
            let mut root_entries: Vec<(String, String, Oid)> = Vec::new();
            let mut sub: BTreeMap<String, Vec<(String, Oid)>> = BTreeMap::new();
            for (path, data) in files {
                let o = obj("blob", data);
                objects.push((o, BLOB, data.to_vec()));
                match path.split_once('/') {
                    Some((d, f)) => sub.entry(d.to_string()).or_default().push((f.to_string(), o)),
                    None => root_entries.push(("100644".into(), path.to_string(), o)),
                }
            }
            for (d, fs) in sub {
                let t = tree(&fs.iter().map(|(n, o)| ("100644", n.as_str(), *o)).collect::<Vec<_>>());
                let to = obj("tree", &t);
                objects.push((to, TREE, t));
                root_entries.push(("40000".into(), d, to));
            }
            root_entries.sort_by(|a, b| a.1.cmp(&b.1));
            let r = tree(&root_entries.iter().map(|(m, n, o)| (m.as_str(), n.as_str(), *o)).collect::<Vec<_>>());
            let ro = obj("tree", &r);
            objects.push((ro, TREE, r));
            let c = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\n{name}\n", hex(&ro)).into_bytes();
            let co = obj("commit", &c);
            objects.push((co, COMMIT, c));
            let path = dir.join(format!("git-{name}-00000000.store"));
            append(&path, Append { objects: &objects, reference: Ref { key: name.into(), commit: co, time: 1 } }, &|r| r.to_vec()).unwrap();
            (path, co)
        };
        let (outer, oc) = mk("outer", &[("Cargo.toml", b"[workspace]\n"), ("libs/a.rs", b"a"), ("apps/README", b"r")]);
        let (inner, ic) = mk("inner", &[("Cargo.toml", b"[package]\n"), ("src/lib.rs", b"pub fn f() {}")]);
        let root = dir.join("vfs").join("x").to_string_lossy().into_owned();
        crate::vfs::mount(&root, Arc::new(Store::open(&outer).unwrap().unwrap()), &oc).unwrap();
        crate::vfs::mount(&format!("{root}/apps/commercial"), Arc::new(Store::open(&inner).unwrap().unwrap()), &ic).unwrap();
        assert_eq!(crate::vfs::read(format!("{root}/libs/a.rs")).unwrap(), b"a");
        assert_eq!(crate::vfs::read(format!("{root}/libs/../Cargo.toml")).unwrap(), b"[workspace]\n");
        assert_eq!(crate::vfs::read(format!("{root}/apps/commercial/src/lib.rs")).unwrap(), b"pub fn f() {}");
        let virtual_file = format!("{root}/apps/commercial/src/../src/lib.rs");
        assert_eq!(crate::vfs::canonicalize(&virtual_file).unwrap(), PathBuf::from(format!("{root}/apps/commercial/src/lib.rs")));
        let metadata = crate::vfs::metadata(&virtual_file).unwrap();
        assert!(metadata.is_file && !metadata.is_dir && !metadata.is_symlink);
        assert_eq!(metadata.len, 13);
        assert_eq!(metadata.modified, crate::vfs::metadata(&virtual_file).unwrap().modified);
        assert!(crate::vfs::canonicalize(format!("{root}/apps/commercial/../README")).is_err());
        assert!(crate::vfs::is_dir(format!("{root}/apps/commercial/src")) && crate::vfs::is_file(format!("{root}/apps/commercial/Cargo.toml")));
        assert!(!crate::vfs::exists(format!("{root}/apps/commercial/README")), "the inner repository owns its folder");
        assert_eq!(crate::vfs::read_dir(format!("{root}/libs")).unwrap(), vec![("a.rs".to_string(), false)]);
        assert!(crate::vfs::read(format!("{root}/nope")).is_err());
        assert!(!std::path::Path::new(&root).exists(), "a mount is never created on disk");
        // the disk is untouched by mounts elsewhere
        assert!(crate::vfs::is_file(&outer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_store_inside_a_bigger_file_mounts_and_reads() {
        let dir = std::env::temp_dir().join(format!("relax-range-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("git-x-00000000.store");
        let a = b"pub fn a() {}\n".to_vec();
        let b = b"[package]\nname = \"x\"\n".to_vec();
        let (ao, bo) = (obj("blob", &a), obj("blob", &b));
        let src = tree(&[("100644", "lib.rs", ao)]);
        let srco = obj("tree", &src);
        let root = tree(&[("100644", "Cargo.toml", bo), ("40000", "src", srco)]);
        let rooto = obj("tree", &root);
        let c = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\none\n", hex(&rooto)).into_bytes();
        let co = obj("commit", &c);
        let objects = vec![(co, COMMIT, c), (rooto, TREE, root), (srco, TREE, src), (ao, BLOB, a.clone()), (bo, BLOB, b.clone())];
        append(&path, Append { objects: &objects, reference: Ref { key: "k".into(), commit: co, time: 1 } }, &|r| r.to_vec()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // embedded at an odd offset, with other bytes (even a store's magic and trailer words) around it
        let mut big = b"RELAXGITnoise-before-".to_vec();
        big.extend_from_slice(&[0xff; 14]);
        let offset = big.len() as u64;
        assert_eq!(offset % 4096 != 0 && offset % 2 == 1, true);
        big.extend_from_slice(&bytes);
        big.extend_from_slice(b"RGITEND1-after");
        let bigger = dir.join("bigger.bin");
        std::fs::write(&bigger, &big).unwrap();
        let s = Store::open_range(&bigger, offset, bytes.len() as u64).unwrap().unwrap();
        assert_eq!(s.range, Some((offset, bytes.len() as u64)));
        assert_eq!(s.object(&ao).unwrap().unwrap(), (BLOB, a.clone()));
        assert_eq!(s.stored_size(&ao).unwrap().is_some(), true);
        assert_eq!(s.index.table(&co).unwrap().len(), 2);
        // a range that is not a store, or outside the file, is no store / an error
        assert!(Store::open_range(&bigger, 3, bytes.len() as u64).unwrap().is_none());
        assert!(Store::open_range(&bigger, offset, bytes.len() as u64 + 100).is_err());
        assert!(compact(&s).is_err(), "a range store is read-only");
        // mounted through the VFS, and through the text a build-script child gets
        let root_dir = dir.join("vfs").join("r").to_string_lossy().into_owned();
        crate::vfs::mount(&root_dir, Arc::new(s), &co).unwrap();
        assert_eq!(crate::vfs::read(format!("{root_dir}/src/lib.rs")).unwrap(), a);
        let text = crate::vfs::mounts_text();
        assert!(text.contains(&format!("\t{offset}+{}\n", bytes.len())), "{text}");
        let root2 = dir.join("vfs").join("r2").to_string_lossy().into_owned();
        let line = text.lines().find(|l| l.starts_with(&root_dir)).unwrap().replacen(&root_dir, &root2, 1);
        crate::vfs::mount_text(&line).unwrap();
        assert_eq!(crate::vfs::read(format!("{root2}/Cargo.toml")).unwrap(), b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha1_known() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn store_round_trip_append_and_compact() {
        let dir = std::env::temp_dir().join(format!("relax-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("git-x-00000000.store");
        let a = b"fn main() {}\n".to_vec();
        let b = b"[package]\nname = \"x\"\n".to_vec();
        let (ao, bo) = (obj("blob", &a), obj("blob", &b));
        let src = tree(&[("100644", "main.rs", ao)]);
        let srco = obj("tree", &src);
        let root = tree(&[("100644", "Cargo.toml", bo), ("40000", "src", srco)]);
        let rooto = obj("tree", &root);
        let c1 = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\none\n", hex(&rooto)).into_bytes();
        let c1o = obj("commit", &c1);
        let objects = vec![(c1o, COMMIT, c1.clone()), (rooto, TREE, root.clone()), (srco, TREE, src.clone()), (ao, BLOB, a.clone()), (bo, BLOB, b.clone())];
        let keep_two = |refs: &[Ref]| refs.iter().rev().take(2).cloned().collect::<Vec<_>>().into_iter().rev().collect();
        let s = append(&path, Append { objects: &objects, reference: Ref { key: "u#main".into(), commit: c1o, time: 1 } }, &keep_two).unwrap();
        let t = s.index.table(&c1o).unwrap();
        assert_eq!(t.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["Cargo.toml", "src/main.rs"]);
        assert_eq!(s.object(&ao).unwrap().unwrap(), (BLOB, a.clone()));
        let f = folders(t);
        assert_eq!(f[""], vec![("Cargo.toml".to_string(), false), ("src".to_string(), true)]);
        // a second commit sharing a blob: only new objects are added
        let a2 = b"fn main() { println!(\"2\"); }\n".to_vec();
        let a2o = obj("blob", &a2);
        let src2 = tree(&[("100644", "main.rs", a2o)]);
        let src2o = obj("tree", &src2);
        let root2 = tree(&[("100644", "Cargo.toml", bo), ("40000", "src", src2o)]);
        let root2o = obj("tree", &root2);
        let c2 = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\ntwo\n", hex(&root2o)).into_bytes();
        let c2o = obj("commit", &c2);
        let size1 = std::fs::metadata(&path).unwrap().len();
        let objects2 = vec![(c2o, COMMIT, c2), (root2o, TREE, root2), (src2o, TREE, src2), (a2o, BLOB, a2.clone()), (bo, BLOB, b.clone())];
        let s = append(&path, Append { objects: &objects2, reference: Ref { key: "u#main".into(), commit: c2o, time: 2 } }, &keep_two).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > size1);
        // the two newest fetches of the ref stay (the previous one is the fallback)
        assert_eq!(s.index.refs.len(), 2);
        assert!(s.index.table(&c1o).is_some());
        assert_eq!(s.object(&a2o).unwrap().unwrap().1, a2);
        // a third fetch drops the oldest
        let c3 = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\nthree\n", hex(&root2o)).into_bytes();
        let c3o = obj("commit", &c3);
        let s = append(&path, Append { objects: &[(c3o, COMMIT, c3)], reference: Ref { key: "u#main".into(), commit: c3o, time: 3 } }, &keep_two).unwrap();
        assert_eq!(s.index.refs.len(), 2);
        assert!(s.index.table(&c1o).is_none() && s.index.table(&c2o).is_some() && s.index.table(&c3o).is_some());
        // an interrupted append (garbage at the end) is ignored on open and cut on the next write
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"O\x03garbage-without-a-trailer").unwrap();
        drop(f);
        let s = Store::open(&path).unwrap().unwrap();
        assert_eq!(s.object(&a2o).unwrap().unwrap().1, a2);
        // compaction keeps only what the kept commit reaches
        compact(&s).unwrap();
        let s = Store::open(&path).unwrap().unwrap();
        assert!(!s.contains(&ao) && !s.contains(&c1o));
        assert_eq!(s.object(&a2o).unwrap().unwrap().1, a2);
        assert_eq!(s.index.table(&c2o).unwrap().len(), 2);
        assert_eq!(s.index.table(&c3o).unwrap().len(), 2);
        assert!(std::fs::read_dir(&dir).unwrap().count() == 1, "one file per repository");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
