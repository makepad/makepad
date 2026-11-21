//! `put <node> <local path> <dest> [--replace]`: a named local folder (or one file) copied into a
//! node's synced checkout, under its `local/` (the state the node's apps keep there: a game
//! library, an app's home), the counterpart of `get` over the same devhub link (no ssh). Its
//! files go into a staging folder beside the destination (`<dest>.put-part`), chunk by chunk;
//! the last request checks their count and bytes and puts them in place: by default each file
//! into the destination (a file of that name replaced, the rest of the folder left as it was);
//! `--replace` makes the destination the copy, the folder it held kept beside it as
//! `<dest>.before-put-<ms>`. Writes need the node's dev switch on, as launches do. One put to a
//! destination at a time.
//!
//! The words of the node's `put` service: `put-begin <dest>`, `put-file <dest> <path b64> <off>
//! <size> <data b64>`, `put-done <dest> merge|replace <files> <bytes>`.

use super::conf::SyncConf;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CHUNK: usize = 256 << 10;

/// `<checkout>/local/<dest>`: `dest` a relative path no store could be steered outside its
/// root with ([`super::files::valid_path`]), none of whose folders below `local/` is a symlink.
fn dest_path(checkout: &Path, dest: &str) -> Result<PathBuf, String> {
    if !super::files::valid_path(dest) {
        return Err(format!("put: `{dest}` is not a relative path under the checkout's local/"));
    }
    let mut p = checkout.join("local");
    for seg in dest.split('/') {
        p.push(seg);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("put: {} is a symlink", p.display()));
        }
    }
    Ok(p)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn staging(dest: &Path) -> PathBuf {
    with_suffix(dest, ".put-part")
}

/// The files under `dir`, relative and `/`-separated, sorted, with their sizes; a symlink is an
/// error (a put copies files, never links).
fn files_under(dir: &Path) -> Result<Vec<(String, PathBuf, u64)>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, u64)>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let kind = e.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
            if kind.is_symlink() {
                return Err(format!("{} is a symlink", p.display()));
            } else if kind.is_dir() {
                walk(root, &p, out)?;
            } else {
                let rel = p.strip_prefix(root).unwrap_or(&p).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                let n = e.metadata().map_err(|e| format!("{}: {e}", p.display()))?.len();
                out.push((rel, p, n));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// One request of the node's `put` service: its answer line, and for a finished put the line its
/// hub record says.
pub(super) fn handle(checkout: &Path, line: &str) -> Result<(String, Option<String>), String> {
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    match word {
        "put-begin" => {
            let dest = dest_path(checkout, rest.trim())?;
            let st = staging(&dest);
            let _ = std::fs::remove_dir_all(&st);
            std::fs::create_dir_all(&st).map_err(|e| format!("{}: {e}", st.display()))?;
            Ok((format!("{{\"staging\":{}}}", json_str(&st.display().to_string())), None))
        }
        "put-file" => {
            let mut w: Vec<&str> = rest.splitn(5, ' ').collect();
            if w.len() == 4 {
                w.push("");
            }
            let [dest, path, off, size, data] = w[..] else { return Err("usage: put-file <dest> <path b64> <off> <size> <data b64>".into()) };
            let st = staging(&dest_path(checkout, dest)?);
            let path = crate::lease::exec::b64_decode(path).and_then(|p| String::from_utf8(p).ok()).ok_or("put-file: the path is not base64 UTF-8")?;
            if !super::files::valid_path(&path) {
                return Err(format!("put-file: `{path}` is not a relative path"));
            }
            let (Ok(off), Ok(size)) = (off.parse::<u64>(), size.parse::<u64>()) else { return Err("put-file: bad numbers".into()) };
            let data = crate::lease::exec::b64_decode(data.trim()).ok_or("put-file: data is not base64")?;
            if off + data.len() as u64 > size {
                return Err(format!("put-file {path}: past its size"));
            }
            if !st.is_dir() {
                return Err(format!("put-file: no put of {dest} began (put-begin)"));
            }
            let file = st.join(&path);
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(off == 0).open(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            f.seek(SeekFrom::Start(off)).and_then(|_| f.write_all(&data)).map_err(|e| format!("{}: {e}", file.display()))?;
            Ok((format!("{{\"off\":{}}}", off + data.len() as u64), None))
        }
        "put-done" => {
            let w: Vec<&str> = rest.split_whitespace().collect();
            let [dest_rel, how, files, bytes] = w[..] else { return Err("usage: put-done <dest> merge|replace <files> <bytes>".into()) };
            let replace = match how {
                "replace" => true,
                "merge" => false,
                _ => return Err(format!("put-done: `{how}` is neither merge nor replace")),
            };
            let (Ok(files), Ok(bytes)) = (files.parse::<usize>(), bytes.parse::<u64>()) else { return Err("put-done: bad numbers".into()) };
            let dest = dest_path(checkout, dest_rel)?;
            let st = staging(&dest);
            if !st.is_dir() {
                return Err(format!("put-done: no put of {dest_rel} began (put-begin)"));
            }
            let staged = files_under(&st)?;
            let got: u64 = staged.iter().map(|(_, _, n)| n).sum();
            if staged.len() != files || got != bytes {
                let _ = std::fs::remove_dir_all(&st);
                return Err(format!("put-done {dest_rel}: {} files of {got} bytes arrived, {files} of {bytes} were sent; nothing put in place", staged.len()));
            }
            let mut kept = None;
            if replace {
                if let Some(dir) = dest.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                if std::fs::symlink_metadata(&dest).is_ok() {
                    let aside = with_suffix(&dest, &format!(".before-put-{}", crate::proto::wall_ms()));
                    std::fs::rename(&dest, &aside).map_err(|e| format!("{} -> {}: {e}", dest.display(), aside.display()))?;
                    kept = Some(aside);
                }
                std::fs::rename(&st, &dest).map_err(|e| format!("{} -> {}: {e}", st.display(), dest.display()))?;
            } else {
                for (rel, from, _) in &staged {
                    let to = dest.join(rel);
                    if let Some(dir) = to.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    }
                    if to.is_dir() {
                        return Err(format!("put-done: {} is a folder; the file of that name stays in {}", to.display(), st.display()));
                    }
                    std::fs::rename(from, &to).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))?;
                }
                let _ = std::fs::remove_dir_all(&st);
            }
            let note = format!(
                "put: {files} files ({:.1} MB) into local/{dest_rel}{}",
                bytes as f64 / 1048576.0,
                kept.as_ref().map_or(String::new(), |k| format!(", what it held kept as {}", k.display()))
            );
            let line = format!(
                "{{\"put\":{},\"files\":{files},\"bytes\":{bytes},\"replaced\":{replace},\"kept\":{}}}",
                json_str(&dest.display().to_string()),
                kept.map_or("null".into(), |k| json_str(&k.display().to_string()))
            );
            Ok((line, Some(note)))
        }
        _ => Err(format!("unknown put word `{word}`: put-begin, put-file, put-done")),
    }
}

/// The node's `put` service ([`handle`]).
pub fn service(conf: Arc<SyncConf>, publish: crate::local::Publish, machine: String) -> Arc<makepad_remote::lines::Handler> {
    Arc::new(move |line: &str, out: &mut Vec<String>| {
        if !crate::lease::exec::writes_allowed() {
            out.push(format!("{{\"err\":{}}}", json_str(&format!("{machine} is not available for dev: a put needs its dev switch on"))));
            return;
        }
        match handle(&conf.checkout, line) {
            Ok((answer, note)) => {
                if let Some(note) = note {
                    publish(hub_record(&machine, "put", 0, "sync", Sev::Info, note, String::new()));
                }
                out.push(answer);
            }
            Err(e) => out.push(format!("{{\"err\":{}}}", json_str(&e))),
        }
    })
}

/// Copies `src` (a folder, or one file) into `<checkout>/local/<dest>` on `node`: its files, its
/// bytes, and where the folder it replaced went.
pub(super) fn put(c: &makepad_remote::link::Client, node: &str, src: &Path, dest: &str, replace: bool) -> Result<(usize, u64, Option<String>), String> {
    use std::io::Read;
    if !super::files::valid_path(dest) {
        return Err(format!("`{dest}` is not a relative path under the node checkout's local/"));
    }
    let files = if src.is_file() {
        let name = src.file_name().ok_or_else(|| format!("{}: no file name", src.display()))?.to_string_lossy().into_owned();
        vec![(name, src.to_path_buf(), std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?.len())]
    } else if src.is_dir() {
        files_under(src)?
    } else {
        return Err(format!("{} is neither a file nor a folder", src.display()));
    };
    let mut pipe = crate::lease::ship::Pipe::open(c, node, "put").map_err(|e| if e.contains("unsupported service") { format!("{e} (its hub is from before `put`: it takes one once it updates)") } else { e })?;
    let mut ok = |_: &str, _: &[String]| Ok(());
    pipe.send(format!("put-begin {dest}"), &mut ok)?;
    let (mut sent, mut buf) = (0u64, vec![0u8; CHUNK]);
    for (rel, path, _) in &files {
        let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
        let pb = makepad_network::digest::base64_encode(rel.as_bytes());
        let mut off = 0u64;
        loop {
            let mut n = 0;
            while n < CHUNK {
                match f.read(&mut buf[n..]).map_err(|e| format!("{}: {e}", path.display()))? {
                    0 => break,
                    k => n += k,
                }
            }
            let n = n.min((size - off) as usize);
            pipe.send(format!("put-file {dest} {pb} {off} {size} {}", makepad_network::digest::base64_encode(&buf[..n])), &mut ok)?;
            off += n as u64;
            if n == 0 || off >= size {
                break;
            }
        }
        if off != size {
            return Err(format!("{}: it changed while it was sent ({off} of {size} bytes)", path.display()));
        }
        sent += size;
    }
    pipe.drain(&mut ok)?;
    let mut done = None;
    let mut last = |_: &str, lines: &[String]| {
        done = lines.first().cloned();
        Ok(())
    };
    pipe.send(format!("put-done {dest} {} {} {sent}", if replace { "replace" } else { "merge" }, files.len()), &mut last)?;
    pipe.drain(&mut last)?;
    let line = done.ok_or_else(|| format!("{node}: put-done gave no answer"))?;
    let kept = makepad_strict_json::parse(line.as_bytes()).ok().and_then(|v| v.get("kept").and_then(|k| k.as_str()).map(str::to_string));
    Ok((files.len(), sent, kept))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A put lands whole: merged file by file into what the folder held, or in place of it with
    /// the old folder kept; a short put, a path out of local/ and a symlinked folder are refused.
    #[test]
    fn a_put_merges_or_replaces_and_refuses_what_is_not_its_own() {
        let checkout = std::env::temp_dir().join(format!("devhub-put-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&checkout);
        let store = checkout.join("local/lib/store");
        std::fs::create_dir_all(store.join("cas")).unwrap();
        std::fs::write(store.join("server-id"), "mine").unwrap();
        std::fs::write(store.join("catalog"), "old").unwrap();
        let b = |s: &str| makepad_network::digest::base64_encode(s.as_bytes());
        let ask = |line: &str| handle(&checkout, line);
        let send = |dest: &str, files: &[(&str, &str)]| {
            ask(&format!("put-begin {dest}")).unwrap();
            for (path, data) in files {
                let (first, rest) = data.split_at(data.len() / 2);
                ask(&format!("put-file {dest} {} 0 {} {}", b(path), data.len(), b(first))).unwrap();
                ask(&format!("put-file {dest} {} {} {} {}", b(path), first.len(), data.len(), b(rest))).unwrap();
            }
        };
        // merge: the named files replaced, the node's own kept
        send("lib/store", &[("catalog", "new catalog"), ("cas/ab", "blob"), ("empty", "")]);
        let (line, note) = ask("put-done lib/store merge 3 15").unwrap();
        assert!(line.contains("\"kept\":null") && note.unwrap().contains("3 files"), "merge answer: {line}");
        assert_eq!(std::fs::read_to_string(store.join("catalog")).unwrap(), "new catalog", "merge replaces a file of the same name");
        assert_eq!(std::fs::read_to_string(store.join("cas/ab")).unwrap(), "blob", "merge writes chunked files whole");
        assert_eq!(std::fs::read_to_string(store.join("server-id")).unwrap(), "mine", "merge leaves the node's other files");
        assert!(store.join("empty").is_file() && !staging(&store).exists(), "an empty file arrives; the staging goes");
        // replace: the copy alone, the old folder beside it
        send("lib/store", &[("catalog", "copy")]);
        let (line, _) = ask("put-done lib/store replace 1 4").unwrap();
        assert!(!store.join("server-id").exists() && std::fs::read_to_string(store.join("catalog")).unwrap() == "copy", "replace leaves only the copy");
        let kept = std::fs::read_dir(checkout.join("local/lib")).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("store.before-put-")).expect("the replaced folder is kept");
        assert!(line.contains(&kept) && checkout.join("local/lib").join(&kept).join("server-id").is_file(), "replace answer names the kept folder: {line}");
        // a put whose files did not all arrive puts nothing in place
        send("lib/store", &[("catalog", "torn")]);
        assert!(ask("put-done lib/store replace 2 10").unwrap_err().contains("nothing put in place"));
        assert_eq!(std::fs::read_to_string(store.join("catalog")).unwrap(), "copy", "a short put changes nothing");
        // out of local/, an absolute path, a symlinked folder
        assert!(ask("put-begin ../src").is_err() && ask("put-begin /etc").is_err());
        ask("put-begin lib/x").unwrap();
        assert!(ask(&format!("put-file lib/x {} 0 1 eA==", b("../evil"))).is_err(), "a file path leaving the staging is refused");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&checkout, checkout.join("local/link")).unwrap();
            assert!(ask("put-begin link/store").unwrap_err().contains("symlink"), "a symlinked folder below local/ is refused");
        }
        std::fs::remove_dir_all(&checkout).unwrap();
    }
}
