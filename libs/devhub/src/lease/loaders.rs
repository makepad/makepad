//! The JIT binaries one node built, by content, so a lease whose tree holds the same inputs again
//! never waits on `cargo build`, however the tree's mtimes moved (one tree, moved per lease: a move
//! touches files, and cargo builds again what it built before). Every box builds its own: a binary
//! never leaves the box it was built on.
//!
//! Dev JIT inputs are the path-package closure's Rust files, Cargo.toml and literal
//! include!/include_str!/include_bytes! targets (include! .rs targets followed transitively).
//! Rust walks exclude hidden entries, target, nested packages and top-level tests/benches/examples.
//! Unreferenced scripts, documentation, licenses and other non-Rust files do not count.
//! package.metadata.commercial.include still adds named files and whole named directories.
//! Cargo.lock, .cargo/config.toml and rust-toolchain{,.toml} count when tracked. Root Cargo.toml
//! counts canonically without workspace members/exclude/default-members or uninherited workspace
//! dependencies, so workspace membership alone cannot rebuild the JIT. Keys hash these paths and
//! blob ids (the normalized manifest's SHA-256 in place of its blob id), plus the compiler.
//!
//! Dev overlays use pinned RELAXGIT blobs and [`super::jitstore`]. A miss builds only the
//! committed pin in `<work>/dev/tree`, with `<work>/dev/target`; edits never enter that tree.
//! A dep-info file outside the declared inputs is noted in the hub log, never an error.
//! A custom JIT self-hosts the bare runner from the overlay through the pinned base JIT.
//!
//! `<tree>/target/host/makepad.jit-<key>` and its `.inputs` keep binaries published by a gate's
//! `loader_build.sh` or a lease's plain `cargo run` of the JIT. They leave [`MARK`]; [`publish`]
//! records Cargo's dep-info and workspace-wide inputs only if the tree still holds that build's
//! sources. [`find`] selects a held binary for those same blobs, without rebuilding for mtimes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What a build leaves in the tree's target to have its binary published.
pub const MARK: &str = "host/makepad.publish";
/// Binaries a box keeps, newest first.
const KEEP: usize = 4;
const PREFIX: &str = "makepad.jit-";

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let o = std::process::Command::new("git").args(args).current_dir(root).stdin(std::process::Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        return Err(format!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The compiler's commit: a rustup update on this box means new binaries.
pub(crate) fn compiler() -> &'static str {
    static COMPILER: OnceLock<String> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let v = super::caps::rust_command("rustc").arg("-vV").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        v.lines().find_map(|l| l.strip_prefix("commit-hash: ")).unwrap_or("").to_string()
    })
}

/// The key of `inputs` with `blob` (path to its git blob); None when one is not tracked.
fn key(compiler: &str, inputs: &[String], blob: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut text = String::with_capacity(inputs.len() * 80);
    text.push_str(compiler);
    for p in inputs {
        text.push('\n');
        text.push_str(p);
        text.push(' ');
        text.push_str(&blob(p)?);
    }
    let h = makepad_network::digest::sha256_hash(text.as_bytes());
    Some(h[..12].iter().map(|b| format!("{b:02x}")).collect())
}

/// `git ls-files -s -z` output: path -> blob.
fn blobs(listing: &str) -> HashMap<String, String> {
    listing
        .split('\0')
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let blob = meta.split(' ').find(|w| w.len() >= 40 && w.bytes().all(|b| b.is_ascii_hexdigit()))?;
            Some((path.to_string(), blob.to_string()))
        })
        .collect()
}

/// `<target>/host/makepad.jit-<key>`.
fn path(target: &Path, key: &str) -> PathBuf {
    target.join("host").join(format!("{PREFIX}{key}"))
}

fn valid_key(k: &str) -> bool {
    k.len() == 24 && k.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The keys this box holds, newest first.
fn keys(target: &Path) -> Vec<String> {
    let mut v: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(target.join("host"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let k = name.strip_prefix(PREFIX)?.strip_suffix(".inputs")?.to_string();
            Some((e.metadata().ok()?.modified().ok()?, k))
        })
        .filter(|(_, k)| valid_key(k))
        .collect();
    v.sort();
    v.into_iter().rev().map(|(_, k)| k).collect()
}

fn read_inputs(target: &Path, key: &str) -> Option<Vec<String>> {
    let p = path(target, key).with_file_name(format!("{PREFIX}{key}.inputs"));
    Some(std::fs::read_to_string(p).ok()?.lines().map(str::to_string).collect())
}

/// The binary built from exactly what the tree holds now, if this box has it.
pub fn find(root: &Path, target: &Path) -> Option<(String, PathBuf)> {
    let held = keys(target);
    if held.is_empty() {
        return None;
    }
    let index = blobs(&git(root, &["ls-files", "-s", "-z"]).ok()?);
    let dirty: HashSet<String> = git(root, &["diff", "--name-only"]).ok()?.lines().map(str::to_string).collect();
    held.into_iter().find_map(|k| {
        let inputs = read_inputs(target, &k)?;
        let p = path(target, &k);
        (!inputs.iter().any(|i| dirty.contains(i)) && p.is_file() && key(compiler(), &inputs, &|f| index.get(f).cloned()).as_deref() == Some(k.as_str())).then_some((k, p))
    })
}

/// Pinned manifests define the JIT's path-package closure, including added and deleted files.
/// Reading blobs makes the key independent of the dev tree's current commit and mtimes.
#[cfg(feature = "checkout")]
pub(crate) struct Inputs {
    pub(crate) files: Vec<String>,
    crate_dirs: std::collections::BTreeSet<String>,
    extra_dirs: std::collections::BTreeSet<String>,
    root_manifest: String,
}

#[cfg(feature = "checkout")]
impl Inputs {
    // Every dev JIT entry point substitutes the same normalized root manifest here.
    fn key(&self, compiler: &str, blob: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        key(compiler, &self.files, &|p| if p == "Cargo.toml" { Some(self.root_manifest.clone()) } else { blob(p) })
    }
}

#[cfg(feature = "checkout")]
fn normal(path: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for p in path.split('/') {
        match p { "" | "." => {}, ".." => { if parts.pop().is_none() { return Err(format!("JIT input outside the repo: {path}")); } }, p => parts.push(p) }
    }
    Ok(parts.join("/"))
}

/// The same package boundary and exclusions for committed files and new overlay inputs.
#[cfg(feature = "checkout")]
fn crate_input(path: &str, dirs: &std::collections::BTreeSet<String>, contains: &dyn Fn(&str) -> bool) -> bool {
    if !path.ends_with(".rs") && path.rsplit('/').next() != Some("Cargo.toml") { return false; }
    let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    loop {
        let manifest = if dir.is_empty() { "Cargo.toml".into() } else { format!("{dir}/Cargo.toml") };
        if contains(&manifest) {
            if !dirs.contains(dir) { return false; }
            let rel = if dir.is_empty() { path } else { &path[dir.len() + 1..] };
            return (rel == "Cargo.toml" || rel.ends_with(".rs"))
                && !rel.split('/').any(|p| p.starts_with('.') || p == "target")
                && !["tests/", "benches/", "examples/"].iter().any(|d| rel.starts_with(d));
        }
        if dir.is_empty() { return false; }
        dir = dir.rsplit_once('/').map_or("", |(dir, _)| dir);
    }
}

// Blobs are immutable. Hold the lock only for lookup/insertion, never while reading or lexing.
// Edited tree files have an empty oid and must not read or populate this process-wide cache.
#[cfg(feature = "checkout")]
fn blob_includes(path: &str, oid: &str, bytes: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Vec<(String, bool)>, String> {
    static INCLUDES: OnceLock<std::sync::Mutex<HashMap<String, Vec<(String, bool)>>>> = OnceLock::new();
    let cache = INCLUDES.get_or_init(Default::default);
    if !oid.is_empty() {
        if let Some(list) = cache.lock().unwrap().get(oid) { return Ok(list.clone()); }
    }
    let bytes = bytes(path)?;
    let list = if std::str::from_utf8(&bytes).is_ok_and(|s| !s.contains("include")) { Vec::new() }
        else { relax_base::srchash::includes(&String::from_utf8_lossy(&bytes)) };
    if !oid.is_empty() { cache.lock().unwrap().insert(oid.into(), list.clone()); }
    Ok(list)
}

#[cfg(feature = "checkout")]
fn include_targets(mut pending: Vec<String>, files: &mut std::collections::BTreeSet<String>, blob: &dyn Fn(&str) -> Option<String>, bytes: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<(), String> {
    let mut scanned = HashSet::new();
    while let Some(path) = pending.pop() {
        if !scanned.insert(path.clone()) { continue; }
        let Some(oid) = blob(&path) else { continue };
        for (name, recurse) in blob_includes(&path, &oid, bytes)? {
            if Path::new(&name).is_absolute() { continue; }
            let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let Ok(target) = normal(&format!("{dir}/{name}")) else { continue };
            if blob(&target).is_none() { continue; }
            if recurse && target.ends_with(".rs") { pending.push(target.clone()); }
            files.insert(target);
        }
    }
    Ok(())
}

/// A canonical value rendering for hashing: sorted table keys and scalars without source spans.
#[cfg(feature = "checkout")]
fn canonical_toml(value: &makepad_toml_parser::Toml, out: &mut String) {
    use makepad_toml_parser::{Toml, TomlTable};
    use std::fmt::Write;
    fn table(t: &TomlTable, out: &mut String) {
        out.push('{');
        for (key, value) in t {
            write!(out, "{key:?}=").unwrap();
            canonical_toml(value, out);
            out.push(',');
        }
        out.push('}');
    }
    match value {
        Toml::Str(v, _) => write!(out, "{v:?}").unwrap(),
        Toml::Bool(v, _) => write!(out, "{v}").unwrap(),
        Toml::Num(v, _) => write!(out, "{v}").unwrap(),
        Toml::Date(v, _) => out.push_str(v),
        Toml::Table(t) => table(t, out),
        Toml::Array(v) => { out.push('['); for value in v { canonical_toml(value, out); out.push(','); } out.push(']'); }
        Toml::ArrayOfTables(v) => { out.push('['); for value in v { table(value, out); out.push(','); } out.push(']'); }
    }
}

#[cfg(feature = "checkout")]
fn pinned_inputs(work: &Path, pin: &super::Pin, index: &std::collections::BTreeMap<String, String>) -> Result<Inputs, String> {
    let blobs = super::relaxgit::Blobs::open(work, pin)?;
    inputs(index, &|path| {
        let oid = index.get(path).ok_or_else(|| format!("JIT manifest missing from pin: {path}"))?;
        blobs.read(path, oid)
    })
}

#[cfg(feature = "checkout")]
pub(crate) fn inputs(index: &std::collections::BTreeMap<String, String>, bytes: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Inputs, String> {
    use makepad_toml_parser::{parse_toml, Toml};
    use std::collections::BTreeSet;
    let read = |path: &str| -> Result<_, String> {
        let bytes = bytes(path)?;
        parse_toml(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?).map_err(|e| format!("{path}: {e}"))
    };
    let mut root = read("Cargo.toml")?;
    let empty = Default::default();
    let workspace = root.get_path(&["workspace", "dependencies"]).and_then(Toml::as_table).unwrap_or(&empty);
    let patches = root.get_path(&["patch", "crates-io"]).and_then(Toml::as_table).unwrap_or(&empty);
    let mut dirs = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut extra_dirs = BTreeSet::new();
    let mut inherited_names = BTreeSet::new();
    let mut todo = vec!["tools/run".to_string()];
    while let Some(dir) = todo.pop() {
        if !dirs.insert(dir.clone()) { continue; }
        let manifest = format!("{dir}/Cargo.toml");
        let doc = read(&manifest)?;
        let mut tables = vec![&doc.root];
        if let Some(targets) = doc.root.get("target").and_then(Toml::as_table) { tables.extend(targets.values().filter_map(Toml::as_table)); }
        for table in tables {
            for kind in ["dependencies", "build-dependencies", "dev-dependencies"] {
                let Some(deps) = table.get(kind).and_then(Toml::as_table) else { continue };
                for (name, dep) in deps {
                    let inherited = dep.as_table().and_then(|t| t.get("workspace")).and_then(Toml::as_bool) == Some(true);
                    if inherited { inherited_names.insert(name.clone()); }
                    if kind == "dev-dependencies" { continue; }
                    let dep = if inherited { workspace.get(name).ok_or_else(|| format!("{manifest}: missing workspace dependency {name}"))? } else { dep };
                    let path = dep.as_table().and_then(|t| t.get("path")).and_then(Toml::as_str);
                    if let Some(p) = path {
                        todo.push(normal(&if inherited { p.to_string() } else { format!("{dir}/{p}") })?);
                    } else {
                        let package = dep.as_table().and_then(|t| t.get("package")).and_then(Toml::as_str).unwrap_or(name);
                        if let Some(p) = patches.get(package).and_then(Toml::as_table).and_then(|t| t.get("path")).and_then(Toml::as_str) { todo.push(normal(p)?); }
                    }
                }
            }
        }
        if let Some(extra) = doc.get_path(&["package", "metadata", "commercial", "include"]).and_then(Toml::as_array) {
            for p in extra.iter().filter_map(Toml::as_str) {
                let p = normal(&format!("{dir}/{p}"))?;
                if index.contains_key(&p) { files.insert(p); }
                else {
                    let prefix = format!("{p}/");
                    let mut found = false;
                    for (path, _) in index.range(prefix.clone()..).take_while(|(path, _)| path.starts_with(&prefix)) { files.insert(path.clone()); found = true; }
                    if !found { return Err(format!("JIT input missing from pin: {p}")); }
                    extra_dirs.insert(p);
                }
            }
        }
    }
    let mut pending = Vec::new();
    for p in index.keys() {
        if crate_input(p, &dirs, &|p| index.contains_key(p)) {
            files.insert(p.clone());
            if p.ends_with(".rs") { pending.push(p.clone()); }
        }
    }
    include_targets(pending, &mut files, &|p| index.get(p).cloned(), bytes)?;
    for p in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml"] {
        if index.contains_key(p) { files.insert(p.into()); }
    }
    if let Some(Toml::Table(workspace)) = root.root.get_mut("workspace") {
        for key in ["members", "exclude", "default-members"] { workspace.remove(key); }
        if let Some(Toml::Table(deps)) = workspace.get_mut("dependencies") { deps.retain(|name, _| inherited_names.contains(name)); }
    }
    let mut rendered = String::new();
    canonical_toml(&Toml::Table(root.root), &mut rendered);
    let root_manifest = makepad_network::digest::sha256_hash(rendered.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Inputs { files: files.into_iter().collect(), crate_dirs: dirs, extra_dirs, root_manifest })
}

/// The dev JIT ids a pinned makepad commit selects, in compiler order.
#[cfg(feature = "checkout")]
pub(crate) fn commit_ids(repo: &Path, sha: &str, compilers: &[String]) -> Result<Vec<String>, String> {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    if compilers.iter().all(|c| c.is_empty()) { return Ok(Vec::new()); }
    let out = Command::new("git").arg("-C").arg(repo).args(["ls-tree", "-r", "-z", "--full-tree", sha])
        .stdin(Stdio::null()).output().map_err(|e| format!("git ls-tree: {e}"))?;
    if !out.status.success() { return Err(format!("git ls-tree: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    let listing = String::from_utf8(out.stdout).map_err(|e| e.to_string())?;
    let index: BTreeMap<String, String> = listing.split('\0').filter_map(|e| {
        let (meta, path) = e.split_once('\t')?;
        let mut words = meta.split_whitespace();
        words.next()?;
        if words.next()? != "blob" { return None; }
        Some((path.to_string(), words.next()?.to_string()))
    }).collect();
    let mut child = Command::new("git").arg("-C").arg(repo).args(["cat-file", "--batch"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("git cat-file: {e}"))?;
    let result = (|| -> Result<Vec<String>, String> {
        let input = child.stdin.take().ok_or("git cat-file: no stdin")?;
        let output = BufReader::new(child.stdout.take().ok_or("git cat-file: no stdout")?);
        let batch = RefCell::new((input, output));
        let bytes = |path: &str| -> Result<Vec<u8>, String> {
            let oid = index.get(path).ok_or_else(|| format!("JIT manifest missing from pin: {path}"))?;
            let (input, output) = &mut *batch.borrow_mut();
            let io = |e| format!("git cat-file: {e}");
            writeln!(input, "{oid}").map_err(io)?;
            input.flush().map_err(io)?;
            let mut head = String::new();
            output.read_line(&mut head).map_err(io)?;
            let words: Vec<_> = head.split_whitespace().collect();
            if words.len() != 3 || words[0] != oid.as_str() || words[1] != "blob" { return Err(format!("git cat-file: {}", head.trim())); }
            let size = words[2].parse::<usize>().map_err(|e| e.to_string())?;
            let mut bytes = vec![0; size];
            output.read_exact(&mut bytes).map_err(io)?;
            let mut end = [0];
            output.read_exact(&mut end).map_err(io)?;
            if end != [b'\n'] { return Err("git cat-file: missing blob terminator".into()); }
            Ok(bytes)
        };
        let inputs = inputs(&index, &bytes)?;
        compilers.iter().filter(|c| !c.is_empty()).map(|c| inputs.key(c, &|p| index.get(p).cloned()).ok_or_else(|| "JIT input missing from pin".to_string())).collect()
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// The native tree's current content key in the dev JIT store. Use the same manifest closure
/// as overlays, with staged, unstaged and new inputs accounted for before accepting a hit.
#[cfg(feature = "checkout")]
pub fn tree_id(root: &Path) -> Option<String> {
    let mut index = std::collections::BTreeMap::new();
    let mut changed = HashSet::new();
    let commercial = root.join("apps/commercial/.git").exists();
    for (dir, prefix) in std::iter::once((root.to_path_buf(), "")).chain(commercial.then(|| (root.join("apps/commercial"), "apps/commercial/"))) {
        let mut files = blobs(&git(&dir, &["ls-files", "-s", "-z"]).ok()?);
        let dirty = git(&dir, &["diff", "--name-only", "-z"]).ok()?;
        let added = git(&dir, &["ls-files", "--others", "--exclude-standard", "-z"]).ok()?;
        for p in dirty.split('\0').chain(added.split('\0')).filter(|p| !p.is_empty()) {
            let path = dir.join(p);
            if !path.exists() && std::fs::symlink_metadata(&path).is_err() { files.remove(p); continue; }
            files.insert(p.into(), String::new());
            changed.insert(format!("{prefix}{p}"));
        }
        index.extend(files.into_iter().filter(|(p, _)| !prefix.is_empty() || !commercial || !p.starts_with("apps/commercial/")).map(|(p, oid)| (format!("{prefix}{p}"), oid)));
    }
    let inputs = inputs(&index, &|p| std::fs::read(root.join(p)).map_err(|e| e.to_string())).ok()?;
    if compiler().is_empty() { return None; }
    inputs.key(compiler(), &|p| {
        if !changed.contains(p) { return index.get(p).cloned(); }
        let path = root.join(p);
        let bytes = if std::fs::symlink_metadata(&path).ok()?.file_type().is_symlink() { std::fs::read_link(&path).ok()?.to_string_lossy().as_bytes().to_vec() } else { std::fs::read(&path).ok()? };
        let mut object = format!("blob {}\0", bytes.len()).into_bytes();
        object.extend_from_slice(&bytes);
        Some(relax_base::gitstore::hex(&relax_base::gitstore::sha1(&object)))
    })
}

/// The lane pin's base JIT key and its inputs, from RELAXGIT only.
#[cfg(feature = "checkout")]
pub fn overlay_id(work: &Path, pin: &super::Pin) -> Result<(String, Vec<String>), String> {
    let index = super::relaxgit::bases(work, pin)?;
    let inputs = pinned_inputs(work, pin, &index)?;
    if compiler().is_empty() { return Err("dev JIT: rustc did not report its compiler commit".into()); }
    let id = inputs.key(compiler(), &|p| index.get(p).cloned()).ok_or("overlay: missing JIT input blob")?;
    Ok((id, inputs.files))
}

#[cfg(feature = "checkout")]
pub struct Overlay {
    pub id: String,
    pub binary: PathBuf,
    pub changed: Vec<String>,
}

/// The store owns Cargo's process and log. Its active check counts every waiting lease;
/// the requester's check only cancels its own resolution and wait for the per-id lock.
#[cfg(feature = "checkout")]
pub struct OverlayBuild<'a> {
    /// Cargo parallelism, independent of a prebuild lease's small reservation.
    pub jobs: u32,
    /// The store build runs at nice 19 (a Unix prebuild).
    pub niced: bool,
    pub run: &'a dyn Fn(&str, &std::process::Command, &dyn Fn() -> Result<(), String>) -> Result<(std::process::ExitStatus, PathBuf), String>,
    pub cancelled: &'a std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub say: &'a dyn Fn(&str),
    pub pending: &'a dyn Fn(&str),
    pub active: &'a dyn Fn() -> Result<(), String>,
}

/// Use the explicit --jit loader id: build it from the pin when it matches, otherwise require
/// a held binary. Without one, use the pin's dev JIT.
#[cfg(feature = "checkout")]
pub fn find_overlay(work: &Path, lane: &str, lease: &str, pin: &super::Pin, requested: Option<&str>, command: &str, build: &OverlayBuild<'_>) -> Result<Overlay, String> {
    (build.active)()?;
    let store = super::jitstore::Store::new(work);
    let index = super::relaxgit::bases(work, pin)?;
    let inputs = pinned_inputs(work, pin, &index)?;
    if compiler().is_empty() { return Err("dev JIT: rustc did not report its compiler commit".into()); }
    let id = inputs.key(compiler(), &|p| index.get(p).cloned()).ok_or("overlay: missing JIT input blob")?;
    let edits = super::relaxgit::overlay(work, lane);
    let deleted: HashSet<String> = std::fs::read_to_string(edits.join(".makepad-run-deleted")).unwrap_or_default().lines().map(str::to_string).collect();
    let records = std::fs::read_to_string(edits.join(".makepad-run-edits")).map_err(|e| format!("overlay: edit records: {e}"))?;
    let records: std::collections::BTreeSet<&str> = records.lines().filter_map(|l| l.split_once('\t').map(|(p, _)| p)).collect();
    let present = |p: &str| !deleted.contains(p) && (records.contains(p) || index.contains_key(p));
    let mut watched: std::collections::BTreeSet<String> = inputs.files.iter().cloned().collect();
    let mut pending = Vec::new();
    for &p in &records {
        let input = watched.contains(p) || crate_input(p, &inputs.crate_dirs, &present)
            || inputs.extra_dirs.iter().any(|dir| p.starts_with(&format!("{dir}/")));
        if !input { continue; }
        watched.insert(p.to_string());
        if p.ends_with(".rs") && !deleted.contains(p) { pending.push(p.to_string()); }
    }
    if !pending.is_empty() {
        let blobs = super::relaxgit::Blobs::open(work, pin)?;
        include_targets(pending, &mut watched, &|p| {
            if !present(p) { None } else if records.contains(p) { Some(String::new()) } else { index.get(p).cloned() }
        }, &|p| {
            if records.contains(p) { std::fs::read(edits.join(p)).map_err(|e| format!("overlay: {p}: {e}")) }
            else { blobs.read(p, index.get(p).ok_or_else(|| format!("overlay: missing include {p}"))?) }
        })?;
    }
    let mut changed = Vec::new();
    for &p in &records {
        if !watched.contains(p) { continue; }
        if deleted.contains(p) { if index.contains_key(p) { changed.push(p.to_string()); } continue; }
        let file = edits.join(p);
        let m = std::fs::symlink_metadata(&file).map_err(|e| format!("overlay: {p}: {e}"))?;
        let bytes = if m.file_type().is_symlink() { std::fs::read_link(&file).map(|p| p.to_string_lossy().as_bytes().to_vec()) } else { std::fs::read(&file) }.map_err(|e| format!("overlay: {p}: {e}"))?;
        let mut object = format!("blob {}\0", bytes.len()).into_bytes();
        object.extend_from_slice(&bytes);
        if index.get(p) != Some(&relax_base::gitstore::hex(&relax_base::gitstore::sha1(&object))) { changed.push(p.to_string()); }
    }
    changed.sort();
    changed.dedup();
    if !changed.is_empty() {
        let paths = changed.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if !matches!(command, "test" | "check") { return Err(format!("overlay: a custom JIT for app runs is not supported yet ({paths})")); }
        (build.say)(&format!("devhub: custom JIT (self-hosted) for {paths}"));
    }
    let selected = requested.unwrap_or(&id);
    (build.active)()?;
    let binary = if selected == id {
        store.ensure(&id, lease, build.say, build.pending, build.active, build.cancelled, |dst, active| build_dev(work, pin, &inputs.files, &id, dst, build, active))?
    } else {
        store.hold(selected, lease)?;
        store.lookup(selected).ok_or_else(|| format!("overlay: dev JIT {selected} is not on this box; held keys: {}", store.snapshot().ids.join(", ")))?
    };
    (build.active)()?;
    Ok(Overlay { id: selected.into(), binary, changed })
}

/// Only committed states enter this checkout. The tree lock covers moving, building and publishing.
#[cfg(feature = "checkout")]
fn build_dev(work: &Path, pin: &super::Pin, inputs: &[String], id: &str, dst: &Path, build: &OverlayBuild<'_>, active: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
    use crate::sync::git::{run, Store};
    let _tree = super::jitstore::lock_while(&work.join("dev/tree.lock"), false, active, &|| {})?;
    active()?;
    let root = work.join("dev/tree");
    let target = work.join("dev/target");
    let commercial = root.join("apps/commercial");
    if pin.commercial.is_none() && commercial.join(".git").exists() {
        let store = Store::open("commercial")?;
        if store.admin_of(&commercial).is_none() { return Err("dev JIT: commercial tree is not owned by the bare store".into()); }
        run(None, Some(&store.git_dir), &[], &["worktree", "remove", &commercial.to_string_lossy()])?;
    }
    for (repo, sha, dir) in std::iter::once(("makepad", pin.makepad.as_str(), root.clone())).chain(pin.commercial.as_deref().map(|s| ("commercial", s, commercial))) {
        let store = Store::open(repo)?;
        if dir.join(".git").exists() {
            if store.admin_of(&dir).is_none() { return Err(format!("dev JIT: {} is not owned by the bare store", dir.display())); }
            let from = run(Some(&dir), None, &[], &["rev-parse", "HEAD"])?;
            if !run(Some(&dir), None, &[], &["diff", "--name-only", "HEAD"])?.trim().is_empty() { return Err(format!("dev JIT: {} has uncommitted files", dir.display())); }
            let lockfile = dir.join("Cargo.lock");
            if lockfile.exists() && run(Some(&dir), None, &[], &["ls-files", "--", "Cargo.lock"])?.trim().is_empty() { std::fs::remove_file(lockfile).map_err(|e| e.to_string())?; }
            if from.trim() != sha {
                run(Some(&dir), None, &[], &["read-tree", "-m", "-u", from.trim(), sha])?;
                run(Some(&dir), None, &[], &["update-ref", "--no-deref", "HEAD", sha])?;
            }
        } else {
            std::fs::create_dir_all(dir.parent().ok_or("dev tree parent")?).map_err(|e| e.to_string())?;
            run(None, Some(&store.git_dir), &[], &["worktree", "add", "--detach", &dir.to_string_lossy(), sha])?;
        }
        if !run(Some(&dir), None, &[], &["diff", "--name-only", "HEAD"])?.trim().is_empty() { return Err(format!("dev JIT: {} has uncommitted files", dir.display())); }
    }
    // The lockfile is generated locally when it is not tracked; never carry another pin's resolution.
    if !inputs.iter().any(|p| p == "Cargo.lock") && root.join("Cargo.lock").exists() { std::fs::remove_file(root.join("Cargo.lock")).map_err(|e| e.to_string())?; }
    let mut cmd = if build.niced {
        let mut cmd = super::caps::rust_command("nice");
        cmd.args(["-n", "19", "cargo"]);
        cmd
    } else { super::caps::rust_command("cargo") };
    cmd.args(["build", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "-j", &build.jobs.max(1).to_string()])
        .current_dir(&root).env("CARGO_TARGET_DIR", &target).env("CARGO_INCREMENTAL", "0").env("CARGO_NET_OFFLINE", "true")
        .env_remove("MAKEPAD_RUN_VFS").env_remove("MAKEPAD_RUN_EDITS").env_remove("MAKEPAD_RUN_WORKSPACE");
    (build.say)(&format!("devhub: building the pinned dev JIT at {}", pin.short()));
    let (status, log) = (build.run)(id, &cmd, active)?;
    if !status.success() {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let tail = text.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(format!("dev JIT cargo failed ({status}):\n{tail}"));
    }
    let dep = std::fs::read_to_string(target.join("host/makepad.d")).map_err(|e| format!("makepad.d: {e}"))?;
    let index = blobs(&git(&root, &["ls-files", "-s", "-z"])?);
    let outside: Vec<_> = inputs_of(&dep, &root, &index).into_iter().filter(|p| !inputs.contains(p)).collect();
    if !outside.is_empty() {
        eprintln!("devhub: dev JIT {id} at {}: dep-info names {} file(s) outside its inputs (not an error): {}{}", pin.short(), outside.len(), outside[..outside.len().min(5)].join(", "), if outside.len() > 5 { " ..." } else { "" });
    }
    let binary = target.join("host/makepad");
    let built = std::fs::metadata(&binary).and_then(|m| m.modified()).map_err(|e| e.to_string())?;
    if let Some(p) = inputs.iter().find(|p| std::fs::metadata(root.join(p)).and_then(|m| m.modified()).map_or(true, |t| t > built)) { return Err(format!("dev JIT: {p} changed after the build")); }
    publish_binary(&binary, dst, &|staged, dst| {
        active()?;
        std::fs::rename(staged, dst).map_err(|e| format!("{}: {e}", dst.display()))
    })
}

/// Publish by rename; Cargo can replace its output without changing a held binary.
#[cfg(feature = "checkout")]
fn publish_binary(binary: &Path, dst: &Path, publish: &dyn Fn(&Path, &Path) -> Result<(), String>) -> Result<(), String> {
    let tmp = dst.with_file_name(format!("{}.part", dst.file_name().unwrap().to_string_lossy()));
    let _ = std::fs::remove_file(&tmp);
    let result = std::fs::hard_link(binary, &tmp).or_else(|_| std::fs::copy(binary, &tmp).map(|_| ()))
        .map_err(|e| format!("{}: {e}", dst.display())).and_then(|_| publish(&tmp, dst));
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

/// A `cargo run --profile host ... -p makepad-run --bin makepad -- <args>` argv: its `<args>`.
pub fn cargo_run_args(argv: &[String]) -> Option<&[String]> {
    let sep = argv.iter().position(|a| a == "--")?;
    let flags = &argv[..sep];
    if flags.first().map(String::as_str) != Some("cargo") || flags.get(1).map(String::as_str) != Some("run") {
        return None;
    }
    let mut want = ["--profile", "host", "-p", "makepad-run", "--bin", "makepad"].iter();
    let mut i = 2;
    let mut next = want.next();
    while i < flags.len() {
        match (flags[i].as_str(), next) {
            (f, Some(w)) if f == *w => next = want.next(),
            ("-q" | "--quiet", _) => {}
            _ => return None,
        }
        i += 1;
    }
    next.is_none().then(|| &argv[sep + 1..])
}

/// What cargo's dep-info names under the tree: tracked files only (it also lists folders, and what
/// is outside the tree comes with the compiler or Cargo.lock), plus the workspace-wide files.
fn inputs_of(depinfo: &str, root: &Path, index: &HashMap<String, String>) -> Vec<String> {
    let list = depinfo.split_once(": ").map_or("", |(_, l)| l).replace("\\ ", "\0");
    let mut out: Vec<String> = list
        .split_whitespace()
        .map(|p| p.replace('\0', " "))
        .filter_map(|p| match Path::new(&p).strip_prefix(root) {
            Ok(r) => Some(r.display().to_string()),
            Err(_) => (!p.starts_with('/')).then_some(p),
        })
        .chain(["Cargo.toml", "Cargo.lock", ".cargo/config.toml"].iter().map(|s| s.to_string()))
        .filter(|p| index.contains_key(p))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Publishes the binary a run built, when it left [`MARK`]. Ok(Some(key)): published; Ok(None): no
/// mark; Err: why it is not shared (the tree no longer holds what it was built from).
pub fn publish(root: &Path, target: &Path) -> Result<Option<String>, String> {
    let mark = target.join(MARK);
    if !mark.exists() {
        return Ok(None);
    }
    let _ = std::fs::remove_file(&mark);
    let bin = target.join("host/makepad");
    let built = std::fs::metadata(&bin).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", bin.display()))?;
    let dep = std::fs::read_to_string(target.join("host/makepad.d")).map_err(|e| format!("makepad.d: {e}"))?;
    let index = blobs(&git(root, &["ls-files", "-s", "-z"])?);
    let dirty: HashSet<String> = git(root, &["diff", "--name-only"])?.lines().map(str::to_string).collect();
    let inputs = inputs_of(&dep, root, &index);
    if inputs.len() < 100 {
        return Err(format!("makepad.d names only {} tracked files", inputs.len()));
    }
    if let Some(p) = inputs.iter().find(|p| dirty.contains(*p)) {
        return Err(format!("{p} is edited"));
    }
    // cargo's own rule: an input newer than the binary means it was not built from this
    if let Some(p) = inputs.iter().find(|p| std::fs::metadata(root.join(p)).and_then(|m| m.modified()).map_or(true, |t| t > built)) {
        return Err(format!("{p} changed after the build"));
    }
    let k = key(compiler(), &inputs, &|f| index.get(f).cloned()).ok_or("an input is not tracked")?;
    let dst = path(target, &k);
    let _ = std::fs::remove_file(&dst);
    // a link: cargo replaces target/host/makepad by a new file, never writes into it
    std::fs::hard_link(&bin, &dst).or_else(|_| std::fs::copy(&bin, &dst).map(|_| ())).map_err(|e| format!("{}: {e}", dst.display()))?;
    write_inputs(target, &k, &(inputs.join("\n") + "\n"))?;
    prune(target);
    Ok(Some(k))
}

fn write_inputs(target: &Path, key: &str, text: &str) -> Result<(), String> {
    let p = path(target, key).with_file_name(format!("{PREFIX}{key}.inputs"));
    let tmp = p.with_extension("part");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))
}

fn prune(target: &Path) {
    for k in keys(target).into_iter().skip(KEEP) {
        let p = path(target, &k);
        let _ = std::fs::remove_file(p.with_file_name(format!("{PREFIX}{k}.inputs")));
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn cargo_run_of_the_jit_is_recognized_and_nothing_else() {
        let plain = words("cargo run --profile host -p makepad-run --bin makepad -- check x");
        assert_eq!(cargo_run_args(&plain).unwrap(), &words("check x")[..]);
        let quiet = words("cargo run --profile host -q -p makepad-run --bin makepad -- test y");
        assert_eq!(cargo_run_args(&quiet).unwrap(), &words("test y")[..]);
        for no in ["cargo run --release -p makepad-run --bin makepad -- x", "cargo run --profile host -p makepad-app -- x", "cargo run --profile host -p makepad-run --bin makepad --features a -- x", "bash x -- y", "cargo run --profile host -p makepad-run --bin makepad"] {
            assert!(cargo_run_args(&words(no)).is_none(), "{no}");
        }
    }

    #[test]
    fn the_key_follows_every_input_blob_and_the_compiler() {
        let inputs = words("a b");
        let map: HashMap<String, String> = [("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())].into();
        let k = key("c", &inputs, &|p| map.get(p).cloned()).unwrap();
        assert!(valid_key(&k));
        assert_eq!(key("c", &inputs, &|p| map.get(p).cloned()).unwrap(), k);
        assert_ne!(key("d", &inputs, &|p| map.get(p).cloned()).unwrap(), k);
        let other: HashMap<String, String> = [("a".to_string(), "1".to_string()), ("b".to_string(), "3".to_string())].into();
        assert_ne!(key("c", &inputs, &|p| other.get(p).cloned()).unwrap(), k);
        assert!(key("c", &words("a z"), &|p| map.get(p).cloned()).is_none());
    }

    #[test]
    fn listings_and_depinfo_parse() {
        let idx = blobs(&format!("100644 {} 0\ta/b c\0", "f".repeat(40)));
        assert_eq!(idx.get("a/b c").unwrap(), &"f".repeat(40));
        let index: HashMap<String, String> = ["a/b c", "Cargo.lock", "tools/x.rs"].iter().map(|p| (p.to_string(), "1".to_string())).collect();
        let dep = "/r/target/host/makepad: /r/a/b\\ c /r/libs/dir tools/x.rs /other/z.rs\n";
        assert_eq!(inputs_of(dep, Path::new("/r"), &index), vec!["Cargo.lock", "a/b c", "tools/x.rs"]);
    }

    /// A binary is found for the tree it was built from and for no edit of it; publishing refuses a
    /// tree that moved on after the build.
    #[test]
    fn publish_then_find_follows_the_tree() {
        let d = std::env::temp_dir().join(format!("devhub-loaders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (root, target) = (d.join("r"), d.join("r/target"));
        std::fs::create_dir_all(target.join("host")).unwrap();
        let g = |a: &[&str]| assert!(std::process::Command::new("git").args(a).current_dir(&root).output().unwrap().status.success(), "{a:?}");
        g(&["init", "-q"]);
        let mut dep = format!("{}/host/makepad:", target.display());
        for i in 0..120 {
            std::fs::write(root.join(format!("f{i}.rs")), format!("{i}")).unwrap();
            dep.push_str(&format!(" {}/f{i}.rs", root.display()));
        }
        std::fs::write(root.join(".gitignore"), "target\n").unwrap();
        g(&["add", "-A"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(target.join("host/makepad"), "bin").unwrap();
        std::fs::write(target.join("host/makepad.d"), &dep).unwrap();
        assert_eq!(publish(&root, &target).unwrap(), None, "no mark");
        std::fs::write(target.join(MARK), "").unwrap();
        let k = publish(&root, &target).unwrap().unwrap();
        assert!(!target.join(MARK).exists());
        assert_eq!(find(&root, &target).unwrap().0, k);
        // an edited input: no binary for this tree
        std::fs::write(root.join("f3.rs"), "edit").unwrap();
        assert!(find(&root, &target).is_none());
        // and this build is not published
        std::fs::write(target.join(MARK), "").unwrap();
        assert!(publish(&root, &target).unwrap_err().contains("f3.rs"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
