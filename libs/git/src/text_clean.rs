//! Git's built-in text conversion on the way into the object store: why a file that differs from
//! its blob byte for byte is still clean (checkout CRLFs under `eol=crlf`, an expanded `$Id$`).
use crate::oid::ObjectId;
use crate::repo::RepositoryPaths;
use std::path::Path;

/// The config [`cleans_to`] reads: the common one, and the worktree's where the repository uses it.
pub fn conversion_config(p: &RepositoryPaths) -> Result<String, String> {
    let mut config_text = match std::fs::read_to_string(p.common_dir.join("config")) {
        Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(), Err(e) => return Err(e.to_string()),
    };
    if crate::config::get(&config_text, "extensions.worktreeconfig").map_err(|e| e.to_string())?.as_deref().and_then(crate::config::parse_bool) == Some(true) {
        match std::fs::read_to_string(p.git_dir.join("config.worktree")) {
            Ok(extra) => { config_text.push('\n'); config_text.push_str(&extra); }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(config_text)
}

/// Whether the tracked file `path` under `root` is the blob `oid` once git's built-in conversion is
/// undone: a CSV or script with checkout CRLFs where the index stores LF. External clean filters are
/// not run, so a file under one, or under attributes this can't read, is an error: not proved clean.
pub fn cleans_to(root: &Path, common: &Path, path: &str, oid: ObjectId, config: &str) -> Result<bool, String> {
    let autocrlf = crate::config::get(config, "core.autocrlf").map_err(|e| e.to_string())?;
    let mut text = if matches!(autocrlf.as_deref(), Some("true" | "input")) { Some("auto".to_string()) } else { None };
    let mut eol = false;
    let mut ident = false;
    let mut filter = false;
    let mut files = vec![(root.join(".gitattributes"), path.to_string())];
    for (i, _) in path.match_indices('/') {
        files.push((root.join(&path[..i]).join(".gitattributes"), path[i+1..].to_string()));
    }
    files.push((common.join("info/attributes"), path.to_string()));
    for (file, relative) in files {
        let rules = match std::fs::read_to_string(file) {
            Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, Err(e) => return Err(e.to_string()),
        };
        for line in rules.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut words = line.split_whitespace();
            let pattern = words.next().unwrap();
            if pattern.starts_with('"') || pattern.starts_with("[attr]") { return Err("tracked status needs an unsupported attribute pattern or macro".into()); }
            let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
            let name = if pattern.contains('/') { relative.as_str() } else { relative.rsplit('/').next().unwrap() };
            if !crate::ignore::wildmatch(pattern.as_bytes(), name.as_bytes()) { continue; }
            for word in words {
                match word {
                    "binary" | "-text" => text = Some("false".into()),
                    "text" => text = Some("true".into()),
                    "text=auto" => text = Some("auto".into()),
                    "!text" => text = None,
                    "eol=lf" | "eol=crlf" => eol = true,
                    "-eol" | "!eol" => eol = false,
                    "ident" => ident = true,
                    "-ident" | "!ident" => ident = false,
                    "-filter" | "!filter" => filter = false,
                    w if w.starts_with("filter=") || w.starts_with("working-tree-encoding=") => filter = true,
                    _ => {},
                }
            }
        }
    }
    if filter { return Err("tracked status cannot prove a changed filtered file clean".into()); }
    let file = root.join(path);
    let normalize = match text.as_deref() {
        Some("false") => false,
        Some("true") => true,
        Some("auto") => text_bytes(&file).map_err(|e| e.to_string())?,
        _ => eol,
    };
    if !normalize && !ident { return Ok(false); }
    let length = canonical_bytes(&file, normalize, ident, |_| {}).map_err(|e| e.to_string())?;
    let mut hash = crate::Sha1::new();
    hash.update(format!("blob {length}\0").as_bytes());
    canonical_bytes(&file, normalize, ident, |bytes| hash.update(bytes)).map_err(|e| e.to_string())?;
    Ok(ObjectId::from_bytes(hash.finalize()) == oid)
}

fn byte(reader: &mut std::io::BufReader<std::fs::File>) -> std::io::Result<Option<u8>> {
    use std::io::BufRead;
    let b = reader.fill_buf()?.first().copied();
    if b.is_some() { reader.consume(1); }
    Ok(b)
}

/// Match Git's text=auto classification without retaining the file.
fn text_bytes(path: &Path) -> std::io::Result<bool> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let (mut printable, mut nonprintable) = (0u64, 0u64);
    let mut last = None;
    while let Some(b) = byte(&mut reader)? {
        if b == 0 || (last == Some(b'\r') && b != b'\n') { return Ok(false); }
        match b {
            b'\r' | b'\n' => {},
            8 | 9 | 12 | 27 => printable += 1,
            0..=31 | 127 => nonprintable += 1,
            _ => printable += 1,
        }
        last = Some(b);
    }
    if last == Some(26) { nonprintable -= 1; }
    Ok(last != Some(b'\r') && nonprintable <= printable >> 7)
}

/// Two streaming passes supply the blob header length and its hash. Seeking
/// back after a non-Id dollar expression avoids buffering arbitrarily long lines.
fn canonical_bytes(path: &Path, normalize: bool, ident: bool, mut emit: impl FnMut(&[u8])) -> std::io::Result<u64> {
    use std::io::{BufRead, Read, Seek, SeekFrom};
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut block = [0u8; 8192];
    let (mut used, mut total) = (0, 0u64);
    let mut push = |bytes: &[u8]| {
        for b in bytes {
            block[used] = *b; used += 1; total += 1;
            if used == block.len() { emit(&block); used = 0; }
        }
    };
    while let Some(b) = byte(&mut reader)? {
        if normalize && b == b'\r' && reader.fill_buf()?.first() == Some(&b'\n') { continue; }
        if ident && b == b'$' {
            let start = reader.stream_position()?;
            let mut prefix = [0; 3];
            let mut matched = false;
            if reader.read_exact(&mut prefix).is_ok() && prefix == *b"Id:" {
                while let Some(end) = byte(&mut reader)? {
                    if end == b'$' { matched = true; break; }
                    if end == b'\n' { break; }
                }
            }
            if matched { push(b"$Id$"); continue; }
            reader.seek(SeekFrom::Start(start))?;
        }
        push(&[b]);
    }
    drop(push);
    emit(&block[..used]);
    Ok(total)
}
