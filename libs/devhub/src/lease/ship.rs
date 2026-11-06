//! Shipping a lane's tree to a lease's work dir over the devhub link (no ssh, no rsync: the same
//! on Linux, macOS and Windows nodes).
//!
//! The client sends a manifest (`path \t size \t mtime µs \t x` lines, in batches) to
//! `ship-have`; the node answers which entries it does not hold as they are (a regular file of
//! that size and mtime, and on Unix that executable bit). The client sends each miss in chunks to
//! `ship-put` (written to `<path>.devhub-part`, renamed into place with the source's mode after
//! its last byte, and the NODE's time as its mtime, as a `git checkout` write: a source mtime
//! older than the node's build artifacts made its cargo keep them, and build loaders from old
//! sources). The node remembers the source's size and mtime of what it wrote, and the stat of the
//! file it wrote, so `ship-have` still knows a file it holds as the source has it; and symlinks to `ship-link` (Unix nodes); then `ship-done` names
//! the complete shipped set, deletions and pinned base blobs. Tree leases undo these paths at
//! release. Overlay leases keep exactly that set under the lane's edits folder: stale files go,
//! each lease stages its transfer before publishing (equal-pin leases can ship concurrently);
//! deletions become `.makepad-run-deleted`, and `.makepad-run-edits` records base blobs (`new`
//! for additions). Live saves use the same complete set. Up to [`WINDOW`] requests
//! are in flight on one routed channel per service.

use makepad_remote::conn::Event;
use makepad_remote::link::Client;
use std::collections::VecDeque;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Paths that never leave their machine in a ship, whoever asks: agent configuration (`.claude`),
/// build output (`target`, `__pycache__`), at any depth, an atomic write's temp file, and `local/`
/// at the root of either repo.
/// Files git ignores are not in a dirty set to begin with, but the commercial repo does not share
/// the root's `.gitignore`, so this holds for it too.
pub fn never_ships(path: &str) -> bool {
    let rel = path.strip_prefix("apps/commercial/").unwrap_or(path);
    rel == "local" || rel.starts_with("local/") || never_ships_named(path)
}

/// What never ships even when a run names it (`DEVHUB_ALSO`): `local/` does then (a lane's
/// bench scripts), build output, agent state and atomic writes' temp files never.
pub fn never_ships_named(path: &str) -> bool {
    path.split('/').any(|c| matches!(c, ".claude" | "target" | "__pycache__")) || path.rsplit('/').next().is_some_and(atomic_temp)
}

/// The temp file of an atomic write, `<name>.tmp.<pid>.<hex>`, renamed onto `<name>` once written:
/// the final file ships, never it. A live session shipped `support.rs.tmp.81511.<hex>` after its
/// rename, and the edit failed on its box (W3766, 11-06).
fn atomic_temp(name: &str) -> bool {
    let Some((base, tail)) = name.rsplit_once(".tmp.") else { return false };
    let Some((pid, hex)) = tail.split_once('.') else { return false };
    !base.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()) && !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The repo's main checkout rather than a linked worktree (a lane's): the work uncommitted there is
/// a person's, and goes to a box only when asked for by name.
pub fn is_main_checkout(root: &Path) -> bool {
    root.join(".git").is_dir()
}

/// Raw bytes per `ship-put` (base64 makes a frame of ~350 KB: under the link's 1 MB frames).
pub const CHUNK: usize = 256 << 10;
/// Manifest entries per `ship-have`.
const HAVE_BATCH: usize = 1500;
/// Requests in flight per channel.
const WINDOW: usize = 64;

/// One file of the manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub mtime_us: u64,
    pub exec: bool,
    /// a symlink's target (shipped as a link, not its content)
    pub link: Option<String>,
}

/// Which source a run took on top of its commits: the pin, each shipped path's content hash (a
/// link's: its target) and the deleted paths. The same files at one pin say the same, whichever
/// lease ships them and however the list is ordered.
pub fn source_id(pin: &str, files: &[(String, String)], deleted: &[String]) -> String {
    let mut files: Vec<&(String, String)> = files.iter().collect();
    files.sort();
    let mut deleted: Vec<&String> = deleted.iter().collect();
    deleted.sort();
    let mut text = format!("{pin}\n");
    for (path, hash) in files { text.push_str(&format!("f\t{path}\t{hash}\n")); }
    for path in deleted { text.push_str(&format!("d\t{path}\n")); }
    makepad_network::digest::sha256_hash(text.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// [`source_id`] of the files as they are under `dir` (one it cannot read says so in the id).
pub fn source_in(dir: &Path, pin: &str, shipped: &[String], deleted: &[String]) -> String {
    let hash = |p: &String| {
        let file = dir.join(p);
        let bytes = match std::fs::read_link(&file) {
            Ok(target) => Ok(target.to_string_lossy().into_owned().into_bytes()),
            Err(_) => std::fs::read(&file),
        };
        bytes.map_or_else(|_| "unread".to_string(), |b| makepad_network::digest::sha256_hash(&b).iter().map(|x| format!("{x:02x}")).collect())
    };
    source_id(pin, &shipped.iter().map(|p| (p.clone(), hash(p))).collect::<Vec<_>>(), deleted)
}

fn b64(b: &[u8]) -> String {
    makepad_network::digest::base64_encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    super::exec::b64_decode(s)
}

/// What the node wrote per work dir: path -> (source size, source mtime µs, source x, the
/// written file's size and mtime µs). Memory only: a restarted hub ships each file once more.
type Wrote = std::collections::HashMap<PathBuf, std::collections::HashMap<String, (u64, u64, bool, u64, u64)>>;
static WROTE: std::sync::Mutex<Option<Wrote>> = std::sync::Mutex::new(None);

fn mtime_us(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_micros() as u64)
}

#[cfg(unix)]
fn is_exec(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_m: &std::fs::Metadata) -> bool {
    false
}

/// The manifest entry of `rel` under `root` (a dir expands to its files); symlinks are not followed.
pub fn entries(root: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<(), String> {
    let p = root.join(rel);
    let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if m.file_type().is_symlink() {
        let t = std::fs::read_link(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        out.push(Entry { path: rel.to_string(), size: 0, mtime_us: 0, exec: false, link: Some(t.to_string_lossy().into_owned()) });
    } else if m.is_dir() {
        let mut kids: Vec<String> = std::fs::read_dir(&p).map_err(|e| format!("{}: {e}", p.display()))?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        kids.sort();
        for k in kids {
            entries(root, &format!("{}/{k}", rel.trim_end_matches('/')), out)?;
        }
    } else if m.is_file() {
        out.push(Entry { path: rel.to_string(), size: m.len(), mtime_us: mtime_us(&m), exec: is_exec(&m), link: None });
    }
    Ok(())
}

fn manifest_line(e: &Entry) -> String {
    match &e.link {
        Some(t) => format!("{}\tL\t{}", e.path, t),
        None => format!("{}\t{}\t{}\t{}", e.path, e.size, e.mtime_us, e.exec as u8),
    }
}

// ---- the node side (the lease executor calls these with the lease's work dir)

/// A relative path that stays inside its root.
pub(crate) fn inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty() && !p.is_absolute() && !rel.contains(['\\', '\t', '\n', '\r']) && rel.split('/').all(|c| !matches!(c, "" | "." | "..")) && p.components().all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Clear cached staging paths whose file/directory shape changed since the previous save.
pub fn stage(work: &Path, manifest_b64: &str) -> Result<(), String> {
    let raw = unb64(manifest_b64).ok_or("ship-have: manifest is not base64")?;
    for line in String::from_utf8_lossy(&raw).lines() {
        let rel = line.split('\t').next().filter(|p| inside(p)).ok_or("ship-have: bad path")?;
        let mut path = work.to_path_buf();
        let mut parts = Path::new(rel).components().peekable();
        while let Some(part) = parts.next() {
            path.push(part.as_os_str());
            match std::fs::symlink_metadata(&path) {
                Ok(m) if parts.peek().is_some() && !m.is_dir() => std::fs::remove_file(&path).map_err(|e| e.to_string())?,
                Ok(m) if parts.peek().is_none() && m.is_dir() => std::fs::remove_dir_all(&path).map_err(|e| e.to_string())?,
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// `ship-have <b64 manifest>` (after the lease id): the indexes of the entries this work dir
/// lacks or holds differently.
pub fn have(work: &Path, manifest_b64: &str) -> Result<String, String> {
    let raw = unb64(manifest_b64).ok_or("ship-have: manifest is not base64")?;
    let text = String::from_utf8_lossy(&raw);
    let mut need = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some(path) = f.first().filter(|p| inside(p)) else { return Err(format!("ship-have: bad path in entry {i}")) };
        let p = work.join(path);
        let held = match f.get(1) {
            Some(&"L") => std::fs::read_link(&p).is_ok_and(|t| Some(t.to_string_lossy().as_ref()) == f.get(2).copied()),
            _ => {
                let (size, mtime, x) = (f.get(1).and_then(|v| v.parse::<u64>().ok()), f.get(2).and_then(|v| v.parse::<u64>().ok()), f.get(3) == Some(&"1"));
                // held: this node wrote it from a source of that size, mtime and mode, and it is
                // still the file it wrote (a checkout, a restore or an edit since makes it a miss)
                let rec = WROTE.lock().unwrap().as_ref().and_then(|w| w.get(work)).and_then(|m| m.get(*path)).copied();
                rec.is_some_and(|(ss, sm, sx, ws, wm)| Some(ss) == size && Some(sm) == mtime && sx == x && std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file() && m.len() == ws && mtime_us(&m) == wm && (cfg!(not(unix)) || is_exec(&m) == x)))
            }
        };
        if !held {
            need.push(i.to_string());
        }
    }
    Ok(format!("{{\"need\":[{}]}}", need.join(",")))
}

/// `ship-put <off> <size> <mtime µs> <x> <path b64> <data b64>` (after the lease id): one chunk.
pub fn put(work: &Path, rest: &str) -> Result<String, String> {
    let mut w: Vec<&str> = rest.splitn(6, ' ').collect();
    // an empty file's data is empty (the line's trailing space is trimmed off)
    if w.len() == 5 {
        w.push("");
    }
    let [off, size, mtime, x, path, data] = w[..] else { return Err("usage: ship-put <id> <off> <size> <mtime_us> <x> <path b64> <data b64>".into()) };
    let (Ok(off), Ok(size), Ok(mtime)) = (off.parse::<u64>(), size.parse::<u64>(), mtime.parse::<u64>()) else { return Err("ship-put: bad numbers".into()) };
    let rel = unb64(path).map(|b| String::from_utf8_lossy(&b).into_owned()).filter(|r| inside(r)).ok_or("ship-put: bad path")?;
    let data = unb64(data.trim()).ok_or("ship-put: data is not base64")?;
    if off + data.len() as u64 > size {
        return Err(format!("ship-put {rel}: past its size"));
    }
    let dst = work.join(&rel);
    let part = PathBuf::from(format!("{}.devhub-part", dst.display()));
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let open = || std::fs::OpenOptions::new().write(true).create(true).truncate(off == 0).open(&part);
    // a prune of the same lane dir (another ship's) may remove the dir that just emptied: make it again
    let mut f = match open() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            open().map_err(|e| format!("{}: {e}", part.display()))?
        }
        r => r.map_err(|e| format!("{}: {e}", part.display()))?,
    };
    f.seek(SeekFrom::Start(off)).and_then(|_| f.write_all(&data)).map_err(|e| format!("{}: {e}", part.display()))?;
    if off + data.len() as u64 == size {
        // the node's own time stays the mtime (never the source's)
        drop(f);
        set_exec(&part, x == "1");
        if std::fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("ship-put {rel}: a directory is in the way"));
        }
        std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        if let Ok(m) = std::fs::symlink_metadata(&dst) {
            WROTE.lock().unwrap().get_or_insert_with(Default::default).entry(work.to_path_buf()).or_default().insert(rel, (size, mtime, x == "1", m.len(), mtime_us(&m)));
        }
        return Ok("{\"done\":1}".into());
    }
    Ok(format!("{{\"off\":{}}}", off + data.len() as u64))
}

#[cfg(unix)]
fn set_exec(p: &Path, x: bool) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(if x { 0o755 } else { 0o644 }));
}

#[cfg(not(unix))]
fn set_exec(_p: &Path, _x: bool) {}

/// `ship-link <path b64> <target b64>` (after the lease id): a symlink (Unix nodes; Windows
/// skips it: creating one needs a privilege a node does not have).
pub fn link(work: &Path, rest: &str) -> Result<String, String> {
    let (path, target) = rest.split_once(' ').ok_or("usage: ship-link <id> <path b64> <target b64>")?;
    let rel = unb64(path).map(|b| String::from_utf8_lossy(&b).into_owned()).filter(|r| inside(r)).ok_or("ship-link: bad path")?;
    let target = unb64(target.trim()).map(|b| String::from_utf8_lossy(&b).into_owned()).ok_or("ship-link: bad target")?;
    make_link(&work.join(rel), &target)
}

#[cfg(unix)]
fn make_link(dst: &Path, target: &str) -> Result<String, String> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let _ = std::fs::remove_file(dst);
    std::os::unix::fs::symlink(target, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok("{\"done\":1}".into())
}

#[cfg(not(unix))]
fn make_link(_dst: &Path, _target: &str) -> Result<String, String> {
    Ok("{\"skipped\":\"symlink\"}".into())
}

// ---- the client side

/// Requests on one routed channel to a node service, up to [`WINDOW`] in flight; each answer is
/// its lines up to the `end` line.
struct Pipe {
    ch: makepad_remote::conn::Channel,
    pending: VecDeque<String>,
    buf: String,
    what: String,
    /// the node's own time on the answered requests (their `end` lines' ms)
    node_ms: u64,
}

impl Pipe {
    fn open(c: &Client, node: &str, service: &str) -> Result<Pipe, String> {
        let ch = c.open("route", &format!("{node} {service} ")).map_err(|e| format!("{node} {service}: {e}"))?;
        Ok(Pipe { ch, pending: VecDeque::new(), buf: String::new(), what: format!("{node} {service}"), node_ms: 0 })
    }

    /// Sends `line`; when the window is full, first takes the oldest answer (checked by `check`).
    fn send(&mut self, line: String, check: &mut dyn FnMut(&str, &[String]) -> Result<(), String>) -> Result<(), String> {
        if self.pending.len() >= WINDOW {
            self.take(check)?;
        }
        let tag = line.split(' ').nth(1).unwrap_or("").to_string();
        let mut l = line;
        l.push('\n');
        self.ch.send(l.into_bytes()).map_err(|e| format!("{}: {e}", self.what))?;
        self.pending.push_back(tag);
        Ok(())
    }

    fn take(&mut self, check: &mut dyn FnMut(&str, &[String]) -> Result<(), String>) -> Result<(), String> {
        let tag = self.pending.pop_front().unwrap_or_default();
        let mut lines = Vec::new();
        loop {
            while let Some(i) = self.buf.find('\n') {
                let l: String = self.buf.drain(..=i).collect();
                let l = l.trim();
                if l.is_empty() {
                    continue;
                }
                if l.starts_with("{\"end\":") {
                    self.node_ms += makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("ms").and_then(super::num)).map_or(0, |ms| ms as u64);
                    return check(&tag, &lines);
                }
                if let Some(e) = makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("err").and_then(|x| x.as_str()).map(str::to_string)) {
                    return Err(format!("{}: {e}", self.what));
                }
                lines.push(l.to_string());
            }
            match self.ch.recv() {
                Some(Event::Data(d)) => self.buf.push_str(&String::from_utf8_lossy(&d)),
                Some(Event::Opened(_)) => {}
                Some(Event::Closed(_, why)) => return Err(format!("{}: {why}", self.what)),
                None => return Err(format!("{}: the link ended", self.what)),
            }
        }
    }

    fn drain(&mut self, check: &mut dyn FnMut(&str, &[String]) -> Result<(), String>) -> Result<(), String> {
        while !self.pending.is_empty() {
            self.take(check)?;
        }
        Ok(())
    }
}

/// What a ship moved.
#[derive(Debug, Default, PartialEq)]
pub struct Shipped {
    pub files: usize,
    pub sent: usize,
    pub bytes: u64,
    /// the node's time on the puts and links (the rest of the send is the way there and back)
    pub node_ms: u64,
}

/// Ships `entries` from `root` into lease `id`'s work dir on `node`. `placed`: the caller knows each
/// entry's path holds a file on the node and its content changed since (a live session's edit of
/// files it shipped before), so the node needs them all and no ship-have round trip asks which
/// (~10 ms of a live edit's save -> frame).
pub fn send(c: &Client, node: &str, id: &str, root: &Path, entries: &[Entry], placed: bool) -> Result<Shipped, String> {
    let need = if placed { (0..entries.len()).collect() } else { need(c, node, id, entries)? };
    let mut out = Shipped { files: entries.len(), ..Default::default() };
    let mut ok = |_: &str, _: &[String]| Ok(());
    let mut put = Pipe::open(c, node, "ship-put")?;
    let mut links = Vec::new();
    for &i in &need {
        let e = &entries[i];
        if let Some(t) = &e.link {
            links.push((e.path.clone(), t.clone()));
            continue;
        }
        let (bytes, mtime_us, exec) = read_stable(&root.join(&e.path)).map_err(|err| format!("{}: {err}", e.path))?;
        put_bytes(&mut put, id, &e.path, &bytes, mtime_us, exec, &mut ok)?;
        out.sent += 1;
        out.bytes += bytes.len() as u64;
    }
    put.drain(&mut ok)?;
    out.node_ms += put.node_ms;
    if !links.is_empty() {
        let mut lp = Pipe::open(c, node, "ship-link")?;
        for (p, t) in links {
            lp.send(format!("ship-link {id} {} {}", b64(p.as_bytes()), b64(t.as_bytes())), &mut ok)?;
            out.sent += 1;
        }
        lp.drain(&mut ok)?;
        out.node_ms += lp.node_ms;
    }
    Ok(out)
}

/// The indexes of `entries` the node's work dir lacks or holds differently (ship-have).
fn need(c: &Client, node: &str, id: &str, entries: &[Entry]) -> Result<Vec<usize>, String> {
    let mut have = Pipe::open(c, node, "ship-have").map_err(|e| format!("{e} (a node hub from before the link ship has no ship-have: update its devhub)"))?;
    let mut need: Vec<usize> = Vec::new();
    let mut base = 0usize;
    let mut bases: VecDeque<usize> = VecDeque::new();
    let take_need = |bases: &mut VecDeque<usize>, lines: &[String], need: &mut Vec<usize>| -> Result<(), String> {
        let b = bases.pop_front().unwrap_or(0);
        let v = lines.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).ok_or("ship-have: no answer")?;
        for x in v.get("need").and_then(|n| n.as_arr()).unwrap_or(&[]) {
            if let Some(i) = super::num(x) {
                need.push(b + i as usize);
            }
        }
        Ok(())
    };
    for batch in entries.chunks(HAVE_BATCH) {
        let text: Vec<String> = batch.iter().map(manifest_line).collect();
        bases.push_back(base);
        base += batch.len();
        let line = format!("ship-have {id} {}", b64(text.join("\n").as_bytes()));
        have.send(line, &mut |_, lines| take_need(&mut bases, lines, &mut need))?;
    }
    have.drain(&mut |_, lines| take_need(&mut bases, lines, &mut need))?;
    need.sort_unstable();
    Ok(need)
}

/// A file's bytes with the mtime they were written at. The manifest's stat is older than the read
/// when the file is saved while the tree ships: the new text under the old mtime looked unchanged
/// to the node's cargo (it compares mtimes) until the file was touched again. So the stat is taken
/// around the read, again until both agree; a file that keeps changing goes with the mtime from
/// before its read (older than its text: the next ship sends it again).
pub(crate) fn read_stable(path: &Path) -> std::io::Result<(Vec<u8>, u64, bool)> {
    let mut tries = 0;
    loop {
        let before = std::fs::metadata(path)?;
        let bytes = std::fs::read(path)?;
        let after = std::fs::metadata(path)?;
        let (t0, t1) = (mtime_us(&before), mtime_us(&after));
        if (t0 == t1 && after.len() == bytes.len() as u64) || tries >= 4 {
            return Ok((bytes, if t0 == t1 { t1 } else { t0 }, is_exec(&after)));
        }
        tries += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn put_bytes(p: &mut Pipe, id: &str, path: &str, bytes: &[u8], mtime_us: u64, exec: bool, ok: &mut dyn FnMut(&str, &[String]) -> Result<(), String>) -> Result<(), String> {
    let pb = b64(path.as_bytes());
    let size = bytes.len();
    let mut off = 0usize;
    loop {
        let end = (off + CHUNK).min(size);
        p.send(format!("ship-put {id} {off} {size} {mtime_us} {} {pb} {}", exec as u8, b64(&bytes[off..end])), ok)?;
        off = end;
        if off >= size {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    /// A file saved after the manifest listed it ships with the mtime of the text that goes, not
    /// the manifest's older one (else the node's cargo keeps its old build of it).
    #[test]
    fn a_file_saved_while_shipping_goes_with_its_new_mtime() {
        let dir = std::env::temp_dir().join(format!("devhub-ship-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.rs");
        std::fs::write(&f, "fn a() {}").unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
        let mut listed = Vec::new();
        entries(&dir, "a.rs", &mut listed).unwrap();
        // saved after the listing: new text, a later mtime
        std::fs::write(&f, "fn a() { 1; }").unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(2_000_000)).unwrap();
        let (bytes, mtime, _) = read_stable(&f).unwrap();
        assert_eq!(bytes, b"fn a() { 1; }");
        assert_eq!(mtime, 2_000_000_000_000, "the text's own mtime");
        assert_ne!(mtime, listed[0].mtime_us, "not the listing's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file rewritten in place with its size and its mtime second kept ships its new text at
    /// once: by its nanoseconds, and where git staged it in its index's second or later (there
    /// its whole stat can be kept), by its content. Status went by the size and the second, so
    /// such an edit stayed the committed text on the node (11-06).
    #[cfg(feature = "checkout")]
    #[test]
    fn an_in_place_edit_keeping_size_and_mtime_second_ships() {
        use std::io::Write;
        use std::time::Duration;
        let repo = std::env::temp_dir().join(format!("devhub-ship-racy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| crate::orch::git(&repo, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).unwrap().trim().to_string();
        let write = |p: &str, text: &str, at: std::time::SystemTime| {
            let mut f = std::fs::File::create(repo.join(p)).unwrap();
            f.write_all(text.as_bytes()).unwrap();
            f.set_modified(at).unwrap();
        };
        let now = crate::proto::wall_ms() / 1000;
        let at = |secs: u64, ms: u64| UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(ms);
        write("old.rs", "fn a() {}\n", at(now - 100, 100));
        write("racy.rs", "fn b() {}\n", at(now + 100, 100));
        git(&["init", "-q", "-b", "work"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let pin = crate::lease::Pin { makepad: git(&["rev-parse", "HEAD"]), commercial: None };
        assert_eq!(uncommitted(&repo, &pin).unwrap(), (vec![], vec![]), "racily clean files of their text are clean");
        write("old.rs", "fn x() {}\n", at(now - 100, 200));
        write("racy.rs", "fn y() {}\n", at(now + 100, 100));
        assert_eq!(uncommitted(&repo, &pin).unwrap().0, ["old.rs", "racy.rs"]);
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atomic_writes_temp_file_never_ships() {
        assert!(never_ships("apps/commercial/fab/toolpath/src/support.rs.tmp.81511.2d68cbffff10"));
        assert!(never_ships_named("src/lib.rs.tmp.7.ab"));
        for keep in ["src/a.tmp.rs", "src/tmp.1.ab", "src/a.rs.tmp.81511", "src/a.rs.tmp.x1.ab", "src/a.rs.tmp.1.xyz", "src/a.rs"] {
            assert!(!never_ships(keep), "{keep}");
        }
    }

    #[test]
    fn have_put_link_on_a_work_dir() {
        let base = std::env::temp_dir().join(format!("devhub-ship-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, work) = (base.join("src"), base.join("work"));
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(src.join("a/b/one.txt"), b"hello").unwrap();
        let big: Vec<u8> = (0..(CHUNK * 2 + 17)).map(|i| (i * 7) as u8).collect();
        std::fs::write(src.join("big.bin"), &big).unwrap();
        std::fs::write(src.join("empty"), b"").unwrap();
        let mut es = Vec::new();
        entries(&src, "a", &mut es).unwrap();
        entries(&src, "big.bin", &mut es).unwrap();
        entries(&src, "empty", &mut es).unwrap();
        assert_eq!(es.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a/b/one.txt", "big.bin", "empty"]);
        let man = |es: &[Entry]| b64(es.iter().map(manifest_line).collect::<Vec<_>>().join("\n").as_bytes());
        assert_eq!(have(&work, &man(&es)).unwrap(), r#"{"need":[0,1,2]}"#);
        for e in &es {
            let bytes = std::fs::read(src.join(&e.path)).unwrap();
            let mut off = 0;
            loop {
                let end = (off + CHUNK).min(bytes.len());
                let r = put(&work, &format!("{off} {} {} {} {} {}", bytes.len(), e.mtime_us, e.exec as u8, b64(e.path.as_bytes()), b64(&bytes[off..end]))).unwrap();
                off = end;
                if off >= bytes.len() {
                    assert_eq!(r, r#"{"done":1}"#);
                    break;
                }
            }
        }
        assert_eq!(std::fs::read(work.join("big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(work.join("a/b/one.txt")).unwrap(), b"hello");
        assert_eq!(have(&work, &man(&es)).unwrap(), r#"{"need":[]}"#, "a second ship sends nothing");
        // a changed file is a miss again
        std::fs::write(src.join("a/b/one.txt"), b"hello!").unwrap();
        let mut es2 = Vec::new();
        entries(&src, "a", &mut es2).unwrap();
        assert_eq!(have(&work, &man(&es2)).unwrap(), r#"{"need":[0]}"#);
        // paths stay inside the work dir
        assert!(put(&work, &format!("0 1 0 0 {} {}", b64(b"../x"), b64(b"x"))).is_err());
        assert!(put(&work, &format!("0 1 0 0 {} {}", b64(b"/etc/x"), b64(b"x"))).is_err());
        assert!(have(&work, &b64(b"..\\x\t1\t0\t0")).is_err());
        assert!(put(&work, &format!("0 1 0 0 {} {}", b64(b"big.bin"), b64(b"xy"))).is_err(), "past its size");
        // an empty file as the trimmed line carries it
        assert_eq!(put(&work, &format!("0 0 0 0 {}", b64(b"e2"))).unwrap(), r#"{"done":1}"#);
        assert_eq!(std::fs::read(work.join("e2")).unwrap(), b"");
        let r = link(&work, &format!("{} {}", b64(b"ln"), b64(b"a/b/one.txt"))).unwrap();
        assert!(r.contains("done") || r.contains("skipped"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A shipped file whose source mtime is older than the node's build artifacts must be rebuilt
    /// by the node's cargo: the node writes it with its own time (JITPROV: loaders built from old
    /// sources). The node still knows it holds that file as the source has it.
    #[test]
    fn a_shipped_file_older_than_the_artifacts_is_rebuilt() {
        let base = std::env::temp_dir().join(format!("devhub-ship-mtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("Cargo.toml"), "[package]\nname = \"shipmtime\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n").unwrap();
        std::fs::write(work.join("src/main.rs"), "fn main() { println!(\"old\"); }\n").unwrap();
        let cargo = |args: &[&str]| std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).args(args).current_dir(&work).env("CARGO_TARGET_DIR", work.join("target")).env_remove("RUSTFLAGS").output().unwrap();
        assert!(cargo(&["build", "-q", "--offline"]).status.success());
        // the source on the Mac: new text, saved a day before the node's build
        let src = "fn main() { println!(\"new\"); }\n";
        let old_us = (std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64).saturating_sub(86_400_000_000);
        let before = std::time::SystemTime::now();
        let r = put(&work, &format!("0 {} {old_us} 0 {} {}", src.len(), b64(b"src/main.rs"), b64(src.as_bytes()))).unwrap();
        assert_eq!(r, r#"{"done":1}"#);
        let m = std::fs::metadata(work.join("src/main.rs")).unwrap().modified().unwrap();
        assert!(m >= before - std::time::Duration::from_secs(1), "written with the node's time, not the source's");
        let out = cargo(&["run", "-q", "--offline"]);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "new", "cargo rebuilt from the shipped source");
        // the same source again is held: nothing ships
        let man = b64(format!("src/main.rs\t{}\t{old_us}\t0", src.len()).as_bytes());
        assert_eq!(have(&work, &man).unwrap(), r#"{"need":[]}"#);
        // a restore of the file (a checkout writes it) makes it a miss again
        std::fs::write(work.join("src/main.rs"), "fn main() { println!(\"old\"); }\n").unwrap();
        assert_eq!(have(&work, &man).unwrap(), r#"{"need":[0]}"#);
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// The repositories of a tree pinned at `pin`: (dir, commit, path prefix in the tree).
fn pinned_repos(root: &Path, pin: &super::Pin) -> Vec<(PathBuf, String, &'static str)> {
    std::iter::once((root.to_path_buf(), pin.makepad.clone(), "")).chain(pin.commercial.clone().map(|s| (root.join("apps/commercial"), s, "apps/commercial/"))).collect()
}

/// Whether `path` (in the tree) belongs to the repository with `prefix` under `pin`: the makepad
/// side leaves a pinned nested commercial checkout's paths to it.
fn in_repo(pin: &super::Pin, prefix: &str, path: &str) -> bool {
    if prefix.is_empty() { !(pin.commercial.is_some() && path.starts_with("apps/commercial/")) } else { path.starts_with(prefix) }
}

/// The blob of the file at `path` in `tree` (a file or a symlink; None for nothing, a folder or a
/// gitlink: what the node's `relaxgit::bases` table holds).
fn blob_at(repo: &mut makepad_git::Repository, tree: &makepad_git::ObjectId, path: &str) -> Result<Option<makepad_git::ObjectId>, String> {
    let parts: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let mut at = *tree;
    for (i, name) in parts.iter().enumerate() {
        let t = repo.read_tree(&at).map_err(|e| format!("{path}: {e}"))?;
        let Some(e) = t.entries.iter().find(|e| e.name == *name) else { return Ok(None) };
        if i + 1 == parts.len() {
            return Ok((e.is_blob() || e.mode == 0o120000).then_some(e.oid));
        }
        if !e.is_tree() { return Ok(None); }
        at = e.oid;
    }
    Ok(None)
}

/// The pin's blob of each of `paths` ("new" where it has no file), per repository, read in process.
fn pin_blobs(root: &Path, pin: &super::Pin, paths: &[String]) -> Result<std::collections::HashMap<String, String>, String> {
    let mut blobs = std::collections::HashMap::new();
    for (dir, sha, prefix) in pinned_repos(root, pin) {
        let mine: Vec<&String> = paths.iter().filter(|p| in_repo(pin, prefix, p)).collect();
        if mine.is_empty() { continue; }
        let found = crate::gitread::with(&dir, |p, repo| -> Result<Vec<Option<makepad_git::ObjectId>>, String> {
            let commit = crate::gitread::resolve(p, repo, &sha).ok_or_else(|| format!("{}: no commit {sha}", dir.display()))?;
            let tree = repo.read_commit(&commit).map_err(|e| format!("{sha}: {e}"))?.tree;
            mine.iter().map(|path| blob_at(repo, &tree, &path[prefix.len()..])).collect()
        }).ok_or_else(|| format!("{}: not a git repository", dir.display()))??;
        for (path, blob) in mine.into_iter().zip(found) {
            blobs.insert(path.clone(), blob.map_or_else(|| "new".to_string(), |b| b.to_hex()));
        }
    }
    Ok(blobs)
}

/// The base blob of each shipped path at the pin, as `ship-done` sends them: `new` where the pin
/// has no file. Read in process: no git process per ship (two `ls-tree -r` of whole trees were).
pub fn bases_json(root: &Path, pin: &super::Pin, paths: &[String]) -> Result<String, String> {
    let blobs = pin_blobs(root, pin, paths)?;
    Ok(format!("{{{}}}", paths.iter().map(|p| format!("{}:{}", makepad_remote::json_str(p), makepad_remote::json_str(blobs.get(p).map_or("new", String::as_str)))).collect::<Vec<_>>().join(",")))
}

/// Current files and deletions against a fixed pin (a live run keeps that pin across saves), in
/// process: each tree's status (what differs from its HEAD, untracked files not ignored), and when
/// the lane committed since the pin, the files those commits changed. No git process.
pub fn uncommitted(root: &Path, pin: &super::Pin) -> Result<(Vec<String>, Vec<String>), String> {
    uncommitted_with(root, pin, |dir| {
        let e = |e: makepad_git::GitError| format!("{}: {e}", dir.display());
        let p = makepad_git::repository_paths(dir).map_err(e)?.ok_or_else(|| format!("{}: not a git worktree", dir.display()))?;
        let head = makepad_git::refs::resolve_head_in(&p.git_dir, &p.common_dir).map_err(e)?.to_hex();
        let status = crate::gitread::worktree(p).map_err(e)?.status().map_err(|err| format!("{}: status: {err}", dir.display()))?;
        Ok((head, status.entries.into_iter().map(|s| s.path).collect()))
    })
}

/// [`uncommitted`] from the master's file monitor: its dirty set of each tree, kept from file
/// events, instead of a status. A live ship asked it with six git processes, 0.2-1 s of the save ->
/// shipped time on the Mac (`git diff` against a commit re-reads a stale index each time).
#[cfg(feature = "checkout")]
pub fn uncommitted_seen(monitor: &crate::monitor::Monitor, root: &Path, pin: &super::Pin) -> Result<(Vec<String>, Vec<String>), String> {
    uncommitted_with(root, pin, |dir| {
        let d = monitor.dirty(dir)?;
        Ok((d.head, d.changed.into_iter().chain(d.deleted).collect()))
    })
}

/// `dirty(dir)`: a repository's HEAD and the paths that differ from it. Adds what changed between
/// the pin and that HEAD (the lane's commits since), then sorts by what is on disk: a file there
/// ships, a path gone is a deletion when the pin has it.
fn uncommitted_with(root: &Path, pin: &super::Pin, dirty: impl Fn(&Path) -> Result<(String, Vec<String>), String>) -> Result<(Vec<String>, Vec<String>), String> {
    let mut paths = std::collections::BTreeSet::new();
    for (dir, sha, prefix) in pinned_repos(root, pin) {
        let (head, mut rel) = dirty(&dir)?;
        if head != sha {
            let id = |s: &str| makepad_git::ObjectId::from_hex(s).map_err(|e| format!("{s}: {e}"));
            let (from, to) = (id(&sha)?, id(&head)?);
            let changes = crate::gitread::with(&dir, |_, repo| repo.diff_commits(&from, &to).map_err(|e| format!("{}: {sha}..{head}: {e}", dir.display())))
                .ok_or_else(|| format!("{}: not a git repository", dir.display()))??;
            rel.extend(changes.into_iter().map(|c| match c {
                makepad_git::TreeChange::Added { path, .. } | makepad_git::TreeChange::Modified { path, .. } | makepad_git::TreeChange::Deleted { path, .. } => path,
            }));
        }
        paths.extend(rel.into_iter().map(|p| format!("{prefix}{p}")).filter(|p| in_repo(pin, prefix, p) && !never_ships(p)));
    }
    let (mut changed, mut gone) = (Vec::new(), Vec::new());
    for p in paths {
        match std::fs::symlink_metadata(root.join(&p)) {
            Ok(m) if !m.is_dir() => changed.push(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => gone.push(p),
            Err(e) => return Err(format!("{p}: {e}")),
            _ => {}
        }
    }
    // a file added after the pin and gone again is no deletion of the pin's
    let blobs = pin_blobs(root, pin, &gone)?;
    let deleted = gone.into_iter().filter(|p| blobs.get(p).is_some_and(|b| b != "new")).collect();
    Ok((changed, deleted))
}
