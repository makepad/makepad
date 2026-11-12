//! `--allow-c`: native dependencies built by real cargo.
//!
//! Without the flag nothing native from a third party runs. With it, the packages whose build
//! script compiles or links native code (`links =`, a build-dependency on cc / cmake / pkg-config /
//! bindgen / vcpkg, or a script that reads `DEP_*` metadata of those) are built by cargo itself, so
//! their build scripts run verbatim: `cargo check --release -p <package> --message-format=json`
//! in the run's workspace, same Cargo.lock, same cargo flags. Cargo's `RUSTC_WRAPPER` is this
//! executable (`wrapper`): it compiles with the real rustc only what the native scripts need (the
//! scripts of the native packages and their build-dependency closure) and answers every other
//! crate with empty outputs, so cargo decides features, cfgs and env for the whole graph and
//! Relax compiles the Rust. Target scripts query Relax's compiler interface even when Cargo
//! compiles their executable; scripts feeding native host dependencies query the native rustc.
//! Cargo's `build-script-executed` messages give the cfgs, rustc-env and
//! OUT_DIR the Relax compile of those crates uses and the static archives and system libraries
//! the scripts link; the archives are linked into ONE shared library with the system linker,
//! which the run loads by path before the program starts.
//!
//! The cargo step and the link are cached under `<relax dir>/native/<key>/`: the key covers the
//! cargo flags, the resolved packages of the native closure (versions, features, sources; a path
//! package's files), the C compiler and the target -- not the package the run is for, so two
//! packages whose native closure is the same share one build. A run with an unchanged key starts
//! no cargo. The scripts' OUT_DIRs live in cargo's target for the selection
//! (`native/cargo-target/<selection>`), which builds under other keys share: the receipt notes
//! each script run cargo's `output` file stood for, and a later run of the script under another
//! key (a path package's source moved, and moved back) makes it no hit.
//!
//! Roots: each `--allow-c` build names itself in `native/roots/<slot>` before it starts, where a
//! slot is the workspace, package, cargo flags and target of a run: the slot's key directory and
//! cargo selection (both tiers' targets). The cache's prune (`prune`, cache.rs prune_root) keeps
//! what a root names and drops everything else: a slot's superseded builds, selections no root
//! uses, the roots of workspaces that are gone. No size cap: a slot's last build stays.
//!
//! A check without the flag starts no cargo either, but it reads a receipt the flag left behind:
//! the same key, so the cfgs, rustc-env and OUT_DIR of that build are what its compile uses
//! (`cached_scripts`). A tree whose C was built once type-checks whole from then on; only an
//! absent receipt (or one that does not cover every native package) leaves a check without them.
//!
//! Cargo brings the lock up to date as `cargo run` does, and fetches only what is missing (the
//! run's --offline / --frozen and cargo's offline settings still hold). Cargo.lock is not
//! tracked, so each tree keeps its own, and with `--locked` every native build failed on a tree
//! whose lock predated a new workspace member. What cargo updates leaves the key as it is.
//! Relax reads path packages from their manifests, and cargo keeps every locked version the
//! manifests still accept. The update adds a member or a path package's new version that the
//! lock lacks, drops what nothing uses and puts the rest in cargo's order.
//!
//! Two tiers, as for the Rust code: the first build compiles the C at opt-level 0 (a cold start
//! waits for it; Robrix's sqlite3.c alone is 28 s at -O3 with gcc and 3 s at -O0). Then cargo
//! builds it again with the release opt-level on a background thread (2 jobs, its own cargo
//! target) and the optimized archives replace the library for the starts after it. The scripts'
//! outputs (cfgs, env, OUT_DIR) stay the first build's, so the Relax compile of these crates and
//! its cache do not change with the tier; an optimized build whose scripts say anything else
//! than the first one's is not taken. `<key>/fast` marks a library of the first tier.

use crate::unitgraph::{BuildScript, Graph};
use std::process::{Command, Stdio};

/// What the run asks of cargo.
pub struct Request<'a> {
    pub workspace: &'a str,
    pub package: &'a str,
    pub cargo_flags: &'a [String],
    pub relax_dir: &'a str,
    pub triple: &'a str,
    pub resolvable: Resolvable,
}

/// What a caller allows the native path to do (`unitgraph::run_build_scripts`).
#[derive(Clone, Copy)]
pub enum Mode<'a> {
    /// `--allow-c`: cargo builds the native packages (or reuses the cached build), and their
    /// library is linked into the run.
    Build(&'a Request<'a>),
    /// A check without `--allow-c`: read the cfgs, env and OUT_DIR of a native build already in
    /// the cache. Nothing is built and no native code is linked, so a tree whose C was built once
    /// (with the flag) type-checks whole from then on.
    Cached(&'a Request<'a>),
}

impl<'a> Mode<'a> {
    pub fn request(&self) -> &'a Request<'a> {
        match self {
            Mode::Build(req) | Mode::Cached(req) => req,
        }
    }

    /// Whether cargo may run and the native library be linked.
    pub fn builds(&self) -> bool {
        matches!(self, Mode::Build(_))
    }
}

/// Which of the C symbols `names` the loader process binds once the libraries `dylibs` and the
/// frameworks `frameworks` are loaded: what the dynamic loader can bind a native library's
/// import to (the C runtime and system libraries; the program's code is in none of them).
pub type Resolvable = fn(names: &[String], dylibs: &[String], frameworks: &[String]) -> Vec<bool>;

/// What cargo's build script of one native package produced.
pub struct ScriptOut {
    pub krate: usize,
    /// A separate host feature set of a package also built for the target.
    pub host: bool,
    pub cfgs: Vec<String>,
    pub env: Vec<(String, String)>,
    pub out_dir: String,
    /// system libraries and frameworks the script links (name, framework)
    pub system: Vec<(String, bool)>,
}

/// The shared library the native archives were linked into.
pub struct NativeLib {
    pub path: String,
    /// the names of the static libraries inside it (`#[link(name = .., kind = "static")]`)
    pub statics: Vec<String>,
    /// the native packages ("name version")
    pub crates: Vec<String>,
}

pub struct Built {
    pub scripts: Vec<ScriptOut>,
    pub lib: Option<NativeLib>,
}

const BUILD_DEPS: [&str; 6] = ["cc", "cmake", "pkg_config", "bindgen", "vcpkg", "nasm_rs"];

/// An exported accessor for a native symbol whose own visibility may be hidden.
/// Hex keeps arbitrary assembler names out of the generated C identifiers.
pub fn address_name(name: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::from("__relax_native_address_");
    for byte in name { write!(out, "{byte:02x}").unwrap(); }
    out
}

/// Packages that keep `cc` as a build-dependency but compile nothing with it under a feature:
/// (package, feature). blake3's `pure` leaves every C and assembly implementation out, so its
/// script builds nothing with `cc`; it and a build script that depends on blake3 (an app's
/// script that hashes its assets) need no `--allow-c` and run under Relax like any other.
const C_FREE_WITH_FEATURE: [(&str, &str); 1] = [("blake3", "pure")];

/// Why a package's build script is native, if it is.
pub fn native_reason(g: &Graph, bs: &BuildScript) -> Option<String> {
    let (dir, name, _) = &g.pkg[bs.krate];
    let manifest = crate::vfs::read_to_string(format!("{}/Cargo.toml", dir)).unwrap_or_default();
    let links = manifest.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("links").is_some_and(|r| r.trim_start().starts_with('='))
    });
    if links {
        return Some(format!("{}: links =", name));
    }
    let c_free = C_FREE_WITH_FEATURE.iter().any(|(p, f)| p == name && bs.features.iter().any(|x| x == f));
    for (_, ext) in &bs.build_deps {
        if BUILD_DEPS.contains(&ext.as_str()) && !c_free {
            return Some(format!("{}: build-dependency {}", name, ext));
        }
    }
    let src = crate::vfs::read_to_string(&bs.src_path).unwrap_or_default();
    if src.contains("DEP_") {
        return Some(format!("{}: reads DEP_ metadata", name));
    }
    None
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hash_dir(dir: &std::path::Path, h: &mut u64) {
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n == "target" || n == ".git" || n == "Cargo.lock" {
            continue;
        }
        let p = e.path();
        *h = fnv(n.as_bytes(), *h);
        match e.file_type() {
            Ok(t) if t.is_dir() => hash_dir(&p, h),
            Ok(_) => {
                if let Ok(d) = std::fs::read(&p) {
                    *h = fnv(&d, *h);
                }
            }
            Err(_) => {}
        }
    }
}

/// The resolved package identities and feature sets, independent of lockfile formatting,
/// unused patches, and the traversal order of Cargo's units. Include build dependencies.
fn resolved_packages_key<'a>(raw: impl IntoIterator<Item = &'a crate::unitgraph::RawUnit>) -> u64 {
    let mut packages: Vec<_> = raw.into_iter().map(|unit| {
        let mut features = unit.features.clone();
        features.sort();
        features.dedup();
        (unit.pkg.clone(), features)
    }).collect();
    packages.sort();
    packages.dedup();
    fnv(format!("{packages:?}").as_bytes(), 0xcbf29ce484222325)
}

fn cc() -> String {
    std::env::var("CC").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "cc".to_string())
}

/// (name, version) of a cargo package id, either format.
fn id_name_version(id: &str) -> (String, String) {
    if id.contains('#') {
        let (_, n, v) = crate::unitgraph::parse_pkg_id(id);
        return (n, v);
    }
    let mut it = id.split_whitespace();
    (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
}

/// Where the native build of `seeds` under `req` lives, and the cargo selection it is keyed on.
struct Plan {
    script_pkgs: Vec<String>,
    lib_pkgs: Vec<String>,
    /// cargo flags without `--release` (cargo check runs the release profile)
    flags: Vec<String>,
    /// "name version" of each native package
    names: Vec<String>,
    key: u64,
    selection: u64,
    dir: String,
    messages_path: String,
    lib_path: String,
    fast_mark: String,
}

/// The key a native build is stored under (module doc: the cargo flags, the resolved packages,
/// the C compiler and the target). `build` writes under it and `cached_scripts` reads it, so a
/// check reads exactly what a `--allow-c` run of the same inputs wrote.
fn plan(g: &Graph, seeds: &[usize], req: &Request) -> Plan {
    let pk = |bs: &BuildScript| g.pkg[bs.krate].clone();
    // packages whose script runs for real: the native ones and the build-dependency closure of
    // those; packages whose lib compiles for real: that closure
    let mut script_pkgs: Vec<String> = Vec::new();
    let mut lib_pkgs: Vec<String> = Vec::new();
    for &s in seeds {
        let bs = &g.build_scripts[s];
        let name = pk(bs).1;
        if !script_pkgs.contains(&name) {
            script_pkgs.push(name);
        }
        for n in dependency_packages(&g.raw, bs.build_deps.iter().map(|d| d.0).collect()) {
            if !lib_pkgs.contains(&n) {
                lib_pkgs.push(n.clone());
            }
            if !script_pkgs.contains(&n) {
                script_pkgs.push(n);
            }
        }
    }
    script_pkgs.sort();
    lib_pkgs.sort();
    // cargo flags without `--release` (cargo check runs the release profile below)
    let flags: Vec<String> = req.cargo_flags.iter().filter(|f| f.as_str() != "--release").cloned().collect();

    // ---- the key: what the native scripts' outputs depend on. Not the package the run is for,
    // and of the resolved graph only the native closure (the packages whose scripts or libs
    // cargo builds for real): two packages whose closure resolves alike share the build.
    let selection = selection_key(&script_pkgs, &lib_pkgs);
    let mut h = fnv(b"native-link-v14", 0xcbf29ce484222325);
    h = fnv(req.triple.as_bytes(), h);
    h = fnv(&selection.to_le_bytes(), h);
    for f in &flags {
        h = fnv(f.as_bytes(), h);
        h = fnv(b"\0", h);
    }
    h = fnv(&resolved_packages_key(g.raw.iter().filter(|u| script_pkgs.binary_search(&u.pkg.1).is_ok())).to_le_bytes(), h);
    let mut native_env: Vec<_> = std::env::vars().filter(|(k, _)| {
        ["CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS", "PKG_CONFIG"]
            .iter().any(|prefix| k == prefix || k.starts_with(&format!("{prefix}_")))
    }).collect();
    native_env.sort();
    for (k, v) in native_env {
        h = fnv(k.as_bytes(), h);
        h = fnv(b"\0", h);
        h = fnv(v.as_bytes(), h);
        h = fnv(b"\0", h);
    }
    let cc_version = Command::new(cc()).arg("--version").output().map(|o| o.stdout).unwrap_or_default();
    h = fnv(cc_version.split(|&b| b == b'\n').next().unwrap_or(&[]), h);
    let mut native_packages: Vec<_> = seeds.iter().map(|&s| {
        let bs = &g.build_scripts[s];
        let mut features = bs.features.clone();
        features.sort();
        features.dedup();
        (pk(bs), features, g.crates[bs.krate].registry)
    }).collect();
    native_packages.sort();
    native_packages.dedup();
    let mut names: Vec<String> = Vec::new();
    for ((dir, name, ver), features, registry) in native_packages {
        names.push(format!("{} {}", name, ver));
        h = fnv(format!("{} {} {:?}", name, ver, features).as_bytes(), h);
        // a registry or git package's source is its id (the resolved packages above); a path
        // package's is its files
        if !registry {
            hash_dir(std::path::Path::new(&dir), &mut h);
        }
    }
    names.sort();
    names.dedup();
    let dir = format!("{}/native/{:016x}", req.relax_dir, h);
    let messages_path = format!("{}/messages.jsonl", dir);
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib_path = format!("{}/libnative.{}", dir, ext);
    let fast_mark = format!("{}/fast", dir);
    Plan { script_pkgs, lib_pkgs, flags, names, key: h, selection, dir, messages_path, lib_path, fast_mark }
}

/// Builds the native packages `seeds` (indices into `g.build_scripts`) with cargo, or finds the
/// cached build, and links them. The error names what failed.
pub fn build(g: &Graph, seeds: &[usize], req: &Request) -> Result<Built, String> {
    let exe = std::env::current_exe().map_err(|e| format!("this executable: {e}"))?;
    let plan = plan(g, seeds, req);
    // named before anything is read or built under it: a prune from now on keeps it
    write_root(req, &plan)?;
    let flags: Vec<&String> = plan.flags.iter().collect();
    let out = match std::fs::read_to_string(&plan.messages_path).ok().and_then(|m| cached(&g.pkg, &script_table(g), seeds, &m, &plan.lib_path)) {
        Some(out) => out,
        None => {
            let m = run_cargo(g, req, &exe, &flags, &plan.script_pkgs, &plan.lib_pkgs, seeds, Tier::Fast)?;
            std::fs::create_dir_all(&plan.dir).map_err(|e| format!("{}: {}", plan.dir, e))?;
            std::fs::write(&plan.fast_mark, b"").map_err(|e| format!("{}: {}", plan.fast_mark, e))?;
            std::fs::write(&plan.messages_path, &m).map_err(|e| format!("{}: {}", plan.messages_path, e))?;
            // a stale library of an earlier link under this key is not trusted
            let _ = std::fs::remove_file(&plan.lib_path);
            outputs(g, seeds, &m)?
        }
    };
    if out.archives.is_empty() && out.dylibs.is_empty() && out.frameworks.is_empty() {
        return Ok(Built { scripts: out.scripts, lib: None });
    }
    if !std::path::Path::new(&plan.lib_path).is_file() {
        link(&plan.lib_path, &out.archives, &out.search, &out.fw_search, &out.dylibs, &out.frameworks, req.triple, req.resolvable)?;
    }
    if std::path::Path::new(&plan.fast_mark).is_file() {
        optimize_later(g, req, exe, &flags, plan.script_pkgs, plan.lib_pkgs, seeds, &out, plan.lib_path.clone(), plan.fast_mark);
    }
    Ok(Built { scripts: out.scripts, lib: Some(NativeLib { path: plan.lib_path, statics: out.statics, crates: plan.names }) })
}

/// The script outputs of the native build already cached under `req`'s key (no cargo, no link):
/// the cfgs, env and OUT_DIR a check compiles the native packages with. `None` when nothing is
/// cached, when the receipt does not cover every seed (a partial build is not a build), or when
/// the directories it names are gone. `cached` rejects the receipt of a build that did not link
/// or whose library the target no longer holds, so only a whole build is read back as one.
pub fn cached_scripts(g: &Graph, seeds: &[usize], req: &Request) -> Option<Vec<ScriptOut>> {
    let plan = plan(g, seeds, req);
    let messages = std::fs::read_to_string(&plan.messages_path).ok()?;
    let out = cached(&g.pkg, &script_table(g), seeds, &messages, &plan.lib_path)?;
    for &s in seeds {
        let (_, n, v) = &g.pkg[g.build_scripts[s].krate];
        if !out.scripts.iter().any(|o| g.pkg[o.krate].1 == *n && g.pkg[o.krate].2 == *v) {
            return None;
        }
    }
    Some(out.scripts)
}

/// The wrapper selects real builds by package name. Selecting one unit therefore
/// selects every version and host/target feature set of that name. Follow all of
/// those units' edges so none of their actual dependencies is replaced by a stub.
fn dependency_packages(raw: &[crate::unitgraph::RawUnit], mut stack: Vec<usize>) -> Vec<String> {
    let mut by_package = std::collections::HashMap::<&str, Vec<usize>>::new();
    for (i, unit) in raw.iter().enumerate() {
        by_package.entry(&unit.pkg.1).or_default().push(i);
    }
    let mut names = std::collections::BTreeSet::new();
    while let Some(i) = stack.pop() {
        let Some(unit) = raw.get(i) else { continue; };
        if !names.insert(unit.pkg.1.clone()) { continue; }
        for &i in &by_package[unit.pkg.1.as_str()] {
            stack.extend(raw[i].deps.iter().map(|d| d.0));
        }
    }
    names.into_iter().collect()
}

/// What cargo's messages say the native scripts produced.
struct Outputs {
    scripts: Vec<ScriptOut>,
    /// (OUT_DIR, the script run the receipt saw there: `run_stamp`) of each script
    runs: Vec<(String, String)>,
    statics: Vec<String>,
    archives: Vec<String>,
    search: Vec<String>,
    fw_search: Vec<String>,
    dylibs: Vec<String>,
    frameworks: Vec<String>,
}

/// (crate, features) of each build script of the graph: what the native steps read of it
fn script_table(g: &Graph) -> Vec<(usize, Vec<String>)> {
    g.build_scripts.iter().map(|b| (b.krate, b.features.clone())).collect()
}

fn outputs(g: &Graph, seeds: &[usize], messages: &str) -> Result<Outputs, String> {
    outputs_with(&g.pkg, &script_table(g), seeds, messages)
}

/// The cargo run cached under this key (`messages`, written once that run succeeded) while what
/// it names is still there: every script's OUT_DIR (generated sources the compile reads), as
/// that run left it, and the linked library when the scripts link anything. Cargo writes
/// `linked_libs` for every script, so a cfg/env-only script, which leaves no library, is reused
/// on its OUT_DIR alone. The OUT_DIRs are cargo's, in the selection's target that other keys'
/// builds share: a script that ran again since (under another key) left other files there.
fn cached(pkg: &[(String, String, String)], scripts_of: &[(usize, Vec<String>)], seeds: &[usize], messages: &str, lib_path: &str) -> Option<Outputs> {
    let out = outputs_with(pkg, scripts_of, seeds, messages).ok()?;
    let links = !(out.archives.is_empty() && out.dylibs.is_empty() && out.frameworks.is_empty());
    let there = out.scripts.iter().all(|s| std::path::Path::new(&s.out_dir).is_dir()) && (!links || std::path::Path::new(lib_path).is_file());
    let same_runs = out.runs.iter().all(|(dir, seen)| !seen.is_empty() && *seen == run_stamp(dir));
    (there && same_runs).then_some(out)
}

/// The script run that left `out_dir` as it is: cargo's `output` file beside it (the script's
/// stdout, which cargo writes after every run), by length and modification time. Empty when
/// there is none.
fn run_stamp(out_dir: &str) -> String {
    let output = std::path::Path::new(out_dir).with_file_name("output");
    std::fs::metadata(output).ok().and_then(|m| {
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}.{}", m.len(), t.as_nanos()))
    }).unwrap_or_default()
}

fn outputs_with(pkg: &[(String, String, String)], scripts_of: &[(usize, Vec<String>)], seeds: &[usize], messages: &str) -> Result<Outputs, String> {

    // ---- what cargo's scripts produced
    let mut scripts = Vec::new();
    let mut statics: Vec<String> = Vec::new();
    let mut archives: Vec<String> = Vec::new();
    let mut search: Vec<String> = Vec::new();
    let mut fw_search: Vec<String> = Vec::new();
    let mut dylibs: Vec<String> = Vec::new();
    let mut frameworks: Vec<String> = Vec::new();
    let mut runs: Vec<(String, String)> = Vec::new();
    for line in messages.lines() {
        let j = crate::json::parse(line.as_bytes()).map_err(|e| format!("cargo message: {e}"))?;
        let (n, v) = id_name_version(j.get("package_id").map(|p| p.str()).unwrap_or(""));
        let Some(&s) = seeds.iter().find(|&&s| {
            let p = &pkg[scripts_of[s].0];
            p.1 == n && p.2 == v
        }) else { continue };
        let strs = |k: &str| -> Vec<String> { j.get(k).map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default() };
        let mut out = ScriptOut { krate: scripts_of[s].0, host: matches!(j.get("relax_host"), Some(crate::json::Json::Bool(true))), cfgs: strs("cfgs"), env: Vec::new(), out_dir: j.get("out_dir").map(|o| o.str().to_string()).unwrap_or_default(), system: Vec::new() };
        runs.push((out.out_dir.clone(), j.get("relax_run").map(|r| r.str().to_string()).unwrap_or_default()));
        if let Some(e) = j.get("env") {
            for kv in e.arr() {
                if let [k, v] = kv.arr() {
                    out.env.push((k.str().to_string(), v.str().to_string()));
                }
            }
        }
        if out.host {
            scripts.push(out);
            continue;
        }
        for p in strs("linked_paths") {
            match p.split_once('=') {
                Some(("framework", d)) => fw_search.push(d.to_string()),
                Some((_, d)) => search.push(d.to_string()),
                None => search.push(p),
            }
        }
        for l in strs("linked_libs") {
            let (kind, name) = match l.split_once('=') {
                Some((k, n)) => (k.to_string(), n.split(':').next().unwrap_or(n).to_string()),
                None => (String::new(), l.clone()),
            };
            match kind.as_str() {
                "static" => statics.push(name),
                "framework" => {
                    frameworks.push(name.clone());
                    out.system.push((name, true));
                }
                _ => {
                    dylibs.push(name.clone());
                    out.system.push((name, false));
                }
            }
        }
        scripts.push(ScriptOut { ..out });
    }
    // the out dirs of the scripts hold archives too (cc's output is `native=<out_dir>`)
    for name in &statics {
        let file = format!("lib{}.a", name);
        match search.iter().map(|d| format!("{}/{}", d, file)).find(|p| std::path::Path::new(p).is_file()) {
            Some(p) => {
                if !archives.contains(&p) {
                    archives.push(p);
                }
            }
            None => return Err(format!("--allow-c: a native build script links static library `{}`, but {} is in none of its search paths ({})", name, file, search.join(", "))),
        }
    }
    Ok(Outputs { scripts, runs, statics, archives, search, fw_search, dylibs, frameworks })
}

#[derive(Clone, Copy, PartialEq)]
enum Tier {
    /// opt-level 0: what a start waits for
    Fast,
    /// the release opt-level, built in the background for later starts
    Optimized,
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The optimized tier of a first-tier library (module doc), on a background thread: cargo with
/// the release opt-level in its own target, then, when its scripts said what the first tier's
/// did, its archives linked over `lib_path` (the running process keeps the library it loaded).
/// Once per process; a process that ends first leaves it to the next start (cargo resumes).
#[allow(clippy::too_many_arguments)]
fn optimize_later(g: &Graph, req: &Request, exe: std::path::PathBuf, flags: &[&String], script_pkgs: Vec<String>, lib_pkgs: Vec<String>, seeds: &[usize], fast: &Outputs, lib_path: String, fast_mark: String) {
    if STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    // what the thread needs, owned: the graph's script and package tables and the request
    let pkg: Vec<(String, String, String)> = g.pkg.clone();
    let scripts = script_table(g);
    let seeds = seeds.to_vec();
    let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
    let (workspace, package, relax_dir, triple, resolvable) = (req.workspace.to_string(), req.package.to_string(), req.relax_dir.to_string(), req.triple.to_string(), req.resolvable);
    let said = |o: &Outputs| -> Vec<(usize, bool, Vec<String>, Vec<(String, String)>)> {
        let mut v: Vec<_> = o.scripts.iter().map(|s| (s.krate, s.host, s.cfgs.clone(), s.env.clone())).collect();
        v.sort();
        v
    };
    let want = said(fast);
    let _ = relax_base::run_threads::spawn("run-native-opt", move || {
        let _z = crate::zone_alloc::compiler_thread();
        let flag_refs: Vec<&String> = flags.iter().collect();
        let m = match run_cargo_with(&pkg, &scripts, &workspace, &package, &relax_dir, &triple, &exe, &flag_refs, &script_pkgs, &lib_pkgs, &seeds, Tier::Optimized) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("makepad-run: --allow-c: the optimized native build failed (the opt-level 0 library stays): {}", e.lines().next().unwrap_or(""));
                return;
            }
        };
        let o = match outputs_with(&pkg, &scripts, &seeds, &m) {
            Ok(o) => o,
            Err(e) => { eprintln!("makepad-run: --allow-c: the optimized native build: {}", e); return; }
        };
        if said(&o) != want {
            eprintln!("makepad-run: --allow-c: the optimized native build's scripts said other cfgs or env than the first build's: its library is not taken");
            return;
        }
        let tmp = format!("{}.opt{}", lib_path, std::process::id());
        if link(&tmp, &o.archives, &o.search, &o.fw_search, &o.dylibs, &o.frameworks, &triple, resolvable).is_ok() && std::fs::rename(&tmp, &lib_path).is_ok() {
            let _ = std::fs::remove_file(&fast_mark);
            
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_cargo(g: &Graph, req: &Request, exe: &std::path::Path, flags: &[&String], script_pkgs: &[String], lib_pkgs: &[String], seeds: &[usize], tier: Tier) -> Result<String, String> {
    run_cargo_with(&g.pkg, &script_table(g), req.workspace, req.package, req.relax_dir, req.triple, exe, flags, script_pkgs, lib_pkgs, seeds, tier)
}

#[allow(clippy::too_many_arguments)]
fn run_cargo_with(pkg: &[(String, String, String)], scripts_of: &[(usize, Vec<String>)], workspace: &str, package: &str, relax_dir: &str, triple: &str, exe: &std::path::Path,
    flags: &[&String], script_pkgs: &[String], lib_pkgs: &[String], seeds: &[usize], tier: Tier) -> Result<String, String> {
    // Cargo does not track the wrapper's selection environment. An empty stub
    // from another selection must never count as a fresh real library.
    let selection = selection_key(script_pkgs, lib_pkgs);
    let target = match tier { Tier::Fast => format!("{}/native/cargo-target/{selection:016x}", relax_dir), Tier::Optimized => format!("{}/native/cargo-target-opt/{selection:016x}", relax_dir) };
    let target = std::path::absolute(&target).map_err(|e| format!("native target directory: {e}"))?;
    // Cargo can put intermediate files outside target_directory. Resolve its
    // configured build root, then isolate wrapper selections there as well.
    // Otherwise OUT_DIR no longer identifies target probes, and Cargo can
    // reuse stubs or nightly cfgs from a different native selection.
    let metadata = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", &target)
        .output().map_err(|e| format!("native build directory: {e}"))?;
    if !metadata.status.success() {
        return Err(format!("native build directory: {}", String::from_utf8_lossy(&metadata.stderr)));
    }
    let metadata = crate::json::parse(&metadata.stdout)?;
    let build = metadata.get("build_directory").map(|v| std::path::PathBuf::from(v.str()))
        .filter(|p| p != &target)
        .map(|p| p.join("makepad-native").join(match tier { Tier::Fast => "fast", Tier::Optimized => "optimized" }).join(format!("{selection:016x}")))
        .unwrap_or_else(|| target.clone());
    if tier == Tier::Fast {
        eprintln!("makepad run: --allow-c: building native dependencies with cargo (opt-level 0 now, optimized in the background; cached afterwards): {}", script_pkgs.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
    }
    let mut cmd = Command::new("cargo");
    // cargo runs in the real workspace: nothing of this run crosses
    crate::progenv::scrub_run_facts(&mut cmd);
    match tier {
        // the scripts see OPT_LEVEL=0 (cc and cmake compile the C unoptimized)
        Tier::Fast => { cmd.env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "0"); }
        // beside the program: two jobs
        Tier::Optimized => { cmd.env("CARGO_BUILD_JOBS", "2"); }
    }
    cmd.args(["check", "--release", "--message-format=json", "-p", package]);
    let cargo_flags = cargo_selection(flags);
    if !cargo_flags.iter().any(|f| f == "--target" || f.starts_with("--target=")) {
        cmd.args(["--target", triple]);
    }
    cmd.current_dir(workspace)
        .args(cargo_flags)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_BUILD_DIR", &build)
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTC_WRAPPER", exe)
        .env("MAKEPAD_RUN_NATIVE_SCRIPTS", script_pkgs.join(","))
        .env("MAKEPAD_RUN_NATIVE_LIBS", lib_pkgs.join(","))
        .env("RUSTC", exe)
        .env("MAKEPAD_RUN_NATIVE_RUSTC", "rustc")
        .env("MAKEPAD_RUN_NATIVE_TARGET", triple)
        .env("MAKEPAD_RUN_NATIVE_BUILD_DIR", &build)
        .env_remove("MAKEPAD_RUN_AS_RUSTC")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| format!("--allow-c needs cargo on PATH to build the native dependencies ({}): {e}", package))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        // cargo's own messages; a native script's failure is in `compiler-message`s and stderr
        let err = String::from_utf8_lossy(&out.stderr);
        let diagnostics: Vec<String> = stdout.lines().filter_map(|line| {
            let j = crate::json::parse(line.as_bytes()).ok()?;
            if j.get("reason")?.str() != "compiler-message" { return None; }
            let message = j.get("message")?;
            if message.get("level")?.str() != "error" { return None; }
            Some(message.get("rendered").filter(|s| !s.str().is_empty())
                .or_else(|| message.get("message"))?.str().to_string())
        }).collect();
        return Err(format!("--allow-c: cargo failed building the native dependencies ({}):\n{}\n{}", script_pkgs.join(", "), diagnostics.join("\n"), err));
    }
    // the feature cross-check, and the messages worth keeping. A package can be built twice, for
    // the host (build scripts, proc macros) and for the target, with features of their own
    // (resolver 2): each of our scripts' feature sets must be one cargo built.
    // Cargo emits both host and target outputs for a shared dependency. Keep
    // their cfgs/env separate: proc macros need host-feature outputs, and the
    // application needs target-feature outputs. Link only target archives when
    // both sides built the same native package.
    let target_scripts: Vec<String> = stdout.lines().filter_map(|line| {
        let j = crate::json::parse(line.as_bytes()).ok()?;
        if j.get("reason")?.str() != "build-script-executed" { return None; }
        is_target_out_dir(j.get("out_dir")?.str(), &build, triple)
            .then(|| j.get("package_id").map(|id| id.str().to_string())).flatten()
    }).collect();
    let mut keep = String::new();
    let mut cargo_sets: Vec<(String, String, Vec<String>)> = Vec::new();
    for line in stdout.lines() {
        let Ok(j) = crate::json::parse(line.as_bytes()) else { continue };
        let reason = j.get("reason").map(|r| r.str()).unwrap_or("");
        let (n, v) = id_name_version(j.get("package_id").map(|p| p.str()).unwrap_or(""));
        let seeded = seeds.iter().any(|&s| {
            let p = &pkg[scripts_of[s].0];
            p.1 == n && p.2 == v
        });
        if !seeded {
            continue;
        }
        match reason {
            "build-script-executed" => {
                let target_output = j.get("out_dir").is_some_and(|out| is_target_out_dir(out.str(), &build, triple));
                let host_output = !target_output && j.get("package_id").is_some_and(|id| target_scripts.iter().any(|p| p == id.str()));
                let body = line.trim_start().strip_prefix('{').ok_or("cargo build-script message is not an object")?;
                keep.push_str(if host_output { "{\"relax_host\":true," } else { "{\"relax_host\":false," });
                // the script run this message reports (`cached`): its OUT_DIR is shared with
                // other keys' builds of the selection
                let stamp = j.get("out_dir").map(|out| run_stamp(out.str())).unwrap_or_default();
                keep.push_str(&format!("\"relax_run\":\"{stamp}\","));
                keep.push_str(body);
                keep.push('\n');
            }
            "compiler-artifact" => {
                let is_lib = j.get("target").and_then(|t| t.get("kind")).is_some_and(|k| k.arr().iter().any(|x| x.str() == "lib" || x.str() == "rlib"));
                if is_lib {
                    let mut cargo_feats: Vec<String> = j.get("features").map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default();
                    cargo_feats.sort();
                    cargo_sets.push((n.clone(), v.clone(), cargo_feats));
                }
            }
            _ => {}
        }
    }
    for &s in seeds {
        let p = &pkg[scripts_of[s].0];
        let built: Vec<&Vec<String>> = cargo_sets.iter().filter(|c| c.0 == p.1 && c.1 == p.2).map(|c| &c.2).collect();
        if built.is_empty() {
            continue;
        }
        let mut ours = scripts_of[s].1.clone();
        ours.sort();
        if !built.iter().any(|c| **c == ours) {
            return Err(format!("--allow-c: cargo and Relax resolve `{} {}` with different features (cargo {:?}, relax {:?})", p.1, p.2, built, ours));
        }
    }
    Ok(keep)
}

fn selection_key(scripts: &[String], libs: &[String]) -> u64 {
    let mut h = fnv(b"target-compiler-probes-v4-build-dir", 0xcbf29ce484222325);
    for list in [scripts, libs] {
        h = fnv(b"\xff", h);
        for name in list { h = fnv(name.as_bytes(), h); h = fnv(b"\0", h); }
    }
    h
}

/// `native/lock`, held exclusively: a root is written, or a prune decides what goes. None when
/// it cannot be opened or (`wait` false) another process holds it.
fn native_lock(native: &std::path::Path, wait: bool) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(native.join("lock")).ok()?;
    let locked = if wait { f.lock().is_ok() } else { f.try_lock().is_ok() };
    locked.then_some(f)
}

/// Names `plan`'s build as the one its slot (module doc: roots) uses now. Under the native lock:
/// a prune either saw the root or was over before it was written, and then the build makes what
/// it needs again.
fn write_root(req: &Request, plan: &Plan) -> Result<(), String> {
    let native = std::path::Path::new(req.relax_dir).join("native");
    let roots = native.join("roots");
    let mut slot = fnv(b"native-root-v1", 0xcbf29ce484222325);
    for part in [req.workspace, req.package, req.triple].into_iter().chain(plan.flags.iter().map(String::as_str)) {
        slot = fnv(part.as_bytes(), slot);
        slot = fnv(b"\0", slot);
    }
    let path = roots.join(format!("{slot:016x}"));
    let text = format!("{}\n{:016x}\n{:016x}\n", req.workspace, plan.key, plan.selection);
    if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    std::fs::create_dir_all(&roots).map_err(|e| format!("{}: {e}", roots.display()))?;
    let _lock = native_lock(&native, true).ok_or_else(|| format!("{}: cannot lock", native.join("lock").display()))?;
    let tmp = roots.join(format!(".{slot:016x}.{}", std::process::id()));
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Keeps `native/` to what the roots name (module doc: roots): every other key's directory and
/// cargo selection goes, unless it changed since `since` (a build of a Relax without roots may
/// be in flight there), and so does the root of a workspace that is gone. Under the native lock,
/// taken only if free (else the next start prunes): what goes is moved to `native/trash` there
/// and deleted after it, so a run waiting to write its root waits for renames only. Names that
/// are not a key or selection (16 hex digits) are not the cache's and stay. Returns how many
/// directories went.
pub fn prune(native: &std::path::Path, since: std::time::SystemTime) -> usize {
    if !native.is_dir() {
        return 0;
    }
    let ours = |name: &str| name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit());
    let old = |p: &std::path::Path| std::fs::symlink_metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t < since);
    let trash = native.join("trash");
    let mut gone = 0;
    if let Some(_lock) = native_lock(native, false) {
        let mut keys = std::collections::HashSet::new();
        let mut selections = std::collections::HashSet::new();
        for e in std::fs::read_dir(native.join("roots")).into_iter().flatten().flatten() {
            let p = e.path();
            // (`.<slot>.<pid>`: a root being written, or what a writer that ended midway left)
            if e.file_name().to_string_lossy().starts_with('.') {
                if old(&p) {
                    let _ = std::fs::remove_file(&p);
                }
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            match text.lines().collect::<Vec<_>>()[..] {
                [workspace, key, selection] if std::path::Path::new(workspace).is_dir() => {
                    keys.insert(key.to_string());
                    selections.insert(selection.to_string());
                }
                // a workspace that is gone
                _ => { let _ = std::fs::remove_file(&p); }
            }
        }
        let mut doomed: Vec<(std::path::PathBuf, String)> = Vec::new();
        for e in std::fs::read_dir(native).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            if name == "cargo-target" || name == "cargo-target-opt" {
                for c in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                    let sel = c.file_name().to_string_lossy().into_owned();
                    if ours(&sel) && !selections.contains(&sel) && old(&c.path()) {
                        doomed.push((c.path(), format!("{name}.{sel}")));
                    }
                }
            } else if ours(&name) && !keys.contains(&name) && old(&p) {
                doomed.push((p, name));
            }
        }
        if !doomed.is_empty() && std::fs::create_dir_all(&trash).is_ok() {
            for (p, name) in doomed {
                if std::fs::rename(&p, trash.join(format!("{name}.{}", std::process::id()))).is_ok() {
                    gone += 1;
                }
            }
        }
    }
    // (also what a prune that ended midway left)
    for e in std::fs::read_dir(&trash).into_iter().flatten().flatten() {
        let _ = std::fs::remove_dir_all(e.path());
    }
    gone
}

/// The run's cargo flags as `cargo check` takes them. A test selection (`makepad test`: `--lib`,
/// `--bins`, `--tests`, `--test <name>`) resolves with the package's dev-dependencies, as `cargo
/// test` does (resolver 2 unifies their features when a target that needs them is built, cargo
/// core/resolver/features.rs `HasDevUnits`); `cargo check --lib` would not, so it becomes `cargo
/// check --tests`, which checks the test targets and resolves the same features.
fn cargo_selection(flags: &[&String]) -> Vec<String> {
    let test = flags.iter().any(|f| matches!(f.as_str(), "--lib" | "--bins" | "--tests" | "--test") || f.starts_with("--test="));
    if !test {
        return flags.iter().map(|f| f.to_string()).collect();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        match flags[i].as_str() {
            "--lib" | "--bins" | "--tests" => {}
            "--test" | "--bin" => i += 1,
            f if f.starts_with("--test=") || f.starts_with("--bin=") => {}
            f => out.push(f.to_string()),
        }
        i += 1;
    }
    out.push("--tests".to_string());
    out
}

/// The imports a linked native library binds by name at load time, as C names: on Mach-O the
/// ones its link found in no library (`nm -m`: "dynamically looked up"; one bound to libSystem
/// is its own), on ELF every undefined one (`nm -D`; ELF records no providing library), the
/// symbol version off.
fn library_imports(lib: &str) -> Result<Vec<String>, String> {
    let apple = cfg!(target_vendor = "apple");
    let mut nm = Command::new("nm");
    nm.args(if apple { ["-m", "-u"] } else { ["-D", "-u"] }).arg(lib);
    let out = nm.output().map_err(|e| format!("--allow-c: cannot read the native library's imports: {e}"))?;
    if !out.status.success() {
        return Err(format!("--allow-c: nm failed for {lib}: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut imports = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let name = if apple {
            // "(undefined) external _name (dynamically looked up)"
            let Some(rest) = line.trim().strip_suffix("(dynamically looked up)") else { continue };
            let Some(name) = rest.split_whitespace().last() else { continue };
            name.strip_prefix('_').unwrap_or(name)
        } else {
            // "                 U name@VERSION", "w name" (a weak one may stay unbound)
            let mut words = line.split_whitespace();
            let (Some("U"), Some(name)) = (words.next(), words.next()) else { continue };
            name.split('@').next().unwrap_or(name)
        };
        imports.insert(name.to_string());
    }
    Ok(imports.into_iter().collect())
}

fn link(lib_path: &str, archives: &[String], search: &[String], fw_search: &[String], dylibs: &[String], frameworks: &[String], triple: &str, resolvable: Resolvable) -> Result<(), String> {
    if let Some(d) = std::path::Path::new(lib_path).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = format!("{}.tmp{}", lib_path, std::process::id());
    let symbols_path = format!("{}.symbols", tmp);
    // the accessors' object (Linux): the callback relink below reuses the link's arguments
    let addresses_path = format!("{}.addresses.o", tmp);
    let mut cmd = Command::new(cc());
    if cfg!(target_os = "macos") {
        cmd.args(["-dynamiclib", "-o", &tmp, "-Wl,-undefined,dynamic_lookup"]);
        for a in archives {
            cmd.arg(format!("-Wl,-force_load,{}", a));
        }
    } else {
        cmd.args(["-shared", "-fPIC", "-o", &tmp]);
        if cfg!(target_os = "linux") {
            // Prebuilt assembly archives may omit .note.GNU-stack. Like rustc's
            // ELF links, keep the stack non-executable instead of inheriting an
            // executable-stack request that modern loaders reject for dlopen.
            cmd.arg("-Wl,-z,noexecstack");
            // All imports must be bound before the program starts.
            cmd.arg("-Wl,-z,now");
        }
        // Force each exported symbol through normal archive extraction. A merged
        // archive may contain the same object twice; --whole-archive would load
        // both copies and fail with duplicate definitions. Each symbol notes whether
        // it is thread-local (the address accessors below leave those out).
        let mut symbols = std::collections::BTreeMap::new();
        for archive in archives {
            let out = Command::new("nm").args(["-g", "--defined-only", "--format=sysv", archive])
                .output().map_err(|e| format!("--allow-c: cannot read archive symbols: {e}"))?;
            if !out.status.success() {
                return Err(format!("--allow-c: nm failed for {archive}: {}", String::from_utf8_lossy(&out.stderr)));
            }
            // name | value | class | type | size | line | section; the member and column headers have no `|`
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let columns: Vec<&str> = line.split('|').map(str::trim).collect();
                if columns.len() >= 4 && columns[2].len() == 1 && !columns[0].is_empty() {
                    symbols.insert(columns[0].to_string(), columns[3] == "TLS");
                }
            }
        }
        let args = symbols.keys().map(|name| {
            format!("--undefined=\"{}\"\n", name.replace('\\', "\\\\").replace('"', "\\\""))
        }).collect::<String>();
        std::fs::write(&symbols_path, args).map_err(|e| format!("{symbols_path}: {e}"))?;
        cmd.arg(format!("-Wl,@{symbols_path}"));
        if cfg!(target_os = "linux") {
            // Rust's static link can refer to hidden archive symbols, whereas dlsym cannot.
            // Export an address accessor for each (crate::native::address_name, which the
            // loader's lookup tries on a miss) without changing the symbols' own visibility.
            // Thread-local symbols get none: an accessor would answer the calling thread's
            // instance, and rustc links no plain extern static to a thread-local one. Names
            // that need assembler directives (versioned ones) stay on the ordinary dlsym path.
            let mut source = String::new();
            let plain = symbols.iter().filter(|(name, tls)| {
                !**tls && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b"_.".contains(&b))
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.$".contains(&b))
            });
            for (i, (name, _)) in plain.enumerate() {
                use std::fmt::Write;
                let escaped = name.bytes().map(|b| format!("\\{b:03o}")).collect::<String>();
                writeln!(source, "extern char s{i}[] __asm__(\"{escaped}\");").unwrap();
                writeln!(source, "__attribute__((visibility(\"default\"))) void *{}(void) {{ return s{i}; }}", address_name(name.as_bytes())).unwrap();
            }
            let source_path = format!("{tmp}.addresses.c");
            std::fs::write(&source_path, source).map_err(|e| format!("{source_path}: {e}"))?;
            let compile = Command::new(cc()).args(["-fPIC", "-c", &source_path, "-o", &addresses_path]).output();
            let _ = std::fs::remove_file(&source_path);
            let failed = match compile {
                Err(e) => Some(format!("cannot compile native address accessors: {e}")),
                Ok(out) if !out.status.success() => Some(format!("compiling native address accessors failed: {}", String::from_utf8_lossy(&out.stderr))),
                Ok(_) => None,
            };
            if let Some(e) = failed {
                let _ = std::fs::remove_file(&addresses_path);
                let _ = std::fs::remove_file(&symbols_path);
                return Err(format!("--allow-c: {e}"));
            }
            cmd.arg(&addresses_path);
        }
        cmd.args(archives);
        // what the archives' C code needs from the C runtime
        cmd.args(["-lm", "-lpthread", "-ldl"]);
    }
    for d in search {
        cmd.arg(format!("-L{}", d));
    }
    for d in fw_search {
        cmd.arg(format!("-F{}", d));
    }
    // JIT callers have no undefined symbols at this link step. Retain shared
    // libraries and every member of implicit static libraries for later lookup.
    if cfg!(target_os = "linux") {
        cmd.args(["-Wl,--no-as-needed", "-Wl,--whole-archive"]);
    }
    let mut linked = std::collections::HashSet::new();
    for l in dylibs {
        if linked.insert(l) {
            cmd.arg(format!("-l{}", l));
        }
    }
    if cfg!(target_os = "linux") {
        cmd.arg("-Wl,--no-whole-archive");
    }
    for f in frameworks {
        cmd.args(["-framework", f]);
    }
    let run = |cmd: &mut Command| -> Result<(), String> {
        let out = cmd.output().map_err(|e| format!("--allow-c: cannot run the C compiler to link the native library: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        Err(format!("--allow-c: linking the native library failed ({} archive(s): {}):\n{}", archives.len(), archives.join(" "), String::from_utf8_lossy(&out.stderr)))
    };
    let mut linked = run(&mut cmd);
    // C code calling back into Rust (crate::native_stub): an import the link left that no library
    // the loader binds defines is the program's. Its stub links in front, and the loader points it
    // at the program's `#[no_mangle]` fn of that name, or refuses to start.
    if linked.is_ok() {
        let stubs_path = format!("{}.callbacks.o", tmp);
        linked = library_imports(&tmp).and_then(|imports| {
            let resolved = resolvable(&imports, dylibs, frameworks);
            let callbacks: Vec<String> = imports.into_iter().zip(resolved).filter(|(_, r)| !r).map(|(n, _)| n).collect();
            if callbacks.is_empty() {
                return Ok(());
            }
            std::fs::write(&stubs_path, crate::native_stub::object(triple, &callbacks)?).map_err(|e| format!("{stubs_path}: {e}"))?;
            let mut with_stubs = Command::new(cc());
            with_stubs.arg(&stubs_path).args(cmd.get_args());
            run(&mut with_stubs)
        });
        let _ = std::fs::remove_file(&stubs_path);
    }
    let _ = std::fs::remove_file(&symbols_path);
    let _ = std::fs::remove_file(&addresses_path);
    if linked.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    linked?;
    std::fs::rename(&tmp, lib_path).map_err(|e| format!("{}: {}", lib_path, e))
}

// ---- cargo's RUSTC_WRAPPER

fn is_target_out_dir(out_dir: &str, cargo_target: &std::path::Path, triple: &str) -> bool {
    std::path::Path::new(out_dir).strip_prefix(cargo_target).ok()
        .and_then(|p| p.components().next()).is_some_and(|c| c.as_os_str() == triple)
}

/// Native execution does not change the compiler consuming a target script's output.
/// Cargo calls `<wrapper> <RUSTC> ...`; a script calls `<RUSTC> ...` directly.
/// An explicit target keeps Cargo's host and target script directories separate,
/// including packages used on both sides with identical features.
pub fn is_target_rustc_probe(args: &[String]) -> bool {
    if std::env::var_os("MAKEPAD_RUN_NATIVE_RUSTC").is_none() {
        return false;
    }
    let Some(first) = args.first() else { return false };
    if std::env::var("RUSTC").is_ok_and(|rustc| first == &rustc) {
        return false;
    }
    let (Ok(out), Some(target), Ok(triple)) = (std::env::var("OUT_DIR"), std::env::var_os("MAKEPAD_RUN_NATIVE_BUILD_DIR"), std::env::var("MAKEPAD_RUN_NATIVE_TARGET")) else { return false };
    is_target_out_dir(&out, std::path::Path::new(&target), &triple)
}

/// This executable as cargo's `RUSTC_WRAPPER` (`MAKEPAD_RUN_NATIVE_SCRIPTS` set) or as a stubbed
/// build script. Returns the exit code when it acted, None when this is an ordinary start.
pub fn wrapper() -> Option<i32> {
    let scripts = std::env::var("MAKEPAD_RUN_NATIVE_SCRIPTS").ok()?;
    let args: Vec<String> = std::env::args().collect();
    // a build script stub (cargo runs what the wrapper left as the script): nothing to do
    let stem = std::path::Path::new(&args[0]).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if std::env::var_os("OUT_DIR").is_some() && (stem.starts_with("build_script_") || stem.starts_with("build-script-")) {
        return Some(0);
    }
    // cargo runs `<wrapper> <rustc> <args>`
    if args.len() < 2 || std::env::var_os("RUSTC_WRAPPER").is_none() && std::env::var_os("CARGO_PKG_NAME").is_none() {
        return None;
    }
    let native_rustc = std::env::var("MAKEPAD_RUN_NATIVE_RUSTC").ok();
    let direct = native_rustc.is_some() && std::env::var("RUSTC").is_ok_and(|rustc| args[1] != rustc);
    let rustc = native_rustc.as_ref().unwrap_or(&args[1]);
    let rest = if direct { &args[1..] } else { &args[2..] };
    if direct {
        return Some(match Command::new(rustc).args(rest).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => { eprintln!("makepad run: native compiler probe: {}: {}", rustc, e); 1 }
        });
    }
    let value = |flag: &str| -> Option<String> {
        let mut i = 0;
        while i < rest.len() {
            if rest[i] == flag {
                return rest.get(i + 1).cloned();
            }
            if let Some(v) = rest[i].strip_prefix(&format!("{}=", flag)) {
                return Some(v.to_string());
            }
            i += 1;
        }
        None
    };
    let is_print = rest.iter().any(|a| a == "--print" || a.starts_with("--print=") || a == "-vV" || a == "--version");
    let name = value("--crate-name");
    let real = is_print || match &name {
        None => true,
        Some(n) if n == "___" => true,
        Some(n) => {
            let pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
            let list = |v: &str| std::env::var(v).unwrap_or_default().split(',').any(|x| x == pkg);
            if n.starts_with("build_script_") { scripts.split(',').any(|x| x == pkg) } else { list("MAKEPAD_RUN_NATIVE_LIBS") }
        }
    };
    if real {
        return Some(match Command::new(rustc).args(rest).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("makepad run: native wrapper: cannot run {}: {}", rustc, e);
                1
            }
        });
    }
    // a stub: the files cargo expects, empty
    let name = name.unwrap();
    let out_dir = value("--out-dir").unwrap_or_else(|| ".".to_string());
    let mut extra = String::new();
    let mut i = 0;
    while i < rest.len() {
        if let Some(v) = rest[i].strip_prefix("-Cextra-filename=") {
            extra = v.to_string();
        } else if rest[i] == "-C" {
            if let Some(v) = rest.get(i + 1).and_then(|x| x.strip_prefix("extra-filename=")) {
                extra = v.to_string();
            }
        }
        i += 1;
    }
    let types: Vec<String> = rest.iter().enumerate().filter_map(|(i, a)| if a == "--crate-type" { rest.get(i + 1).cloned() } else { a.strip_prefix("--crate-type=").map(str::to_string) }).collect();
    let emit = value("--emit").unwrap_or_default();
    let json = value("--json").unwrap_or_default().contains("artifacts");
    let dylib_ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let mut made: Vec<(String, &str)> = Vec::new();
    for e in emit.split(',') {
        match e.split('=').next().unwrap_or("") {
            "dep-info" => made.push((format!("{}/{}{}.d", out_dir, name, extra), "dep-info")),
            "metadata" => made.push((format!("{}/lib{}{}.rmeta", out_dir, name, extra), "metadata")),
            "link" => {
                for t in &types {
                    let f = match t.as_str() {
                        "bin" => format!("{}/{}{}", out_dir, name, extra),
                        "rlib" | "lib" => format!("{}/lib{}{}.rlib", out_dir, name, extra),
                        "staticlib" => format!("{}/lib{}{}.a", out_dir, name, extra),
                        _ => format!("{}/lib{}{}.{}", out_dir, name, extra, dylib_ext),
                    };
                    made.push((f, "link"));
                }
            }
            _ => {}
        }
    }
    let exe = std::env::current_exe().ok();
    for (path, kind) in &made {
        let _ = std::fs::create_dir_all(&out_dir);
        let is_bin = kind == &"link" && types.iter().any(|t| t == "bin") && path.ends_with(&format!("{}{}", name, extra));
        if is_bin {
            // the stub script is this executable (wrapper(): a build script stub exits at once)
            let _ = std::fs::remove_file(path);
            if let Some(e) = &exe {
                if std::fs::hard_link(e, path).is_err() {
                    let _ = std::fs::copy(e, path);
                }
            }
        } else if *kind == "dep-info" {
            let src = rest.iter().find(|a| a.ends_with(".rs")).cloned().unwrap_or_default();
            let _ = std::fs::write(path, format!("{}: {}\n", path, src));
        } else {
            let _ = std::fs::write(path, b"");
        }
        if json {
            eprintln!("{{\"$message_type\":\"artifact\",\"artifact\":\"{}\",\"emit\":\"{}\"}}", path.replace('\\', "\\\\").replace('"', "\\\""), kind);
        }
    }
    Some(0)
}

#[cfg(test)]
#[test]
fn test_selections_check_with_dev_dependencies() {
    let s = |v: &[&str]| -> Vec<String> {
        let owned: Vec<String> = v.iter().map(|x| x.to_string()).collect();
        cargo_selection(&owned.iter().collect::<Vec<_>>())
    };
    assert_eq!(s(&["--features", "a"]), ["--features", "a"]);
    assert_eq!(s(&["--bin", "x"]), ["--bin", "x"]);
    assert_eq!(s(&["--lib", "--features", "a"]), ["--features", "a", "--tests"]);
    assert_eq!(s(&["--bins", "--bin", "x"]), ["--tests"]);
    assert_eq!(s(&["--test", "it", "--test=two", "--no-default-features"]), ["--no-default-features", "--tests"]);
}

#[cfg(test)]
#[test]
fn script_selection_follows_every_unit_of_a_package() {
    // two units of `a` (host and target feature sets): the second one's dependency is a real build too
    let unit = |name: &str, deps: &[usize]| crate::unitgraph::RawUnit {
        name: name.to_string(), src_path: String::new(), edition: 2021, features: Vec::new(), proc_macro: false,
        deps: deps.iter().map(|&d| (d, String::new())).collect(), run_script: false,
        pkg: (String::new(), name.to_string(), "1.0.0".to_string()),
    };
    let raw = [unit("a", &[1]), unit("b", &[]), unit("a", &[3]), unit("c", &[]), unit("d", &[])];
    assert_eq!(dependency_packages(&raw, vec![0]), ["a", "b", "c"]);
    let build = std::path::Path::new("/w/build");
    assert!(is_target_out_dir("/w/build/x86_64-unknown-linux-gnu/release/build/a-1/out", build, "x86_64-unknown-linux-gnu"));
    assert!(!is_target_out_dir("/w/build/release/build/a-1/out", build, "x86_64-unknown-linux-gnu"));
    assert!(!is_target_out_dir("/elsewhere/x86_64-unknown-linux-gnu/out", build, "x86_64-unknown-linux-gnu"));
    assert_ne!(selection_key(&["a".into()], &["b".into()]), selection_key(&["a".into(), "b".into()], &[]));
}

#[cfg(test)]
#[test]
fn native_key_uses_sorted_resolved_packages() {
    let unit = |name: &str, version: &str, features: &[&str]| crate::unitgraph::RawUnit {
        name: name.into(), src_path: String::new(), edition: 2021,
        features: features.iter().map(|f| f.to_string()).collect(), proc_macro: false,
        deps: Vec::new(), run_script: false,
        pkg: (format!("packages/{name}"), name.into(), version.into()),
    };
    let a = unit("a", "1.0.0", &["fast", "std"]);
    let b = unit("b", "2.0.0", &[]);
    let key = resolved_packages_key(&[a.clone(), b.clone()]);
    assert_eq!(key, resolved_packages_key(&[b.clone(), unit("a", "1.0.0", &["std", "fast"]), a.clone()]),
        "unit order, duplicate targets and feature order must not change the key");
    assert_ne!(key, resolved_packages_key(&[a.clone(), unit("b", "2.0.1", &[])]),
        "a resolved version change must invalidate native artifacts");
    assert_ne!(key, resolved_packages_key(&[unit("a", "1.0.0", &["std"]), b.clone()]),
        "a resolved feature change must invalidate native artifacts");
    let mut other_source = a;
    other_source.pkg.0 = "other/a".into();
    assert_ne!(key, resolved_packages_key(&[other_source, b]),
        "a resolved source change must invalidate native artifacts");
}

#[cfg(test)]
#[test]
fn a_cached_cargo_run_stands_while_what_it_names_is_there() {
    let dir = std::env::temp_dir().join(format!("relax-native-cached-{}", std::process::id()));
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let lib = dir.join("libnative.so");
    let lib = lib.to_str().unwrap();
    let pkg = [(String::new(), "cfg_only".to_string(), "1.0.0".to_string())];
    let scripts = [(0, Vec::new())];
    let quoted = out_dir.to_str().unwrap().replace('\\', "\\\\").replace('"', "\\\"");
    // cargo's record of the script's run, beside its OUT_DIR
    std::fs::write(dir.join("output"), b"cargo:rustc-cfg=has_feature\n").unwrap();
    let run = run_stamp(out_dir.to_str().unwrap());
    assert!(!run.is_empty());
    let msg_run = |libs: &str, run: &str| format!(r#"{{"relax_host":false,"relax_run":"{run}","reason":"build-script-executed","package_id":"registry+https://example.test/index#cfg_only@1.0.0","linked_libs":{libs},"linked_paths":["native={quoted}"],"cfgs":["has_feature"],"env":[["GENERATED","yes"]],"out_dir":"{quoted}"}}"#);
    let msg = |libs: &str| msg_run(libs, &run);
    let hit = |m: &str| cached(&pkg, &scripts, &[0], m, lib).is_some();
    // a script that links nothing leaves no library to find: cargo is not run again
    assert!(hit(&msg("[]")), "a cfg/env-only script's run is reused without a library");
    // one that links something needs the linked library
    assert!(!hit(&msg(r#"["dylib=m"]"#)), "a system library to link, no library yet");
    assert!(!hit(&msg(r#"["static=gen"]"#)), "an archive missing from its search path");
    std::fs::write(out_dir.join("libgen.a"), b"!<arch>\n").unwrap();
    assert!(!hit(&msg(r#"["static=gen"]"#)), "an archive to link, no library yet");
    std::fs::write(lib, b"").unwrap();
    assert!(hit(&msg(r#"["static=gen"]"#)) && hit(&msg(r#"["dylib=m"]"#)), "the linked library is there");
    let m = msg("[]");
    assert!(!hit(&m[..m.len() - 1]), "a cut-off message file is no hit");
    // the OUT_DIR is shared with other keys' builds of the selection: a later run of the script
    // (under another key) left it as that run wrote it, so the receipt is no hit
    assert!(!hit(&msg_run("[]", "")), "a receipt that saw no run of the script");
    std::fs::write(dir.join("output"), b"cargo:rustc-cfg=other_feature\n").unwrap();
    assert!(!hit(&msg("[]")), "the script ran again since the receipt");
    assert!(hit(&msg_run("[]", &run_stamp(out_dir.to_str().unwrap()))), "the receipt of that later run");
    // generated sources live in OUT_DIR: a run whose OUT_DIR is gone is no hit, library or not
    std::fs::remove_dir_all(&out_dir).unwrap();
    assert!(!hit(&msg("[]")) && !hit(&msg(r#"["dylib=m"]"#)), "a removed OUT_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

/// relax issue 46 (the prune's units, after makepad/relax#47): key directories and both tiers'
/// cargo selections, kept by what the roots name, not by size.
#[cfg(test)]
#[test]
fn the_native_prune_keeps_what_the_roots_name() {
    let dir = std::env::temp_dir().join(format!("relax-native-prune-{}", std::process::id()));
    let native = dir.join("native");
    let unit = |rel: &str| -> std::path::PathBuf {
        let p = native.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("blob"), b"built").unwrap();
        p
    };
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(native.join("roots")).unwrap();
    let root = |name: &str, workspace: &std::path::Path, key: &str, selection: &str| {
        std::fs::write(native.join("roots").join(name), format!("{}\n{key}\n{selection}\n", workspace.display())).unwrap();
    };
    // two slots of a workspace (a run and a test of it: the second as a build writes it), and a
    // slot of a workspace that is gone
    root("00000000000000a1", &workspace, "aaaaaaaaaaaaaaaa", "1111111111111111");
    let relax_dir = dir.to_str().unwrap();
    let flags = vec!["--tests".to_string()];
    let req = Request { workspace: workspace.to_str().unwrap(), package: "app", cargo_flags: &flags, relax_dir, triple: "x86_64-unknown-linux-gnu", resolvable: |names, _, _| vec![false; names.len()] };
    let plan = Plan { script_pkgs: Vec::new(), lib_pkgs: Vec::new(), flags: flags.clone(), names: Vec::new(), key: 0xabababababababab, selection: 0x1111111111111111,
        dir: String::new(), messages_path: String::new(), lib_path: String::new(), fast_mark: String::new() };
    write_root(&req, &plan).unwrap();
    assert_eq!(std::fs::read_dir(native.join("roots")).unwrap().count(), 2, "one root file per slot, no temporary left");
    root("00000000000000b1", &dir.join("removed"), "bbbbbbbbbbbbbbbb", "2222222222222222");
    let named = [unit("aaaaaaaaaaaaaaaa"), unit("abababababababab"), unit("cargo-target/1111111111111111"), unit("cargo-target-opt/1111111111111111")];
    // a slot's superseded build, the gone workspace's build, a selection no root uses
    let unnamed = [unit("cccccccccccccccc"), unit("bbbbbbbbbbbbbbbb"), unit("cargo-target/2222222222222222"), unit("cargo-target-opt/3333333333333333")];
    // not the cache's: stays whatever its age
    let foreign = unit("not-a-key");
    // everything changed before `since`: what no root names goes
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    assert_eq!(prune(&native, later), unnamed.len());
    for p in &named { assert!(p.join("blob").is_file(), "a root names {}", p.display()); }
    for p in &unnamed { assert!(!p.exists(), "no root names {}", p.display()); }
    assert!(foreign.is_dir(), "a directory that is not a key or a selection");
    assert!(!native.join("roots/00000000000000b1").exists(), "the root of a workspace that is gone");
    assert_eq!(std::fs::read_dir(native.join("roots")).unwrap().count(), 2, "the workspace's two roots stay");
    assert_eq!(std::fs::read_dir(native.join("trash")).map_or(0, |d| d.count()), 0, "the trash is emptied");
    // a directory no root names yet, changed after `since` (a build of a Relax without roots in
    // flight): stays
    let fresh = unit("cargo-target/4444444444444444");
    let since = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    assert_eq!(prune(&native, since), 0);
    assert!(fresh.join("blob").is_file(), "a fresh selection");
    // the lock held (a root being written): the prune leaves everything for the next start
    let held = native_lock(&native, false).unwrap();
    assert_eq!(prune(&native, later), 0);
    assert!(fresh.is_dir());
    drop(held);
    assert_eq!(prune(&native, later), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&STARTED, std::sync::atomic::AtomicBool::new(false));
}
