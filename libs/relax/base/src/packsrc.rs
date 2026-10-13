//! `makepad pack-src <out.store> <package>...`: the packed source archive `makepad run --src` runs from
//! (local/agent_state/fast-rust/android/ANDROID.md section 3). There is no archive format of its
//! own: the output is a git store (gitstore.rs) holding ONE synthetic commit, mounted by the VFS
//! (vfs.rs) as the workspace root.
//!
//! The file set is what the front end READS, recorded rather than guessed: the packer runs
//! `makepad check` of every package in a child process with a cold cache and MAKEPAD_RUN_READ_LOG set (vfs.rs
//! notes every read, probe and folder listing of the child and of the build scripts it starts), for
//! the crate graph and cfg of `--target`. To that it adds
//!   - the `resources/` folder of every crate it read Rust sources of (what apps load at run time),
//!   - Relax's std tree (`libs/relax/compiler/std`, whole: the loader verifies it by digest).
//! Anything read outside the workspace (and outside the child's scratch folder) is an error: the
//! archive could not serve it.
//!
//! Deterministic: blobs are hashed as git blobs (the oids of the checkout, so cache keys and edit
//! bases match), trees are built in git's order, the commit has a fixed author, date and message,
//! objects are written in oid order at the store's fixed deflate level.
use crate::gitstore::{self as store, Append, Oid, Ref, BLOB, COMMIT, TREE};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// The std tree, relative to the workspace root (std_source.rs looks for it there).
pub const STD_DIR: &str = "libs/relax/compiler/std";

pub struct Options {
    pub out: PathBuf,
    pub target: Option<String>,
    /// package, then its `check` flags
    pub packages: Vec<(String, Vec<String>)>,
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut target = None;
    let mut out = None;
    let mut packages: Vec<(String, Vec<String>)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--target" {
            target = Some(args.get(i + 1).ok_or("--target wants a triple")?.clone());
            i += 2;
        } else if a.starts_with("--") {
            let Some(last) = packages.last_mut() else { return Err(format!("{a}: flags follow a package")) };
            last.1.push(a.to_string());
            if a == "--features" {
                last.1.push(args.get(i + 1).ok_or("--features wants a list")?.clone());
                i += 1;
            }
            i += 1;
        } else if out.is_none() {
            out = Some(PathBuf::from(a));
            i += 1;
        } else {
            packages.push((a.to_string(), Vec::new()));
            i += 1;
        }
    }
    let out = out.ok_or("usage: pack-src [--target <triple>] <out.store> <package> [flags] [<package>...]")?;
    if packages.is_empty() {
        return Err("pack-src needs at least one package".into());
    }
    Ok(Options { out, target, packages })
}

/// The workspace root: the nearest folder at or above the current one whose Cargo.toml has a `[workspace]` (or `workspace.` keys).
fn workspace_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        if std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]" || l.starts_with("workspace."))) {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("pack-src: run it inside the workspace (no workspace Cargo.toml above the current folder)".into());
        }
    }
}

fn excluded(rel: &str) -> bool {
    let mut parts = rel.split('/');
    matches!(parts.next(), Some("target" | "local")) || rel.split('/').any(|p| p == ".git")
}

/// Every regular file below `dir` (relative paths with `/`), skipping `.git`.
fn walk(root: &Path, dir: &str, out: &mut BTreeSet<String>) -> Result<(), String> {
    let abs = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
    let Ok(rd) = std::fs::read_dir(&abs) else { return Ok(()) };
    for e in rd {
        let e = e.map_err(|e| format!("{}: {e}", abs.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
        let t = std::fs::metadata(e.path()).map_err(|e| format!("{rel}: {e}"))?;
        if t.is_dir() {
            walk(root, &rel, out)?;
        } else if t.is_file() {
            out.insert(rel);
        }
    }
    Ok(())
}

enum Node {
    File(u32, Oid),
    Dir(BTreeMap<String, Node>),
}

fn insert(dir: &mut BTreeMap<String, Node>, path: &str, mode: u32, oid: Oid) {
    match path.split_once('/') {
        None => {
            dir.insert(path.to_string(), Node::File(mode, oid));
        }
        Some((d, rest)) => {
            let Node::Dir(sub) = dir.entry(d.to_string()).or_insert_with(|| Node::Dir(BTreeMap::new())) else { unreachable!("a file and a folder share a path") };
            insert(sub, rest, mode, oid);
        }
    }
}

fn git_oid(kind: &str, data: &[u8]) -> Oid {
    let mut h = format!("{kind} {}\0", data.len()).into_bytes();
    h.extend_from_slice(data);
    store::sha1(&h)
}

/// A tree object (git's order: names as bytes, a folder compared as `name/`).
fn write_tree(dir: &BTreeMap<String, Node>, objects: &mut Vec<(Oid, u8, Vec<u8>)>) -> Oid {
    let mut ents: Vec<(Vec<u8>, String, u32, Oid)> = Vec::new();
    for (name, node) in dir {
        match node {
            Node::File(mode, oid) => ents.push((name.clone().into_bytes(), name.clone(), *mode, *oid)),
            Node::Dir(sub) => {
                let oid = write_tree(sub, objects);
                let mut key = name.clone().into_bytes();
                key.push(b'/');
                ents.push((key, name.clone(), 0o40000, oid));
            }
        }
    }
    ents.sort_by(|a, b| a.0.cmp(&b.0));
    let mut data = Vec::new();
    for (_, name, mode, oid) in &ents {
        data.extend_from_slice(format!("{mode:o} {name}\0").as_bytes());
        data.extend_from_slice(oid);
    }
    let oid = git_oid("tree", &data);
    objects.push((oid, TREE, data));
    oid
}

#[cfg(unix)]
fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) { 0o100755 } else { 0o100644 }
}
#[cfg(not(unix))]
fn file_mode(_: &Path) -> u32 {
    0o100644
}

/// The nearest folder at or above `rel`'s folder that is a package (in `pkgs`; "" is the root).
fn package_of<'a>(rel: &str, pkgs: &'a BTreeSet<String>) -> Option<&'a str> {
    let mut dir = rel;
    loop {
        dir = match dir.rfind('/') {
            Some(i) => &dir[..i],
            None if dir.is_empty() => return None,
            None => "",
        };
        if let Some(p) = pkgs.get(dir) {
            return Some(p.as_str());
        }
        if dir.is_empty() {
            return None;
        }
    }
}

pub fn run(opts: Options) -> Result<(), String> {
    let root = workspace_root()?;
    // the read log's spelling (vfs::normalize_path: `/` separators, no verbatim prefix, lowercase drive)
    let root_s = crate::vfs::normalize_path(&root.to_string_lossy()).trim_end_matches('/').to_string();
    let work = PathBuf::from(format!("{}.pack-work", opts.out.display()));
    if work.exists() {
        std::fs::remove_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    }
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let log = work.join("reads.log");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let work_s = crate::vfs::normalize_path(&work.canonicalize().map_err(|e| e.to_string())?.to_string_lossy());
    let work_plain = crate::vfs::normalize_path(&work.to_string_lossy());
    let t0 = std::time::Instant::now();
    for (pkg, flags) in &opts.packages {
        let out_log = work.join(format!("check-{pkg}.log"));
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("check").arg("--no-serve");
        // every workspace crate the package reaches (platform, widgets, its families ...), for the
        // target: an app check covers only the app's own crates, and an error in a shared crate under
        // the target's cfgs (video.rs on Android) otherwise first shows up as a JIT compile error on
        // the device. About 12-17 s per package on the Mac with the shared cache.
        cmd.arg("--workspace");
        cmd.arg(pkg).args(flags).current_dir(&root);
        cmd.env("MAKEPAD_RUN_CACHE_DIR", work.join("cache"));
        cmd.env("MAKEPAD_RUN_READ_LOG", &log);
        cmd.env_remove("MAKEPAD_RUN_VFS").env_remove("MAKEPAD_RUN_WORKSPACE").env_remove("MAKEPAD_RUN_EDITS").env_remove("MAKEPAD_RUN_CHECK_TARGET");
        cmd.env("MAKEPAD_RUN_OFFLINE", "1").env("MAKEPAD_CEF_OFFLINE", "1");
        if let Some(t) = &opts.target {
            cmd.env("MAKEPAD_RUN_CHECK_TARGET", t);
        }
        let f = std::fs::File::create(&out_log).map_err(|e| e.to_string())?;
        cmd.stdout(f.try_clone().map_err(|e| e.to_string())?).stderr(f);
        let st = cmd.status().map_err(|e| format!("{}: {e}", exe.display()))?;
        let text = std::fs::read_to_string(&out_log).unwrap_or_default();
        let diag = text.lines().filter(|l| l.contains(": E0") || l.starts_with("error")).count();
        eprintln!("pack-src: check {pkg}{}: {} ({diag} diagnostic lines, {:.0} s)", opts.target.as_ref().map_or(String::new(), |t| format!(" --target {t}")), st, t0.elapsed().as_secs_f64());
        // an archive of a tree that does not compile is a broken app on the phone (found on the device, minutes
        // later): the build stops here with the first errors
        if diag > 0 {
            let errs: Vec<&str> = text.lines().filter(|l| l.contains(": E0") || l.starts_with("error")).take(8).collect();
            return Err(format!("check of {pkg} found {diag} error line(s); log {}:\n{}", out_log.display(), errs.join("\n")));
        }
        // a check that failed to run to the end read fewer files than a run needs
        if st.code().is_none() || !text.contains("makepad check:") {
            let tail: Vec<&str> = text.lines().rev().take(8).collect();
            return Err(format!("check of {pkg} did not finish ({}); log {}:\n{}", st, out_log.display(), tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
        }
    }
    let reads = std::fs::read_to_string(&log).map_err(|e| format!("no read log: {e}"))?;

    // classify
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut listed: BTreeSet<String> = BTreeSet::new();
    let mut outside: BTreeSet<String> = BTreeSet::new();
    for line in reads.lines() {
        let Some((kind, path)) = line.split_once('\t') else { continue };
        // the child's own scratch folder (cache, build script outputs) is not a source
        if path.starts_with(&format!("{work_s}/")) || path.starts_with(&format!("{work_plain}/")) {
            continue;
        }
        let Some(rel) = path.strip_prefix(&format!("{root_s}/")).or_else(|| (path == root_s).then_some("")) else {
            outside.insert(path.to_string());
            continue;
        };
        if rel.is_empty() || excluded(rel) {
            continue;
        }
        match kind {
            "f" | "p" => {
                if std::fs::metadata(root.join(rel)).is_ok_and(|m| m.is_file()) {
                    files.insert(rel.to_string());
                }
            }
            "d" => {
                listed.insert(rel.to_string());
            }
            _ => {}
        }
    }
    if !outside.is_empty() {
        let mut m = format!("the run reads {} paths outside the workspace {root_s}, the archive cannot serve them:", outside.len());
        for p in outside.iter().take(25) {
            m.push_str(&format!("\n  {p}"));
        }
        return Err(m);
    }

    // packages: folders whose Cargo.toml was read; crates of the graph: those whose Rust was read
    let pkgs: BTreeSet<String> = files
        .iter()
        .filter_map(|f| f.strip_suffix("Cargo.toml").filter(|d| d.is_empty() || d.ends_with('/')))
        .map(|d| d.trim_end_matches('/').to_string())
        .collect();
    let mut graph: BTreeSet<String> = BTreeSet::new();
    for f in &files {
        if f.ends_with(".rs") || f.ends_with(".rx") {
            if let Some(p) = package_of(f, &pkgs) {
                graph.insert(p.to_string());
            }
        }
    }
    let read_only: BTreeSet<String> = files.clone();
    // an Android app reads its resources (fonts, images, ..) from the APK's assets, which the
    // APK builder packages per crate (cargo makepad android): the archive holds the sources
    let assets_in_apk = opts.target.as_deref().is_some_and(|t| t.contains("android"));
    for p in graph.iter().filter(|_| !assets_in_apk) {
        let res = if p.is_empty() { "resources".to_string() } else { format!("{p}/resources") };
        walk(&root, &res, &mut files)?;
    }
    let mut std_files = BTreeSet::new();
    walk(&root, STD_DIR, &mut std_files)?;
    if std_files.is_empty() {
        return Err(format!("{STD_DIR} is not in the workspace"));
    }
    files.extend(std_files);
    // a folder the front end listed lists the same in the archive: its files are all in (the
    // listing shows no folder that holds nothing packed; reported below)
    let before_listed = files.len();
    for d in &listed {
        let mut here = BTreeSet::new();
        let _ = walk_one(&root, d, &mut here);
        files.extend(here);
    }
    let from_listings = files.len() - before_listed;
    files.retain(|f| !excluded(f));

    // objects
    let mut objects: Vec<(Oid, u8, Vec<u8>)> = Vec::new();
    let mut seen: HashMap<Oid, ()> = HashMap::new();
    let mut tree: BTreeMap<String, Node> = BTreeMap::new();
    let mut sizes: Vec<(String, u64, Oid)> = Vec::new();
    for f in &files {
        let abs = root.join(f);
        let data = std::fs::read(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
        let oid = git_oid("blob", &data);
        insert(&mut tree, f, file_mode(&abs), oid);
        sizes.push((f.clone(), data.len() as u64, oid));
        if seen.insert(oid, ()).is_none() {
            objects.push((oid, BLOB, data));
        }
    }
    let tree_oid = write_tree(&tree, &mut objects);
    let commit = format!("tree {}\nauthor makepad pack-src <pack-src@makepad> 0 +0000\ncommitter makepad pack-src <pack-src@makepad> 0 +0000\n\npacked sources\n", store::hex(&tree_oid)).into_bytes();
    let commit_oid = git_oid("commit", &commit);
    objects.push((commit_oid, COMMIT, commit));
    objects.sort_by(|a, b| a.0.cmp(&b.0));
    objects.dedup_by(|a, b| a.0 == b.0);

    if opts.out.exists() {
        std::fs::remove_file(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
    }
    let s = store::append(&opts.out, Append { objects: &objects, reference: Ref { key: "pack-src".into(), commit: commit_oid, time: 0 } }, &|r| r.to_vec())?;
    let total = std::fs::metadata(&opts.out).map_err(|e| e.to_string())?.len();

    // report
    let mut per_crate: BTreeMap<String, (u64, u64, usize)> = BTreeMap::new();
    let mut stored: HashMap<Oid, u64> = HashMap::new();
    for (path, raw, oid) in &sizes {
        let c = s.stored_size(oid).map_err(|e| e.to_string())?.unwrap_or(0);
        let c = *stored.entry(*oid).or_insert(c);
        let key = if path.starts_with(&format!("{STD_DIR}/")) {
            "(makepad run std)".to_string()
        } else {
            match package_of(path, &pkgs) {
                Some("") | None => "(workspace root)".to_string(),
                Some(p) => p.to_string(),
            }
        };
        let e = per_crate.entry(key).or_default();
        e.0 += raw;
        e.1 += c;
        e.2 += 1;
    }
    println!("pack-src: {} -> {} bytes, {} files, {} objects, commit {}", opts.out.display(), total, sizes.len(), objects.len(), store::hex(&commit_oid));
    println!("pack-src: read by the front end: {} files; resources and std added: {}; {} folders listed; {} crates with sources", read_only.len(), files.len().saturating_sub(read_only.len()), listed.len(), graph.len());
    println!("pack-src: size per crate (stored bytes, raw bytes, files):");
    let mut rows: Vec<_> = per_crate.iter().collect();
    rows.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then(a.0.cmp(b.0)));
    for (name, (raw, comp, n)) in rows {
        println!("  {comp:>10} {raw:>10} {n:>5}  {name}");
    }
    println!("pack-src: the 10 largest files (stored, raw):");
    let mut big: Vec<_> = sizes.iter().collect();
    big.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (path, raw, oid) in big.into_iter().take(10) {
        println!("  {:>10} {raw:>10}  {path}", stored[oid]);
    }
    // listed folders whose listing differs: a subfolder of it holds nothing in the archive
    let mut missing = Vec::new();
    for d in &listed {
        let Ok(rd) = std::fs::read_dir(root.join(d)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = format!("{d}/{name}");
            if name != ".git" && !excluded(&rel) && e.file_type().is_ok_and(|t| t.is_dir()) && !files.range(format!("{rel}/")..).next().is_some_and(|f| f.starts_with(&format!("{rel}/"))) {
                missing.push(rel);
            }
        }
    }
    println!("pack-src: {} files added because their folder was listed; {} listed subfolders hold nothing in the archive", from_listings, missing.len());
    for m in missing.iter().take(20) {
        println!("  {m}");
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}

/// The regular files directly in `dir`.
fn walk_one(root: &Path, dir: &str, out: &mut BTreeSet<String>) -> Result<(), String> {
    for e in std::fs::read_dir(root.join(dir)).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if e.file_type().map_err(|e| e.to_string())?.is_file() {
            out.insert(format!("{dir}/{}", e.file_name().to_string_lossy()));
        }
    }
    Ok(())
}
