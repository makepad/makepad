//! The size map, kept between runs.
//!
//! Measuring a full home directory takes minutes, and the answer barely
//! changes between one look and the next — so the finished tree is written to
//! disk and read straight back the next time the same folder is mapped. A map
//! that appears instantly is the difference between a tool somebody reaches
//! for while cleaning up and one they open once.
//!
//! What the cache costs in truth, it pays back in honesty elsewhere: the view
//! says when the map was made, deletions the app itself performs are folded
//! straight into the cached tree (so a delete never costs a rescan), and a
//! rescan is one keystroke away. Changes made *outside* the app are not seen
//! until then, and the "scanned 2h ago" line is there so nobody is surprised
//! by that.
//!
//! The format is a plain little-endian byte stream with a magic number and a
//! version in front. Nothing here ever tries to read an older layout: a
//! version bump simply makes every existing file unreadable, the load returns
//! `None`, and the app falls back to a fresh scan. A cache is an optimisation,
//! and an optimisation that can fail loudly is worse than one that cannot fail
//! at all.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::treemap::Node;

/// `MPFM`, so a stray file in the cache directory is never mistaken for one
/// of ours.
const MAGIC: u32 = 0x4D50_464D;
/// Bump this whenever [`Node`]'s encoding changes. Every older file then
/// fails to load and is simply rewritten by the next scan.
const VERSION: u32 = 2;

/// A ceiling on how big a tree is worth keeping. Past this the file itself
/// becomes slow enough to read that a fresh scan is competitive, and writing
/// it would spend more of the user's disk than the map is worth on a disk they
/// are trying to empty.
const MAX_NODES: u64 = 4_000_000;

/// And a ceiling on the file itself. The node count only estimates the size —
/// names vary — and this is a tool for people whose disk is nearly full. It
/// must never be the thing that fills it.
const MAX_BYTES: usize = 192 << 20;

/// Longest name we will believe out of a cache file. Anything past it means
/// the file is damaged, and the load gives up rather than allocating whatever
/// number it just read.
const MAX_NAME: u32 = 4096;

/// A map read back off the disk.
pub struct Cached {
    /// When the scan that produced this ran, in seconds since the epoch.
    pub scanned_at: u64,
    pub tree: Node,
}

/// Seconds since the epoch, or 0 when the clock cannot say.
pub fn now() -> u64 {
    crate::vfs::now_secs()
}

/// "2h ago", "just now" — how a person reads an age.
pub fn age_text(scanned_at: u64) -> String {
    let now = now();
    if scanned_at == 0 || now < scanned_at {
        return "scanned just now".to_string();
    }
    let seconds = now - scanned_at;
    if seconds < 90 {
        "scanned just now".to_string()
    } else if seconds < 5400 {
        format!("scanned {}m ago", seconds / 60)
    } else if seconds < 172_800 {
        format!("scanned {}h ago", seconds / 3600)
    } else {
        format!("scanned {}d ago", seconds / 86_400)
    }
}

/// Where the map of `root` lives.
///
/// The file is named by a hash of the absolute path rather than by the path
/// itself: a path can be longer than a filename may be, and can hold every
/// character a filename may not.
fn cache_path(root: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".config/mpfiles/sizemaps");
    // The scan scope is part of the map's identity: a tree measured with the
    // system folders in it must never be served as the excluded one, or the
    // other way round. Both scopes keep their own file, so flipping the
    // checkbox back is instant once each has been scanned.
    let scope = if crate::model::scan_all() { "-all" } else { "" };
    Some(dir.join(format!("{:016x}{scope}.map", hash_path(root))))
}

/// FNV-1a over the path's bytes. Not a security hash — a name, and a stable
/// one across runs, which `DefaultHasher` explicitly is not.
fn hash_path(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The map of `root`, if one was saved and still reads.
///
/// Anything wrong with the file — wrong magic, wrong version, truncated,
/// written for a different folder — is a miss, not an error. The caller scans.
pub fn load(root: &Path) -> Option<Cached> {
    let path = cache_path(root)?;
    let mut bytes = Vec::new();
    fs::File::open(&path).ok()?.read_to_end(&mut bytes).ok()?;
    let mut reader = Reader {
        bytes: &bytes,
        at: 0,
    };
    if reader.u32()? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    let scanned_at = reader.u64()?;
    let saved_root = reader.string()?;
    // The hash names the file; this confirms it. Two different folders whose
    // paths collide would otherwise show each other's map.
    if Path::new(&saved_root) != root {
        return None;
    }
    let tree = reader.node(0)?;
    Some(Cached { scanned_at, tree })
}

/// The bytes of a saved map, or `None` when the tree is too big to be worth
/// keeping. Encoding is the caller's to schedule — it walks the whole tree,
/// so it belongs wherever the tree already is rather than behind a clone.
pub fn encode(root: &Path, tree: &Node, scanned_at: u64) -> Option<Vec<u8>> {
    if tree.files as u64 > MAX_NODES {
        return None;
    }
    let mut out = Vec::with_capacity(1 << 16);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&scanned_at.to_le_bytes());
    write_string(&mut out, &root.display().to_string());
    write_node(&mut out, tree);
    (out.len() <= MAX_BYTES).then_some(out)
}

/// Put `bytes` where [`load`] will find them for `root`. Best effort: a cache
/// that cannot be written is a cache miss next time, and never a failure the
/// user has to hear about.
pub fn store(root: &Path, bytes: &[u8]) {
    let Some(path) = cache_path(root) else {
        return;
    };
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    // Written beside the real file and renamed over it, so a map that is
    // still being written is never a map that gets read.
    let temp = path.with_extension("part");
    let wrote = fs::File::create(&temp).and_then(|mut file| file.write_all(bytes));
    if wrote.is_ok() {
        let _ = fs::rename(&temp, &path);
    } else {
        let _ = fs::remove_file(&temp);
    }
}

/// Throw away the saved map of `root`, so the next look measures the disk.
pub fn forget(root: &Path) {
    if let Some(path) = cache_path(root) {
        let _ = fs::remove_file(path);
    }
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn write_node(out: &mut Vec<u8>, node: &Node) {
    write_string(out, &node.name);
    let flags = (node.is_dir as u8) | ((node.denied as u8) << 1);
    out.push(flags);
    out.push(node.kind);
    out.extend_from_slice(&node.size.to_le_bytes());
    out.extend_from_slice(&node.files.to_le_bytes());
    out.extend_from_slice(&node.modified.to_le_bytes());
    out.extend_from_slice(&(node.children.len() as u32).to_le_bytes());
    for child in &node.children {
        write_node(out, child);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn string(&mut self) -> Option<String> {
        let len = self.u32()?;
        if len > MAX_NAME {
            return None;
        }
        String::from_utf8(self.take(len as usize)?.to_vec()).ok()
    }

    /// One node and everything under it. `depth` is carried only to stop a
    /// damaged file from recursing until the stack runs out — a cache is not
    /// a trusted input just because we wrote it.
    fn node(&mut self, depth: usize) -> Option<Node> {
        if depth > 512 {
            return None;
        }
        let name = self.string()?;
        let flags = self.u8()?;
        let kind = self.u8()?;
        let size = self.u64()?;
        let files = self.u32()?;
        let modified = self.u32()?;
        let count = self.u32()?;
        // A child count larger than the bytes left could only come from a
        // damaged file, and reserving on it would be the damage's whole point.
        if count as usize > self.bytes.len() - self.at {
            return None;
        }
        let mut children = Vec::with_capacity(count as usize);
        for _ in 0..count {
            children.push(self.node(depth + 1)?);
        }
        Some(Node {
            name,
            is_dir: flags & 1 != 0,
            done: true,
            denied: flags & 2 != 0,
            kind,
            size,
            files,
            modified,
            children,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Node {
        let mut root = Node::dir("root".into(), 0);
        root.children.push(Node::file_at("a.mov".into(), 5, 900, 123_456));
        root.modified = 123_456;
        let mut sub = Node::dir("sub".into(), 0);
        sub.children.push(Node::file("b.txt".into(), 2, 30));
        sub.size = 30;
        sub.files = 1;
        sub.done = true;
        let mut locked = Node::dir("locked".into(), 0);
        locked.denied = true;
        locked.done = true;
        root.children.push(sub);
        root.children.push(locked);
        root.size = 930;
        root.files = 2;
        root.done = true;
        root
    }

    fn round_trip(tree: &Node) -> Node {
        let root = Path::new("/some/where");
        let bytes = encode(root, tree, 1234).unwrap();
        let mut reader = Reader {
            bytes: &bytes,
            at: 0,
        };
        assert_eq!(reader.u32().unwrap(), MAGIC);
        assert_eq!(reader.u32().unwrap(), VERSION);
        assert_eq!(reader.u64().unwrap(), 1234);
        assert_eq!(reader.string().unwrap(), "/some/where");
        reader.node(0).unwrap()
    }

    #[test]
    fn a_tree_survives_the_round_trip_unchanged() {
        let tree = sample();
        let back = round_trip(&tree);
        assert_eq!(back.name, tree.name);
        assert_eq!(back.size, tree.size);
        assert_eq!(back.files, tree.files);
        assert_eq!(back.children.len(), 3);
        assert_eq!(back.children[0].name, "a.mov");
        assert_eq!(back.children[0].kind, 5);
        // v2's whole point: the age survives, so "show me what's new" works
        // straight off a loaded map.
        assert_eq!(back.children[0].modified, 123_456);
        assert_eq!(back.modified, 123_456);
        assert_eq!(back.children[1].children[0].name, "b.txt");
        // The one thing a reload must not forget: which folders it could not
        // read, so the map keeps admitting the total is short.
        assert!(back.children[2].denied);
        // Everything read back is finished by definition — only a completed
        // scan is ever written.
        assert!(back.done && back.children[1].done);
    }

    #[test]
    fn a_damaged_file_is_a_miss_and_never_a_panic() {
        let bytes = encode(Path::new("/x"), &sample(), 1).unwrap();
        for cut in [0, 4, 9, 20, bytes.len() - 1] {
            let mut reader = Reader {
                bytes: &bytes[..cut],
                at: 0,
            };
            // Whatever it reads, it must stop rather than run off the end.
            let _ = reader.u32().and_then(|_| reader.node(0));
        }
        // Wrong magic, right length.
        let mut wrong = bytes.clone();
        wrong[0] ^= 0xff;
        let mut reader = Reader {
            bytes: &wrong,
            at: 0,
        };
        assert_ne!(reader.u32().unwrap(), MAGIC);
    }

    #[test]
    fn the_file_name_follows_the_folder_not_the_other_way_round() {
        assert_ne!(hash_path(Path::new("/a")), hash_path(Path::new("/b")));
        assert_eq!(hash_path(Path::new("/a/b")), hash_path(Path::new("/a/b")));
    }

    #[test]
    fn ages_read_the_way_a_person_would_say_them() {
        let now = now();
        assert_eq!(age_text(now), "scanned just now");
        assert_eq!(age_text(now - 600), "scanned 10m ago");
        assert_eq!(age_text(now - 7200), "scanned 2h ago");
        assert_eq!(age_text(now - 3 * 86_400), "scanned 3d ago");
        // A clock that went backwards is not a reason to print nonsense.
        assert_eq!(age_text(now + 5000), "scanned just now");
    }
}
