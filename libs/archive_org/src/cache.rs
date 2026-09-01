//! Where fetched bytes live: a digest-keyed cache that cannot go stale.
//!
//! An archive file is immutable under its (item, name) — re-uploads make
//! a new name or a new item — so the cache key is a hash of exactly that
//! pair, and the file keeps its own extension so decoders can sniff it.
//! There is no index: the file either exists at its name or it does not.

use std::path::{Path, PathBuf};

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 15) as usize] as char);
    }
    out
}

/// A filename-safe stem: ASCII letters, digits, `-`, `_`; everything else
/// becomes `-`, runs collapsed, bounded.
pub fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= max {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "file".to_string()
    } else {
        out
    }
}

/// `cache_dir/media/<identifier-slug>-<digest16>.<ext>` for one file of
/// one item.
pub fn cache_file_for(cache_dir: &Path, identifier: &str, name: &str) -> PathBuf {
    let key = format!("{identifier}/{name}");
    let digest = makepad_network::digest::sha256_hash(key.as_bytes());
    let ext = name
        .rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".to_string());
    cache_dir
        .join("media")
        .join(format!("{}-{}.{}", slug(identifier, 40), &hex(&digest)[..16], ext))
}

/// The in-progress twin of a cache file: `clip.part.mp4` beside `clip.mp4`.
/// The real extension stays LAST so a decoder asked to read the growing
/// file (a swatch that starts before the download ends) recognises it.
pub fn part_file_for(dest: &Path) -> PathBuf {
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => dest.with_file_name(format!("{stem}.part.{ext}")),
        None => dest.with_file_name(format!("{stem}.part")),
    }
}

/// The HEAD of a cache file — the first N bytes of a big clip, kept for
/// auditioning: `clip.head.mp4` beside `clip.mp4`. A different name from
/// the whole file, so a truncated swatch can never be mistaken for an
/// import-ready download.
pub fn head_file_for(dest: &Path) -> PathBuf {
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    match dest.extension().and_then(|e| e.to_str()) {
        Some(ext) => dest.with_file_name(format!("{stem}.head.{ext}")),
        None => dest.with_file_name(format!("{stem}.head")),
    }
}

/// `cache_dir/thumbs/<identifier-slug>-<digest16>.jpg` for an item tile.
pub fn thumb_file_for(cache_dir: &Path, identifier: &str) -> PathBuf {
    let digest = makepad_network::digest::sha256_hash(identifier.as_bytes());
    cache_dir
        .join("thumbs")
        .join(format!("{}-{}.jpg", slug(identifier, 40), &hex(&digest)[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Apple Fukkireta.mp4", 64), "apple-fukkireta-mp4");
        assert_eq!(slug("///", 8), "file");
        assert_eq!(slug("abcdefghij", 4), "abcd");
    }

    #[test]
    fn paths() {
        let dir = Path::new("/c");
        let a = cache_file_for(dir, "item", "Content/clip one.MP4");
        assert!(a.starts_with("/c/media"));
        assert_eq!(a.extension().unwrap(), "mp4");
        assert_eq!(a, cache_file_for(dir, "item", "Content/clip one.MP4"));
        assert_ne!(a, cache_file_for(dir, "item", "Content/clip two.MP4"));
        assert_ne!(a, cache_file_for(dir, "other", "Content/clip one.MP4"));
        assert_eq!(cache_file_for(dir, "i", "noext").extension().unwrap(), "bin");
        assert_eq!(cache_file_for(dir, "i", "x.tar.gz").extension().unwrap(), "gz");
        assert!(thumb_file_for(dir, "item").starts_with("/c/thumbs"));
        assert_eq!(part_file_for(Path::new("/c/media/x.mp4")), PathBuf::from("/c/media/x.part.mp4"));
        assert_eq!(part_file_for(Path::new("/c/media/x")), PathBuf::from("/c/media/x.part"));
        assert_eq!(head_file_for(Path::new("/c/media/x.mp4")), PathBuf::from("/c/media/x.head.mp4"));
        assert_eq!(
            part_file_for(&head_file_for(Path::new("/c/media/x.mp4"))),
            PathBuf::from("/c/media/x.head.part.mp4")
        );
    }
}
