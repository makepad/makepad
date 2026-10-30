//! Shipping a lane's tree to a lease's work dir over the devhub link (no ssh, no rsync: the same
//! on Linux, macOS and Windows nodes).
//!
//! The client sends a manifest (`path \t size \t mtime µs \t x` lines, in batches) to
//! `ship-have`; the node answers which entries it does not hold as they are (a regular file of
//! that size and mtime, and on Unix that executable bit). The client sends each miss in chunks to
//! `ship-put` (written to `<path>.devhub-part`, renamed into place with the source's mtime and
//! mode after its last byte), and symlinks to `ship-link` (Unix nodes). The file list then goes
//! as `.devhub-files.new`, and `lease-prune` removes what the previous ship had and this one has
//! not. Up to [`WINDOW`] requests are in flight on one routed channel per service.

use makepad_remote::conn::Event;
use makepad_remote::link::Client;
use std::collections::VecDeque;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

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

fn b64(b: &[u8]) -> String {
    makepad_network::digest::base64_encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    super::exec::b64_decode(s)
}

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
fn inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty() && !p.is_absolute() && !rel.contains('\\') && p.components().all(|c| matches!(c, std::path::Component::Normal(_)))
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
                std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file() && Some(m.len()) == size && Some(mtime_us(&m)) == mtime && (cfg!(not(unix)) || is_exec(&m) == x))
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
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(off == 0).open(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    f.seek(SeekFrom::Start(off)).and_then(|_| f.write_all(&data)).map_err(|e| format!("{}: {e}", part.display()))?;
    if off + data.len() as u64 == size {
        let t = UNIX_EPOCH + std::time::Duration::from_micros(mtime);
        f.set_modified(t).map_err(|e| format!("{}: {e}", part.display()))?;
        drop(f);
        set_exec(&part, x == "1");
        if std::fs::symlink_metadata(&dst).is_ok_and(|m| m.is_dir()) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("ship-put {rel}: a directory is in the way"));
        }
        std::fs::rename(&part, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
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
}

impl Pipe {
    fn open(c: &Client, node: &str, service: &str) -> Result<Pipe, String> {
        let ch = c.open("route", &format!("{node} {service} ")).map_err(|e| format!("{node} {service}: {e}"))?;
        Ok(Pipe { ch, pending: VecDeque::new(), buf: String::new(), what: format!("{node} {service}") })
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
}

/// Ships `entries` from `root` into lease `id`'s work dir on `node`, then `list` (the prune list)
/// as `.devhub-files.new`.
pub fn send(c: &Client, node: &str, id: &str, root: &Path, entries: &[Entry], list: &str) -> Result<Shipped, String> {
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
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
    put_bytes(&mut put, id, ".devhub-files.new", list.as_bytes(), now, false, &mut ok)?;
    put.drain(&mut ok)?;
    if !links.is_empty() {
        let mut lp = Pipe::open(c, node, "ship-link")?;
        for (p, t) in links {
            lp.send(format!("ship-link {id} {} {}", b64(p.as_bytes()), b64(t.as_bytes())), &mut ok)?;
            out.sent += 1;
        }
        lp.drain(&mut ok)?;
    }
    Ok(out)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
