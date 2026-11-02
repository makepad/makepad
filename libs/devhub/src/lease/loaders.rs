//! The JIT binaries one node built, by content, so a lease whose tree holds the same inputs again
//! never waits on `cargo build`, however the tree's mtimes moved (one tree, moved per lease: a move
//! touches files, and cargo builds again what it built before). Every box builds its own: a binary
//! never leaves the box it was built on.
//!
//! `<tree>/target/host/makepad.jit-<key>` is the binary; `.inputs` beside it names what it was built
//! from: the files of cargo's own dep-info for it (`target/host/makepad.d`), what cargo itself would
//! look at, plus the three workspace-wide files. The key is the hash of those paths' git blobs and
//! the compiler: a binary is valid exactly for trees with the same blobs, so it can't be stale. Its
//! loader id comes from the binary, as for any other build.
//!
//! - A gate's `loader_build.sh` leaves [`MARK`], and so does a lease's plain `cargo run` of the JIT;
//!   at the run's end the hub [`publish`]es the binary into this store, only when the tree still
//!   holds what it was built from.
//! - [`find`]: the binary for the tree as it stands (a tracked input edited means none).
//! - Dev overlays use pinned RELAXGIT blobs and [`super::jitstore`]. A miss builds only the
//!   committed pin in `<work>/dev/tree`, with `<work>/dev/target`; edits never enter that tree.
//!   A custom JIT self-hosts the bare runner from the overlay through the pinned base JIT.

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
fn compiler() -> &'static str {
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
struct Inputs {
    files: Vec<String>,
    dirs: std::collections::BTreeSet<String>,
}

#[cfg(feature = "checkout")]
fn normal(path: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for p in path.split('/') {
        match p { "" | "." => {}, ".." => { if parts.pop().is_none() { return Err(format!("JIT input outside the repo: {path}")); } }, p => parts.push(p) }
    }
    Ok(parts.join("/"))
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
fn inputs(index: &std::collections::BTreeMap<String, String>, bytes: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Inputs, String> {
    use makepad_toml_parser::{parse_toml, Toml};
    use std::collections::BTreeSet;
    let read = |path: &str| -> Result<_, String> {
        let bytes = bytes(path)?;
        parse_toml(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?).map_err(|e| format!("{path}: {e}"))
    };
    let root = read("Cargo.toml")?;
    let empty = Default::default();
    let workspace = root.get_path(&["workspace", "dependencies"]).and_then(Toml::as_table).unwrap_or(&empty);
    let patches = root.get_path(&["patch", "crates-io"]).and_then(Toml::as_table).unwrap_or(&empty);
    let mut dirs = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut extra_dirs = BTreeSet::new();
    let mut todo = vec!["tools/run".to_string()];
    while let Some(dir) = todo.pop() {
        if !dirs.insert(dir.clone()) { continue; }
        let manifest = format!("{dir}/Cargo.toml");
        let doc = read(&manifest)?;
        let mut tables = vec![&doc.root];
        if let Some(targets) = doc.root.get("target").and_then(Toml::as_table) { tables.extend(targets.values().filter_map(Toml::as_table)); }
        for table in tables {
            for deps in ["dependencies", "build-dependencies"].iter().filter_map(|k| table.get(*k).and_then(Toml::as_table)) {
                for (name, dep) in deps {
                    let inherited = dep.as_table().and_then(|t| t.get("workspace")).and_then(Toml::as_bool) == Some(true);
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
    for p in index.keys() {
        let owner = Path::new(p).ancestors().skip(1).find(|d| index.contains_key(&format!("{}/Cargo.toml", d.display())));
        if let Some(owner) = owner {
            let dir = owner.to_string_lossy();
            let rel = p.strip_prefix(&format!("{dir}/")).unwrap_or(p);
            if dirs.contains::<str>(&dir) && !rel.split('/').any(|p| p.starts_with('.')) && !["tests/", "benches/", "examples/", "target/"].iter().any(|d| rel.starts_with(d)) { files.insert(p.clone()); }
        }
    }
    for p in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml"] {
        if index.contains_key(p) { files.insert(p.into()); }
    }
    dirs.extend(extra_dirs);
    Ok(Inputs { files: files.into_iter().collect(), dirs })
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
            files.entry(p.into()).or_default();
            changed.insert(format!("{prefix}{p}"));
        }
        index.extend(files.into_iter().filter(|(p, _)| !prefix.is_empty() || !commercial || !p.starts_with("apps/commercial/")).map(|(p, oid)| (format!("{prefix}{p}"), oid)));
    }
    let inputs = inputs(&index, &|p| std::fs::read(root.join(p)).map_err(|e| e.to_string())).ok()?;
    if compiler().is_empty() { return None; }
    key(compiler(), &inputs.files, &|p| {
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
    let inputs = pinned_inputs(work, pin, &index)?.files;
    if compiler().is_empty() { return Err("dev JIT: rustc did not report its compiler commit".into()); }
    let id = key(compiler(), &inputs, &|p| index.get(p).cloned()).ok_or("overlay: missing JIT input blob")?;
    Ok((id, inputs))
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
    pub run: &'a dyn Fn(&str, &std::process::Command, &dyn Fn() -> Result<(), String>) -> Result<(std::process::ExitStatus, PathBuf), String>,
    pub cancelled: &'a std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub say: &'a dyn Fn(&str),
    pub pending: &'a dyn Fn(&str),
    pub active: &'a dyn Fn() -> Result<(), String>,
}

/// Use the requested loader id: build it from the pin when it matches, otherwise require
/// a held binary. A client's content hint and an explicit --jit use this same path.
#[cfg(feature = "checkout")]
pub fn find_overlay(work: &Path, lane: &str, lease: &str, pin: &super::Pin, requested: Option<&str>, cores: u32, command: &str, build: &OverlayBuild<'_>) -> Result<Overlay, String> {
    (build.active)()?;
    let store = super::jitstore::Store::new(work);
    let index = super::relaxgit::bases(work, pin)?;
    let inputs = pinned_inputs(work, pin, &index)?;
    if compiler().is_empty() { return Err("dev JIT: rustc did not report its compiler commit".into()); }
    let id = key(compiler(), &inputs.files, &|p| index.get(p).cloned()).ok_or("overlay: missing JIT input blob")?;
    let edits = super::relaxgit::overlay(work, lane);
    let deleted: HashSet<String> = std::fs::read_to_string(edits.join(".makepad-run-deleted")).unwrap_or_default().lines().map(str::to_string).collect();
    let records = std::fs::read_to_string(edits.join(".makepad-run-edits")).map_err(|e| format!("overlay: edit records: {e}"))?;
    let mut changed = Vec::new();
    for p in records.lines().filter_map(|l| l.split_once('\t').map(|(p, _)| p)) {
        let owner = Path::new(p).ancestors().skip(1).find(|dir| index.contains_key(&format!("{}/Cargo.toml", dir.display())));
        let input = inputs.files.binary_search_by(|f| f.as_str().cmp(p)).is_ok() || inputs.dirs.iter().any(|dir| {
            let nested = owner.as_ref().is_some_and(|o| *o != Path::new(dir) && o.starts_with(dir));
            p.strip_prefix(&format!("{dir}/")).is_some_and(|rel| !nested && !rel.split('/').any(|p| p.starts_with('.')) && !["tests/", "benches/", "examples/", "target/"].iter().any(|d| rel.starts_with(d)))
        });
        if !input { continue; }
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
        store.ensure(&id, lease, build.say, build.pending, build.active, build.cancelled, |dst, active| build_dev(work, pin, &inputs.files, cores, &id, dst, build, active))?
    } else {
        store.hold(selected, lease)?;
        store.lookup(selected).ok_or_else(|| format!("overlay: dev JIT {selected} is not on this box; held keys: {}", store.snapshot().ids.join(", ")))?
    };
    (build.active)()?;
    Ok(Overlay { id: selected.into(), binary, changed })
}

/// Only committed states enter this checkout. The tree lock covers moving, building and publishing.
#[cfg(feature = "checkout")]
fn build_dev(work: &Path, pin: &super::Pin, inputs: &[String], cores: u32, id: &str, dst: &Path, build: &OverlayBuild<'_>, active: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
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
    let mut cmd = super::caps::rust_command("cargo");
    cmd.args(["build", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "-j", &cores.max(1).to_string()])
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
    if let Some(p) = inputs_of(&dep, &root, &index).iter().find(|p| !inputs.contains(p)) { return Err(format!("dev JIT: undeclared input {p}; add it to the crate's metadata.commercial.include")); }
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
