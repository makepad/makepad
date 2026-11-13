//! `--allow-c`: native dependencies built by real cargo.
//!
//! The flag is the person's opt-in to C tooling: without it no package's C is compiled or linked
//! (neither cargo nor the system C compiler runs, and no native library is loaded). It is no
//! sandbox: `relax run` runs the package's Rust, its build scripts and proc macros included, with
//! or without it. A package cannot grant it to itself (its run-env may not set RELAX_ALLOW_C), and
//! a relax its program starts does not inherit it (progenv ALLOW_C). With it, the packages whose build
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
//! cargo flags, the resolved packages of the native closure (versions, features, sources; each
//! path package's files, a workspace build-dependency's too), the C compiler and the target --
//! not the package the run is for, so two packages whose native closure is the same share one
//! build, nor what the resolution says outside that closure. A run with an unchanged key starts
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
//! A build script runs as a `run --units` child of its own, which resolves the native packages its
//! build-dependencies reach but not the run's key (its unit graph is a part of the run's). So the
//! run writes what the native scripts produced (cfgs, env, OUT_DIR) and the library they were
//! linked into to a script receipt, named by its content, and hands it to its script children in
//! `RELAX_NATIVE_RECEIPT` (`write_receipt` / `read_receipt`). A child matches the entries by
//! the scripts' own paths, so a build script that calls into a native crate compiles and runs
//! with that crate's outputs and library, without cargo and without the flag itself.
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
    /// `--quiet` / `--silent` (a program's re-run, an integration test's CARGO_BIN_EXE child):
    /// no status line; the stderr is the program's
    pub quiet: bool,
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
/// import to (the C runtime and system libraries; the program's code is in none of them). A
/// library is the file the link took, from the build scripts' search folders `search` first.
pub type Resolvable = fn(names: &[String], dylibs: &[String], frameworks: &[String], search: &[String]) -> Vec<bool>;

/// What cargo's build script of one native package produced.
#[derive(Clone)]
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
#[derive(Clone)]
pub struct NativeLib {
    pub path: String,
    /// the names of the static libraries inside it (`#[link(name = .., kind = "static")]`)
    pub statics: Vec<String>,
    /// the native packages ("name version")
    pub crates: Vec<String>,
    /// the program links only some of `crates` (it took the library of the run that started it,
    /// linked for more packages): C of the others may call a `#[no_mangle]` fn it lacks, which
    /// stays unbound (native_stub.rs, relax issue 174)
    pub partial: bool,
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

/// Why a package's build script is native. One script can have several reasons; the strongest
/// one is the classification: a build-dependency that compiles C (`cc`, `cmake`, ..) needs
/// `--allow-c` whether or not the package also declares `links =`, which alone only passes
/// metadata to its dependents and runs under Relax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeReason {
    /// a build-dependency on one of [`BUILD_DEPS`] (its extern name)
    CompilesC(String),
    /// the script links a static archive it builds itself with another toolchain (a `swiftc`
    /// or `ar` it runs directly, no `cc` crate): a run cannot load an archive, so cargo builds
    /// the package like a `cc` one and the archive joins the native library
    StaticArchive,
    /// `links =` in the manifest
    Links,
    /// the script reads another package's `DEP_*` metadata
    DepMetadata,
}

impl NativeReason {
    /// The script runs a C compiler (or a native build tool): it cannot run without one, so
    /// without `--allow-c` a run refuses it and a check skips it.
    pub fn compiles_c(&self) -> bool {
        matches!(self, NativeReason::CompilesC(_) | NativeReason::StaticArchive)
    }

    /// `<package>: <reason>`, as the C gate's messages print it.
    pub fn describe(&self, pkg: &str) -> String {
        match self {
            NativeReason::CompilesC(dep) => format!("{}: build-dependency {}", pkg, dep),
            NativeReason::StaticArchive => format!("{}: links a static archive its script builds", pkg),
            NativeReason::Links => format!("{}: links =", pkg),
            NativeReason::DepMetadata => format!("{}: reads DEP_ metadata", pkg),
        }
    }
}

/// Why a package's build script is native, if it is (the strongest reason: [`NativeReason`]).
pub fn native_reason(g: &Graph, bs: &BuildScript) -> Option<NativeReason> {
    let (dir, name, _) = &g.pkg[bs.krate];
    let c_free = C_FREE_WITH_FEATURE.iter().any(|(p, f)| p == name && bs.features.iter().any(|x| x == f));
    for (_, ext) in &bs.build_deps {
        if BUILD_DEPS.contains(&ext.as_str()) && !c_free {
            return Some(NativeReason::CompilesC(ext.clone()));
        }
    }
    if links_static_archive(&crate::vfs::read_to_string(&bs.src_path).unwrap_or_default()) {
        return Some(NativeReason::StaticArchive);
    }
    let manifest = crate::vfs::read_to_string(format!("{}/Cargo.toml", dir)).unwrap_or_default();
    let links = manifest.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("links").is_some_and(|r| r.trim_start().starts_with('='))
    });
    if links {
        return Some(NativeReason::Links);
    }
    if reads_dep_metadata(bs) {
        return Some(NativeReason::DepMetadata);
    }
    None
}

/// Whether a build script's source prints a plain `rustc-link-lib=static=` (or `static:+mods=`)
/// directive. A `-bundle` static link names a library the toolchain already has (a compiler
/// runtime), not one the script built.
fn links_static_archive(src: &str) -> bool {
    src.match_indices("rustc-link-lib=static").any(|(i, m)| {
        let rest = &src[i + m.len()..];
        rest.starts_with('=') || (rest.starts_with(':') && !rest[1..].starts_with("-bundle"))
    })
}

#[cfg(test)]
mod static_archive_tests {
    use super::links_static_archive;

    #[test]
    fn a_script_built_archive_is_native_and_a_toolchain_runtime_is_not() {
        assert!(links_static_archive(r#"println!("cargo:rustc-link-lib=static=speech_bridge");"#));
        assert!(links_static_archive(r#"println!("cargo::rustc-link-lib=static:+whole-archive=foo");"#));
        assert!(!links_static_archive(r#"println!("cargo:rustc-link-lib=static:-bundle={rt}");"#));
        assert!(!links_static_archive(r#"println!("cargo:rustc-link-lib=framework=Speech");"#));
    }
}

/// Whether a build script reads `DEP_*` metadata (its source names such a variable).
pub fn reads_dep_metadata(bs: &BuildScript) -> bool {
    crate::vfs::read_to_string(&bs.src_path).unwrap_or_default().contains("DEP_")
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A path package's files, by the package's directory as the graph names it: a shared
/// workspace's (vfs.rs SHARE) is a folder mount's virtual path, on no disk, read in its checkout.
fn hash_package_dir(dir: &str, h: &mut u64) {
    hash_dir(std::path::Path::new(&*crate::vfs::on_disk(dir)), h);
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
        ((crate::dirs::anchored(&unit.pkg.0).into_owned(), unit.pkg.1.clone(), unit.pkg.2.clone()), features)
    }).collect();
    packages.sort();
    packages.dedup();
    fnv(format!("{packages:?}").as_bytes(), 0xcbf29ce484222325)
}

/// The directories of the path packages among the units of the closure `names` (sorted, each
/// once), but `natives` (hashed with the native packages): their files are what cargo compiles.
/// Registry, git and sysroot units are left out (RawUnit::path_source).
fn closure_path_dirs(raw: &[crate::unitgraph::RawUnit], names: &[String], natives: &[&str]) -> Vec<String> {
    let mut dirs: Vec<String> = raw.iter()
        .filter(|u| u.path_source && names.binary_search(&u.pkg.1).is_ok() && !natives.contains(&u.pkg.0.as_str()))
        .map(|u| u.pkg.0.clone())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
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
    // (v16: a receipt holds its scripts' declared variables, `cached`)
    let mut h = fnv(b"native-link-v16", 0xcbf29ce484222325);
    // the worktree: payloads still hold its absolute paths (Target B removes this per payload)
    h = fnv(&crate::dirs::anchors_key().to_le_bytes(), h);
    h = fnv(req.triple.as_bytes(), h);
    h = fnv(&selection.to_le_bytes(), h);
    for f in &flags {
        h = fnv(f.as_bytes(), h);
        h = fnv(b"\0", h);
    }
    h = fnv(&resolved_packages_key(g.raw.iter().filter(|u| script_pkgs.binary_search(&u.pkg.1).is_ok())).to_le_bytes(), h);
    // and the program's rustc flags, wherever cargo takes them from: the native library is linked
    // with the program's link arguments (`link_args`)
    // (values as the OS holds them: the environment need not be Unicode)
    let mut native_env: Vec<(String, Vec<u8>)> = std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.into_encoded_bytes())).filter(|(k, _)| {
        ["CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS", "PKG_CONFIG", "RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"]
            .iter().any(|prefix| k == prefix || k.starts_with(&format!("{prefix}_")))
            || (k.starts_with("CARGO_TARGET_") && (k.ends_with("_RUSTFLAGS") || k.ends_with("_LINKER")))
    }).collect();
    for (_, file) in crate::resolve::cargo_config_files(req.workspace) {
        // (vfs: a shared workspace's config is a folder mount's file)
        native_env.push((file.clone(), crate::vfs::read_to_string(&file).unwrap_or_default().into_bytes()));
    }
    native_env.sort();
    for (k, v) in native_env {
        h = fnv(k.as_bytes(), h);
        h = fnv(b"\0", h);
        h = fnv(&v, h);
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
            hash_package_dir(&dir, &mut h);
        }
    }
    // and the closure's other path packages, by their files: a workspace crate the native scripts
    // build-depend on is compiled into them (a registry or git package is its id, above)
    let native_dirs: Vec<&str> = seeds.iter().map(|&s| g.pkg[g.build_scripts[s].krate].0.as_str()).collect();
    for dir in closure_path_dirs(&g.raw, &script_pkgs, &native_dirs) {
        hash_package_dir(&dir, &mut h);
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
    let exe = crate::dirs::loader_exe().map_err(|e| format!("this executable: {e}"))?;
    let plan = plan(g, seeds, req);
    // named before anything is read or built under it: a prune from now on keeps it
    write_root(req, &plan)?;
    let flags: Vec<&String> = plan.flags.iter().collect();
    let out = match std::fs::read_to_string(&plan.messages_path).ok().and_then(|m| cached(&g.pkg, &script_table(g), seeds, &m, &plan.lib_path)) {
        Some(out) => out,
        None => {
            let m = run_cargo(g, req, &exe, &flags, &plan.script_pkgs, &plan.lib_pkgs, seeds, Tier::Fast)?;
            let out = outputs(g, seeds, &m)?;
            std::fs::create_dir_all(&plan.dir).map_err(|e| format!("{}: {}", plan.dir, e))?;
            // an earlier build's library and receipt under this key stay until this build's library
            // has linked (link writes it beside, then renames it over the old one): an executable
            // prepared with them still runs when this link fails, and no receipt is ever newer than
            // the library beside it (relax issue 160: the library went before a link that failed)
            if !(out.archives.is_empty() && out.dylibs.is_empty() && out.frameworks.is_empty()) {
                link(&plan.lib_path, &out, req.triple, req.resolvable)?;
            }
            std::fs::write(&plan.fast_mark, b"").map_err(|e| format!("{}: {}", plan.fast_mark, e))?;
            write_beside(&plan.messages_path, m.as_bytes())?;
            out
        }
    };
    if out.archives.is_empty() && out.dylibs.is_empty() && out.frameworks.is_empty() {
        return Ok(Built { scripts: out.scripts, lib: None });
    }
    if std::path::Path::new(&plan.fast_mark).is_file() {
        optimize_later(g, req, exe, &flags, plan.script_pkgs, plan.lib_pkgs, seeds, &out, plan.lib_path.clone(), plan.fast_mark);
    }
    Ok(Built { scripts: out.scripts, lib: Some(NativeLib { path: plan.lib_path, statics: out.statics, crates: plan.names, partial: false }) })
}

/// The script outputs of the native build already cached under `req`'s key (no cargo, no link):
/// the cfgs, env and OUT_DIR a check compiles the native packages with. `None` when nothing is
/// cached, when the receipt does not cover every seed (a partial build is not a build), or when
/// the directories it names are gone. `cached` rejects the receipt of a build that did not link
/// or whose library the target no longer holds, so only a whole build is read back as one.
pub fn cached_scripts(g: &Graph, seeds: &[usize], req: &Request) -> Option<Vec<ScriptOut>> {
    cached_outputs(g, seeds, req).map(|(scripts, _)| scripts)
}

/// `cached_scripts` and the library the cached build linked (None when it linked nothing): what a
/// build script's own child run calls into.
pub fn cached_outputs(g: &Graph, seeds: &[usize], req: &Request) -> Option<(Vec<ScriptOut>, Option<NativeLib>)> {
    let plan = plan(g, seeds, req);
    let messages = std::fs::read_to_string(&plan.messages_path).ok()?;
    let out = cached(&g.pkg, &script_table(g), seeds, &messages, &plan.lib_path)?;
    for &s in seeds {
        let (_, n, v) = &g.pkg[g.build_scripts[s].krate];
        if !out.scripts.iter().any(|o| g.pkg[o.krate].1 == *n && g.pkg[o.krate].2 == *v) {
            return None;
        }
    }
    let links = !(out.archives.is_empty() && out.dylibs.is_empty() && out.frameworks.is_empty());
    let lib = links.then(|| NativeLib { path: plan.lib_path.clone(), statics: out.statics.clone(), crates: plan.names.clone(), partial: false });
    Some((out.scripts, lib))
}

/// A script receipt (`write_receipt`): the native scripts' outputs by script path, and the library.
pub struct Receipt {
    pub scripts: Vec<ReceiptScript>,
    pub lib: Option<NativeLib>,
}

/// One native script's outputs in a script receipt, by the script's `src_path`: the key a build
/// script's own child run can match (its unit graph is a part of the run's, so it cannot
/// compute the run's key).
pub struct ReceiptScript {
    pub path: String,
    pub host: bool,
    pub out_dir: String,
    pub cfgs: Vec<String>,
    pub env: Vec<(String, String)>,
    pub system: Vec<(String, bool)>,
}

/// Writes the native scripts' outputs (those of `seeds`) and the library for the run's own
/// build-script children: `<dir>/native-scripts-<hash of the content>.json`, so equal outputs
/// are one file whose name says nothing about the run that wrote it (a script's rerun key holds
/// the name, and a name per run would run every script again on every run). Its path.
pub fn write_receipt(g: &Graph, seeds: &[usize], scripts: &[ScriptOut], lib: Option<&NativeLib>, dir: &str) -> Result<String, String> {
    let list = |items: &mut dyn Iterator<Item = String>| format!("[{}]", items.collect::<Vec<_>>().join(","));
    let mut entries = Vec::new();
    for &s in seeds {
        let bs = &g.build_scripts[s];
        let (_, name, version) = &g.pkg[bs.krate];
        for o in scripts.iter().filter(|o| o.krate == bs.krate || (g.pkg[o.krate].1 == *name && g.pkg[o.krate].2 == *version)) {
            entries.push(format!(
                "{{\"path\":{},\"host\":{},\"out_dir\":{},\"cfgs\":{},\"env\":{},\"system\":{}}}",
                json_string(&bs.src_path),
                o.host,
                json_string(&o.out_dir),
                list(&mut o.cfgs.iter().map(|c| json_string(c))),
                list(&mut o.env.iter().map(|(k, v)| format!("[{},{}]", json_string(k), json_string(v)))),
                list(&mut o.system.iter().map(|(k, framework)| format!("[{},{}]", json_string(k), framework))),
            ));
        }
    }
    let mut out = format!("{{\"scripts\":[{}]", entries.join(","));
    if let Some(lib) = lib {
        out.push_str(&format!(
            ",\"lib\":{{\"path\":{},\"statics\":{},\"crates\":{}}}",
            json_string(&lib.path),
            list(&mut lib.statics.iter().map(|n| json_string(n))),
            list(&mut lib.crates.iter().map(|n| json_string(n))),
        ));
    }
    out.push('}');
    let path = format!("{}/native-scripts-{:016x}.json", dir, fnv(out.as_bytes(), 0xcbf29ce484222325));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(out.as_str()) {
        write_beside(&path, out.as_bytes())?;
    }
    Ok(path)
}

/// Writes the file `path` beside it, then renames it over `path`: a reader sees the old file
/// whole or the new one, and a write that fails leaves the old one.
fn write_beside(path: &str, data: &[u8]) -> Result<(), String> {
    let tmp = format!("{}.tmp{}", path, std::process::id());
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {}", path, e)
    })
}

/// Whether `path` names a receipt as a run wrote it into the run dir `dir` (`write_receipt`): in
/// that folder, named by its content. A receipt named by the environment is taken only so (relax
/// issue 168); which native packages it covers is matched after (`inherited_scripts`).
pub fn receipt_in(path: &str, dir: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let (Some(Ok(at)), Ok(run_dir)) = (p.parent().map(std::path::Path::canonicalize), std::path::Path::new(dir).canonicalize()) else {
        return Err(format!("{path} is not in the run dir {dir}"));
    };
    if at != run_dir {
        return Err(format!("{path} is not in the run dir {dir}"));
    }
    let text = std::fs::read(p).map_err(|e| format!("{path}: {e}"))?;
    if p.file_name().and_then(|n| n.to_str()) != Some(format!("native-scripts-{:016x}.json", fnv(&text, 0xcbf29ce484222325)).as_str()) {
        return Err(format!("{path} is not a receipt as a run wrote it"));
    }
    Ok(())
}

/// A script receipt (`write_receipt`), or None when there is none to use: unreadable, or naming an
/// OUT_DIR or a library that is gone.
pub fn read_receipt(path: &str) -> Option<Receipt> {
    let text = std::fs::read_to_string(path).ok()?;
    let j = crate::json::parse(text.as_bytes()).ok()?;
    let strs = |v: Option<&crate::json::Json>| -> Vec<String> { v.map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default() };
    let mut scripts = Vec::new();
    for s in j.get("scripts")?.arr() {
        let out_dir = s.get("out_dir").map(|v| v.str().to_string()).unwrap_or_default();
        if !out_dir.is_empty() && !std::path::Path::new(&out_dir).is_dir() {
            return None;
        }
        let pairs = |k: &str| -> Vec<(&crate::json::Json, &crate::json::Json)> {
            s.get(k).map(|a| a.arr().iter().filter_map(|kv| match kv.arr() { [k, v] => Some((k, v)), _ => None }).collect()).unwrap_or_default()
        };
        scripts.push(ReceiptScript {
            path: s.get("path")?.str().to_string(),
            host: matches!(s.get("host"), Some(crate::json::Json::Bool(true))),
            out_dir,
            cfgs: strs(s.get("cfgs")),
            env: pairs("env").into_iter().map(|(k, v)| (k.str().to_string(), v.str().to_string())).collect(),
            system: pairs("system").into_iter().map(|(k, v)| (k.str().to_string(), matches!(v, crate::json::Json::Bool(true)))).collect(),
        });
    }
    let lib = match j.get("lib") {
        Some(l) => {
            let path = l.get("path")?.str().to_string();
            if !std::path::Path::new(&path).is_file() {
                return None;
            }
            Some(NativeLib { path, statics: strs(l.get("statics")), crates: strs(l.get("crates")), partial: false })
        }
        None => None,
    };
    Some(Receipt { scripts, lib })
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
    /// each variable a script declared (`rerun-if-env-changed`) and its value when it ran (None:
    /// unset), as `declared_env` read them
    env: Vec<(String, Option<String>)>,
    statics: Vec<String>,
    archives: Vec<String>,
    search: Vec<String>,
    fw_search: Vec<String>,
    dylibs: Vec<String>,
    frameworks: Vec<String>,
    /// the program's link arguments (`-C link-arg`s of its binary's rustc command: RUSTFLAGS, the
    /// cargo config's rustflags, its build scripts' `rustc-link-arg`s), as cargo gave them
    link_args: Vec<String>,
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
    // a variable a script reads is one of its inputs, as cargo has it: another value runs it again
    // (relax review B-M5: OPENSSL_DIR pointed at another OpenSSL, and the library built against
    // the old one stayed linked)
    let same_env = out.env.iter().all(|(k, v)| std::env::var(k).ok() == *v);
    (there && same_runs && same_env).then_some(out)
}

/// The variables the script run that left `out_dir` declared with `rerun-if-env-changed` (cargo's
/// `output` file beside it), each once, with this process's value (None: unset): cargo gives the
/// script this environment.
fn declared_env(out_dir: &str) -> Vec<(String, Option<String>)> {
    let output = std::fs::read_to_string(std::path::Path::new(out_dir).with_file_name("output")).unwrap_or_default();
    let mut names: Vec<&str> = output.lines().filter_map(|l| {
        l.strip_prefix("cargo::").or_else(|| l.strip_prefix("cargo:"))?.strip_prefix("rerun-if-env-changed=")
    }).map(str::trim_end).collect();
    names.sort();
    names.dedup();
    names.into_iter().map(|k| (k.to_string(), std::env::var(k).ok())).collect()
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
    // each static library with the search paths of the script that named it: the copy that script
    // built (a bundled sqlite in its OUT_DIR) wins over one of the same name in a path another
    // script added (on Linux, the system's non-PIC libsqlite3.a in the path fontconfig's script adds)
    let mut static_homes: Vec<(String, Vec<String>)> = Vec::new();
    let mut archives: Vec<String> = Vec::new();
    let mut search: Vec<String> = Vec::new();
    let mut fw_search: Vec<String> = Vec::new();
    let mut dylibs: Vec<String> = Vec::new();
    let mut frameworks: Vec<String> = Vec::new();
    let mut runs: Vec<(String, String)> = Vec::new();
    let mut env_seen: Vec<(String, Option<String>)> = Vec::new();
    let mut link_args: Vec<String> = Vec::new();
    for line in messages.lines() {
        let j = crate::json::parse(line.as_bytes()).map_err(|e| format!("cargo message: {e}"))?;
        if let Some(a) = j.get("relax_link_args") {
            link_args = a.arr().iter().map(|x| x.str().to_string()).collect();
            continue;
        }
        let (n, v) = id_name_version(j.get("package_id").map(|p| p.str()).unwrap_or(""));
        let Some(&s) = seeds.iter().find(|&&s| {
            let p = &pkg[scripts_of[s].0];
            p.1 == n && p.2 == v
        }) else { continue };
        let strs = |k: &str| -> Vec<String> { j.get(k).map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default() };
        let mut out = ScriptOut { krate: scripts_of[s].0, host: matches!(j.get("relax_host"), Some(crate::json::Json::Bool(true))), cfgs: strs("cfgs"), env: Vec::new(), out_dir: j.get("out_dir").map(|o| o.str().to_string()).unwrap_or_default(), system: Vec::new() };
        runs.push((out.out_dir.clone(), j.get("relax_run").map(|r| r.str().to_string()).unwrap_or_default()));
        for kv in j.get("relax_env").map(|e| e.arr()).unwrap_or(&[]) {
            if let [k, v] = kv.arr() {
                env_seen.push((k.str().to_string(), matches!(v, crate::json::Json::Str(_)).then(|| v.str().to_string())));
            }
        }
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
        let mut own_search: Vec<String> = Vec::new();
        for p in strs("linked_paths") {
            match p.split_once('=') {
                Some(("framework", d)) => fw_search.push(d.to_string()),
                Some((_, d)) => {
                    search.push(d.to_string());
                    own_search.push(d.to_string());
                }
                None => {
                    search.push(p.clone());
                    own_search.push(p);
                }
            }
        }
        for l in strs("linked_libs") {
            let (kind, name) = match l.split_once('=') {
                Some((k, n)) => (k.to_string(), n.split(':').next().unwrap_or(n).to_string()),
                None => (String::new(), l.clone()),
            };
            match kind.as_str() {
                "static" => {
                    static_homes.push((name.clone(), own_search.clone()));
                    statics.push(name);
                }
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
    for (name, own_search) in &static_homes {
        let file = format!("lib{}.a", name);
        match own_search.iter().chain(search.iter()).map(|d| format!("{}/{}", d, file)).find(|p| std::path::Path::new(p).is_file()) {
            Some(p) => {
                if !archives.contains(&p) {
                    archives.push(p);
                }
            }
            None => return Err(format!("--allow-c: a native build script links static library `{}`, but {} is in none of its search paths ({})", name, file, search.join(", "))),
        }
    }
    Ok(Outputs { scripts, runs, env: env_seen, statics, archives, search, fw_search, dylibs, frameworks, link_args })
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
                eprintln!("relax: --allow-c: the optimized native build failed (the opt-level 0 library stays): {}", e.lines().next().unwrap_or(""));
                return;
            }
        };
        let o = match outputs_with(&pkg, &scripts, &seeds, &m) {
            Ok(o) => o,
            Err(e) => { eprintln!("relax: --allow-c: the optimized native build: {}", e); return; }
        };
        if said(&o) != want {
            eprintln!("relax: --allow-c: the optimized native build's scripts said other cfgs or env than the first build's: its library is not taken");
            return;
        }
        let tmp = format!("{}.opt{}", lib_path, std::process::id());
        if link(&tmp, &o, &triple, resolvable).is_ok() && std::fs::rename(&tmp, &lib_path).is_ok() {
            let _ = std::fs::remove_file(&fast_mark);
            
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_cargo(g: &Graph, req: &Request, exe: &std::path::Path, flags: &[&String], script_pkgs: &[String], lib_pkgs: &[String], seeds: &[usize], tier: Tier) -> Result<String, String> {
    if tier == Tier::Fast && !req.quiet {
        eprintln!("relax run: --allow-c: building native dependencies with cargo (opt-level 0 now, optimized in the background; cached afterwards): {}", script_pkgs.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
    }
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
        .current_dir(&*crate::vfs::on_disk(workspace))
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
    // the program's link arguments: the wrapper writes them here, a file per binary or test
    // harness. A folder of its own per cargo run: the program's stubs depend on its name
    // (wrapper), so cargo runs them again, and they say the link arguments, even when nothing else
    // changed
    let run_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let link_dir = build.join(format!("relax-link-args.{}.{run_id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&link_dir);
    std::fs::create_dir_all(&link_dir).map_err(|e| format!("{}: {e}", link_dir.display()))?;
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
    // (a shared workspace's folder is a folder mount, vfs.rs: cargo builds in its checkout)
    cmd.current_dir(&*crate::vfs::on_disk(workspace))
        .args(cargo_flags)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_BUILD_DIR", &build)
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTC_WRAPPER", exe)
        .env("RELAX_NATIVE_SCRIPTS", script_pkgs.join(","))
        .env("RELAX_NATIVE_LIBS", lib_pkgs.join(","))
        .env("RUSTC", exe)
        .env("RELAX_NATIVE_RUSTC", "rustc")
        .env("RELAX_NATIVE_TARGET", triple)
        .env("RELAX_NATIVE_BUILD_DIR", &build)
        .env("RELAX_NATIVE_LINK_ARGS", &link_dir)
        .env_remove("RELAX_AS_RUSTC")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output();
    let link_args = program_link_args(&link_dir, package, flags);
    let _ = std::fs::remove_dir_all(&link_dir);
    let out = out.map_err(|e| format!("--allow-c needs cargo on PATH to build the native dependencies ({}): {e}", package))?;
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
                // and the variables it declared (`rerun-if-env-changed`) with the values it ran
                // with (`cached`)
                let declared = j.get("out_dir").map(|out| declared_env(out.str())).unwrap_or_default();
                let pairs: Vec<String> = declared.iter().map(|(k, v)| {
                    format!("[{},{}]", json_string(k), v.as_deref().map_or("null".to_string(), json_string))
                }).collect();
                keep.push_str(&format!("\"relax_env\":[{}],", pairs.join(",")));
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
    keep.push_str(&format!("{{\"relax_link_args\":[{}]}}\n", link_args.iter().map(|a| json_string(a)).collect::<Vec<_>>().join(",")));
    Ok(keep)
}

/// The link arguments the wrapper saw on the rustc command of the program (`wrapper`, a file per
/// crate of the package's binaries and test harnesses): the binary `--bin` names, else the crate
/// named after the package, else what they all say alike.
fn program_link_args(dir: &std::path::Path, package: &str, flags: &[&String]) -> Vec<String> {
    let units: Vec<(String, String)> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.to_string(), std::fs::read_to_string(e.path()).ok()?))).collect();
    let named = flags.iter().position(|f| f.as_str() == "--bin").and_then(|i| flags.get(i + 1)).map(|b| b.as_str())
        .or_else(|| flags.iter().find_map(|f| f.strip_prefix("--bin=")))
        .unwrap_or(package).replace('-', "_");
    let alike = units.first().filter(|(_, t)| units.iter().all(|(_, u)| u == t));
    let text = units.iter().find(|(n, _)| *n == named).or(alike).map_or("", |(_, t)| t.as_str());
    text.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The `-C link-arg=` / `-C link-args=` values of a rustc command line, in order.
fn link_args_of(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let value = if a == "-C" || a == "--codegen" {
            i += 1;
            args.get(i).map(String::as_str)
        } else {
            a.strip_prefix("-C").or_else(|| a.strip_prefix("--codegen="))
        };
        if let Some(v) = value {
            if let Some(x) = v.strip_prefix("link-arg=") {
                out.push(x.to_string());
            } else if let Some(x) = v.strip_prefix("link-args=") {
                out.extend(x.split_whitespace().map(str::to_string));
            }
        }
        i += 1;
    }
    out
}

/// `s` as a JSON string literal.
fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn selection_key(scripts: &[String], libs: &[String]) -> u64 {
    let mut h = fnv(b"target-compiler-probes-v5-link-args", 0xcbf29ce484222325);
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
/// it needs again. A shared workspace's (a folder mount, vfs.rs) is its checkout's on disk: each
/// checkout has its slot, and `prune` finds the folder.
fn write_root(req: &Request, plan: &Plan) -> Result<(), String> {
    let native = std::path::Path::new(req.relax_dir).join("native");
    let roots = native.join("roots");
    let workspace = crate::vfs::on_disk(req.workspace);
    let mut slot = fnv(b"native-root-v1", 0xcbf29ce484222325);
    for part in [&*workspace, req.package, req.triple].into_iter().chain(plan.flags.iter().map(String::as_str)) {
        slot = fnv(part.as_bytes(), slot);
        slot = fnv(b"\0", slot);
    }
    let path = roots.join(format!("{slot:016x}"));
    let text = format!("{}\n{:016x}\n{:016x}\n", workspace, plan.key, plan.selection);
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
/// taken only if free (else the next start prunes), or with `wait` once it is: what goes is moved
/// to `native/trash` there and deleted after it, so a run waiting to write its root waits for
/// renames only. Names that are not a key or selection (16 hex digits) are not the cache's and
/// stay. Returns how many directories went.
/// A child being spawned holds the lock too, from its fork to its exec, when it was forked while
/// the lock was held: an flock belongs to the open file, which the child's copy of the descriptor
/// keeps. So right after its holder let go the lock can still be taken for that moment; a caller
/// that needs the prune done (a test, while other tests spawn) waits.
pub fn prune(native: &std::path::Path, since: std::time::SystemTime, wait: bool) -> usize {
    if !native.is_dir() {
        return 0;
    }
    let ours = |name: &str| name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit());
    let old = |p: &std::path::Path| std::fs::symlink_metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t < since);
    let trash = native.join("trash");
    let mut gone = 0;
    if let Some(_lock) = native_lock(native, wait) {
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

/// What a native library's C code needs from the C runtime beyond what the C compiler links by
/// default, on `triple`: on Linux (and the other ELF systems) its math, threads and dynamic
/// loading are libraries of their own. mingw-w64's gcc links its C runtime by default and has no
/// libdl, so `-ldl` failed every Windows link (relax issue 155: "cannot find -ldl"); Apple's
/// libSystem holds them all (its link never asked).
fn c_runtime_libs(triple: &str) -> &'static [&'static str] {
    if triple.contains("-windows") || triple.contains("-apple-") {
        &[]
    } else {
        &["-lm", "-lpthread", "-ldl"]
    }
}

fn link(lib_path: &str, out: &Outputs, triple: &str, resolvable: Resolvable) -> Result<(), String> {
    let (archives, search, fw_search, dylibs, frameworks) = (&out.archives, &out.search, &out.fw_search, &out.dylibs, &out.frameworks);
    if let Some(d) = std::path::Path::new(lib_path).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = format!("{}.tmp{}", lib_path, std::process::id());
    let symbols_path = format!("{}.symbols", tmp);
    // the accessors' object: the callback relink below reuses the link's arguments
    let addresses_path = format!("{}.addresses.o", tmp);
    let mut cmd = Command::new(cc());
    if cfg!(target_os = "macos") {
        cmd.args(["-dynamiclib", "-o", &tmp, "-Wl,-undefined,dynamic_lookup"]);
        for a in archives {
            cmd.arg(format!("-Wl,-force_load,{}", a));
        }
        // C built with -fvisibility=hidden and assembly's `.private_extern` (ring's core) stay
        // local to the library, where dlsym cannot see them: the same address accessors as Linux
        compile_accessors(&macho_archive_symbols(archives)?, &tmp, &addresses_path)?;
        cmd.arg(&addresses_path);
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
            if let Err(e) = compile_accessors(&symbols, &tmp, &addresses_path) {
                let _ = std::fs::remove_file(&symbols_path);
                return Err(e);
            }
            cmd.arg(&addresses_path);
        }
        cmd.args(archives);
        cmd.args(c_runtime_libs(triple));
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
    // the program's own link arguments, as rustc gives them to the linker for its binary (an rpath
    // that finds a library of the scripts' search folders at load time)
    cmd.args(&out.link_args);
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
            let resolved = resolvable(&imports, dylibs, frameworks, search);
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

/// Rust's static link can refer to hidden archive symbols, whereas dlsym cannot. This compiles an
/// exported address accessor for each of `symbols` (crate::native::address_name, which the
/// loader's lookup tries on a miss) into `addresses_path`, without changing the symbols' own
/// visibility. `symbols`: each defined global symbol as the object files spell it (Mach-O with
/// its leading `_`), and whether it is thread-local. Thread-local symbols get none: an accessor
/// would answer the calling thread's instance, and rustc links no plain extern static to a
/// thread-local one. Names that need assembler directives (versioned ones) stay on the ordinary
/// dlsym path, and on Mach-O so does a name with no C spelling (no leading `_`).
fn compile_accessors(symbols: &std::collections::BTreeMap<String, bool>, tmp: &str, addresses_path: &str) -> Result<(), String> {
    use std::fmt::Write;
    let mut source = String::new();
    let plain = symbols.iter().filter(|(name, tls)| {
        !**tls && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b"_.".contains(&b))
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.$".contains(&b))
    });
    for (i, (name, _)) in plain.enumerate() {
        // the name dlsym is asked for: Mach-O spells C names with a leading `_`
        let c_name = if cfg!(target_vendor = "apple") {
            match name.strip_prefix('_') {
                Some(n) => n,
                None => continue,
            }
        } else {
            name.as_str()
        };
        let escaped = name.bytes().map(|b| format!("\\{b:03o}")).collect::<String>();
        writeln!(source, "extern char s{i}[] __asm__(\"{escaped}\");").unwrap();
        writeln!(source, "__attribute__((visibility(\"default\"))) void *{}(void) {{ return s{i}; }}", address_name(c_name.as_bytes())).unwrap();
    }
    let source_path = format!("{tmp}.addresses.c");
    std::fs::write(&source_path, source).map_err(|e| format!("{source_path}: {e}"))?;
    let compile = Command::new(cc()).args(["-fPIC", "-c", &source_path, "-o", addresses_path]).output();
    let _ = std::fs::remove_file(&source_path);
    let failed = match compile {
        Err(e) => Some(format!("cannot compile native address accessors: {e}")),
        Ok(out) if !out.status.success() => Some(format!("compiling native address accessors failed: {}", String::from_utf8_lossy(&out.stderr))),
        Ok(_) => None,
    };
    if let Some(e) = failed {
        let _ = std::fs::remove_file(addresses_path);
        return Err(format!("--allow-c: {e}"));
    }
    Ok(())
}

/// The defined global symbols of Mach-O archives (`nm -g --defined-only -m`: one
/// `<value> (<segment>,<section>) [private ]external <name>` line each; the member headers have no
/// section), thread-local when its section is one of `__thread_vars` / `_data` / `_bss`.
fn macho_archive_symbols(archives: &[String]) -> Result<std::collections::BTreeMap<String, bool>, String> {
    let mut symbols = std::collections::BTreeMap::new();
    for archive in archives {
        let out = Command::new("nm").args(["-g", "--defined-only", "-m", archive])
            .output().map_err(|e| format!("--allow-c: cannot read archive symbols: {e}"))?;
        if !out.status.success() {
            return Err(format!("--allow-c: nm failed for {archive}: {}", String::from_utf8_lossy(&out.stderr)));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some(open) = line.find(" (") else { continue };
            let Some(close) = line[open..].find(") ").map(|k| open + k) else { continue };
            if !line[..open].bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let section = &line[open + 2..close];
            let Some(name) = line[close + 2..].split_whitespace().last() else { continue };
            symbols.insert(name.to_string(), section.contains(",__thread_"));
        }
    }
    Ok(symbols)
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
    if std::env::var_os("RELAX_NATIVE_RUSTC").is_none() {
        return false;
    }
    let Some(first) = args.first() else { return false };
    if std::env::var("RUSTC").is_ok_and(|rustc| first == &rustc) {
        return false;
    }
    let (Ok(out), Some(target), Ok(triple)) = (std::env::var("OUT_DIR"), std::env::var_os("RELAX_NATIVE_BUILD_DIR"), std::env::var("RELAX_NATIVE_TARGET")) else { return false };
    is_target_out_dir(&out, std::path::Path::new(&target), &triple)
}

/// This executable as cargo's `RUSTC_WRAPPER` (`RELAX_NATIVE_SCRIPTS` set) or as a stubbed
/// build script. Returns the exit code when it acted, None when this is an ordinary start.
pub fn wrapper() -> Option<i32> {
    let scripts = std::env::var("RELAX_NATIVE_SCRIPTS").ok()?;
    // cargo's arguments carry paths, which need not be Unicode: rustc gets them as they are, and
    // a stub reads its flags from a lossy copy (its files go where the paths say, as given)
    let os_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let args: Vec<String> = os_args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    // a build script stub (cargo runs what the wrapper left as the script): nothing to do
    let stem = std::path::Path::new(&os_args[0]).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if std::env::var_os("OUT_DIR").is_some() && (stem.starts_with("build_script_") || stem.starts_with("build-script-")) {
        return Some(0);
    }
    // cargo runs `<wrapper> <rustc> <args>`
    if args.len() < 2 || std::env::var_os("RUSTC_WRAPPER").is_none() && std::env::var_os("CARGO_PKG_NAME").is_none() {
        return None;
    }
    let native_rustc = std::env::var_os("RELAX_NATIVE_RUSTC");
    let direct = native_rustc.is_some() && std::env::var_os("RUSTC").is_some_and(|rustc| os_args[1] != rustc);
    let rustc = native_rustc.unwrap_or_else(|| os_args[1].clone());
    let rustc_name = rustc.to_string_lossy().into_owned();
    let from = if direct { 1 } else { 2 };
    let (rest, os_rest) = (&args[from..], &os_args[from..]);
    if direct {
        return Some(match Command::new(&rustc).args(os_rest).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => { eprintln!("relax run: native compiler probe: {}: {}", rustc_name, e); 1 }
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
            if n.starts_with("build_script_") { scripts.split(',').any(|x| x == pkg) } else { list("RELAX_NATIVE_LIBS") }
        }
    };
    if real {
        return Some(match Command::new(&rustc).args(os_rest).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("relax run: native wrapper: cannot run {}: {}", rustc_name, e);
                1
            }
        });
    }
    // a stub: the files cargo expects, empty
    let name = name.unwrap();
    // (cargo passes `--out-dir <path>`: that path as given)
    let out_dir = match rest.iter().position(|a| a == "--out-dir").and_then(|i| os_rest.get(i + 1)) {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::path::PathBuf::from(value("--out-dir").unwrap_or_else(|| ".".to_string())),
    };
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
    // the program's binaries and test harnesses: their link arguments, which the native library is
    // linked with (program_link_args). The stub's dep-info names the run's folder, so the next
    // cargo run, with a folder of its own, does not take this one as fresh.
    let capture = std::env::var("RELAX_NATIVE_LINK_ARGS").ok().filter(|_| {
        std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some() && (types.iter().any(|t| t == "bin") || rest.iter().any(|a| a == "--test"))
    });
    if let Some(dir) = &capture {
        let _ = std::fs::write(std::path::Path::new(dir).join(&name), link_args_of(rest).join("\n"));
    }
    let emit = value("--emit").unwrap_or_default();
    let json = value("--json").unwrap_or_default().contains("artifacts");
    let dylib_ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    // a binary as rustc names it, which cargo runs (a stub build script) or hard-links: `.exe` on
    // Windows. Before, the stub had no `.exe`, so cargo found no build-script-build.exe to run
    // ("never executed", os error 2) for the first stubbed script it ran (relax issue 164)
    let bin = format!("{}{}{}", name, extra, std::env::consts::EXE_SUFFIX);
    let mut made: Vec<(std::path::PathBuf, &str)> = Vec::new();
    for e in emit.split(',') {
        match e.split('=').next().unwrap_or("") {
            "dep-info" => made.push((out_dir.join(format!("{}{}.d", name, extra)), "dep-info")),
            "metadata" => made.push((out_dir.join(format!("lib{}{}.rmeta", name, extra)), "metadata")),
            "link" => {
                for t in &types {
                    let f = match t.as_str() {
                        "bin" => bin.clone(),
                        "rlib" | "lib" => format!("lib{}{}.rlib", name, extra),
                        "staticlib" => format!("lib{}{}.a", name, extra),
                        _ => format!("lib{}{}.{}", name, extra, dylib_ext),
                    };
                    made.push((out_dir.join(f), "link"));
                }
            }
            _ => {}
        }
    }
    let exe = std::env::current_exe().ok();
    for (path, kind) in &made {
        let _ = std::fs::create_dir_all(&out_dir);
        let is_bin = kind == &"link" && types.iter().any(|t| t == "bin") && path.file_name() == Some(std::ffi::OsStr::new(&bin));
        if is_bin {
            // the stub script is this executable (wrapper(): a build script stub exits at once)
            let _ = std::fs::remove_file(path);
            if let Some(e) = &exe {
                if std::fs::hard_link(e, path).is_err() {
                    let _ = std::fs::copy(e, path);
                }
            }
        } else if *kind == "dep-info" {
            // (the paths as given)
            let src = rest.iter().position(|a| a.ends_with(".rs")).map(|i| os_rest[i].clone()).unwrap_or_default();
            let mut d = path.as_os_str().as_encoded_bytes().to_vec();
            d.extend_from_slice(b": ");
            d.extend_from_slice(src.as_encoded_bytes());
            d.push(b'\n');
            if let Some(dir) = &capture {
                // rustc's form of an `env!` the crate read (cargo's dep-info parser unescapes it)
                d.extend_from_slice(format!("\n# env-dep:RELAX_NATIVE_LINK_ARGS={}\n", dir.replace('\\', "\\\\").replace('\n', "\\n")).as_bytes());
            }
            let _ = std::fs::write(path, d);
        } else {
            let _ = std::fs::write(path, b"");
        }
        if json {
            eprintln!("{{\"$message_type\":\"artifact\",\"artifact\":\"{}\",\"emit\":\"{}\"}}", path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""), kind);
        }
    }
    Some(0)
}

#[cfg(test)]
#[test]
fn a_static_library_comes_from_the_script_that_built_it() {
    // two scripts: the first only adds a search path that happens to hold a library of the
    // second's name (a system dev package); the second builds its own copy into its OUT_DIR
    let root = std::env::temp_dir().join(format!("relax-native-static-home-{}", std::process::id()));
    let (system, bundled) = (root.join("system"), root.join("bundled"));
    for d in [&system, &bundled] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("libsqlite3.a"), b"!<arch>\n").unwrap();
    }
    let pkg = vec![
        (String::new(), "fontish-sys".to_string(), "1.0.0".to_string()),
        (String::new(), "sqlite-sys".to_string(), "1.0.0".to_string()),
    ];
    let scripts = vec![(0usize, Vec::new()), (1usize, Vec::new())];
    let line = |name: &str, paths: &str, libs: &str| format!(
        r#"{{"reason":"build-script-executed","package_id":"registry+https://github.com/rust-lang/crates.io-index#{name}@1.0.0","linked_libs":[{libs}],"linked_paths":[{paths}],"cfgs":[],"env":[],"out_dir":""}}"#);
    let messages = format!("{}\n{}\n",
        line("fontish-sys", &format!(r#""native={}""#, system.display()), r#""fontish""#),
        line("sqlite-sys", &format!(r#""native={}""#, bundled.display()), r#""static=sqlite3""#));
    let out = outputs_with(&pkg, &scripts, &[0, 1], &messages).unwrap_or_else(|e| panic!("{e}"));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(out.archives, vec![format!("{}/libsqlite3.a", bundled.display())]);
}

#[cfg(test)]
#[test]
fn the_c_runtime_libraries_are_the_targets_own() {
    for triple in ["x86_64-pc-windows-gnu", "x86_64-pc-windows-msvc", "aarch64-pc-windows-gnullvm"] {
        assert!(c_runtime_libs(triple).is_empty(), "{triple}: Windows' C runtime has no libdl, and links its math and threads by default");
    }
    for triple in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-musl"] {
        assert_eq!(c_runtime_libs(triple), ["-lm", "-lpthread", "-ldl"], "{triple}: libm, libpthread and libdl are libraries of their own");
    }
    assert!(c_runtime_libs("aarch64-apple-darwin").is_empty(), "macOS: libSystem holds them");
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
        pkg: (String::new(), name.to_string(), "1.0.0".to_string()), path_source: false, host_checks: None,
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
        pkg: (format!("packages/{name}"), name.into(), version.into()), path_source: false, host_checks: None,
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

/// The native key follows Relax's resolution of the native closure, features included, and the
/// files of its path packages: never the package a run is for, nor what lies outside the closure
/// (relax PR 115: a runner update whose resolver moves, but not the closure, keeps the build).
#[cfg(test)]
#[test]
fn native_key_follows_the_resolved_closure_and_its_path_sources() {
    let dir = std::env::temp_dir().join(format!("relax-native-key-{}", std::process::id()));
    let ws = dir.join("ws");
    for (rel, text) in [("n/build.rs", "fn main() {}\n"), ("h/src/lib.rs", "pub const LEVEL: u8 = 1;\n")] {
        let p = ws.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let at = |rel: &str| ws.join(rel).display().to_string();
    let unit = |name: &str, dir: String, features: &[&str], deps: &[usize], path_source: bool| crate::unitgraph::RawUnit {
        name: name.into(), src_path: String::new(), edition: 2021, features: features.iter().map(|f| f.to_string()).collect(),
        proc_macro: false, deps: deps.iter().map(|&d| (d, String::new())).collect(), run_script: false,
        pkg: (dir, name.into(), "1.0.0".into()), path_source, host_checks: None,
    };
    // the package a run is for (`root`), the native package n (its lib, and its script, which
    // build-depends on the workspace crate h and the registry crate cc), and x outside the closure
    let graph = |root: &str, n_features: &[&str], x_features: &[&str]| Graph {
        crates: vec![crate::crates::CrateUnit { name: "n".into(), src_path: at("n/src/lib.rs"), edition: 2021, features: Vec::new(), deps: Vec::new(),
            host: None, proc_macro: false, bin: false, harness: false, build_script: false, version: "1.0.0".into(), registry: false, host_only: false }],
        root: 0,
        build_scripts: vec![BuildScript { krate: 0, also: Vec::new(), src_path: at("n/build.rs"), edition: 2021,
            features: n_features.iter().map(|f| f.to_string()).collect(), build_deps: vec![(3, "h".into()), (4, "cc".into())], links_deps: Vec::new(), kept: false, profile: crate::resolve::ScriptProfile::RUN }],
        pkg: vec![(at("n"), "n".into(), "1.0.0".into())],
        cfgs: vec![Vec::new()],
        env: vec![Vec::new()],
        link_libs: Vec::new(),
        native: None,
        raw: vec![
            unit(root, at(root), &[], &[1, 5], true),
            unit("n", at("n"), n_features, &[], true),
            unit("n", at("n"), n_features, &[3, 4], true),
            unit("h", at("h"), &[], &[], true),
            unit("cc", "registry/cc-1.0.0".into(), &["parallel"], &[], false),
            unit("x", "registry/x-1.0.0".into(), x_features, &[], false),
        ],
        bins: Vec::new(),
    };
    let flags: Vec<String> = Vec::new();
    let relax_dir = dir.join("relax");
    let req = Request { workspace: ws.to_str().unwrap(), package: "app", cargo_flags: &flags, relax_dir: relax_dir.to_str().unwrap(), triple: "x86_64-unknown-linux-gnu", resolvable: |names, _, _, _| vec![false; names.len()], quiet: false };
    let key = |g: &Graph| plan(g, &[0], &req).key;
    let k = key(&graph("app", &["std"], &["a"]));
    assert_eq!(k, key(&graph("second", &["std"], &["a"])), "another package with the same native closure must share the build");
    assert_eq!(k, key(&graph("app", &["std"], &["a", "b"])), "a resolution that moves outside the native closure (a runner update's resolver) must keep the build");
    assert_ne!(k, key(&graph("app", &["std", "static"], &["a"])), "the closure's resolved features must be in the key");
    std::fs::write(ws.join("h/src/lib.rs"), "pub const LEVEL: u8 = 2;\n").unwrap();
    assert_ne!(k, key(&graph("app", &["std"], &["a"])), "a path build-dependency's source must be in the key");
    let _ = std::fs::remove_dir_all(&dir);
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
    let req = Request { workspace: workspace.to_str().unwrap(), package: "app", cargo_flags: &flags, relax_dir, triple: "x86_64-unknown-linux-gnu", resolvable: |names, _, _, _| vec![false; names.len()], quiet: false };
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
    assert_eq!(prune(&native, later, true), unnamed.len());
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
    assert_eq!(prune(&native, since, true), 0);
    assert!(fresh.join("blob").is_file(), "a fresh selection");
    // the lock held (a root being written): the prune leaves everything for the next start. (A
    // child another test spawns may hold it for a moment after it was let go, `prune`: so this
    // test waits for it, and the prunes that must run wait.)
    let held = native_lock(&native, true).unwrap();
    assert_eq!(prune(&native, later, false), 0);
    assert!(fresh.is_dir());
    drop(held);
    assert_eq!(prune(&native, later, true), 1);
    // a shared workspace's run (RELAX_SHARE: its workspace is a folder mount's virtual path, on no
    // disk) names the checkout on disk: the slot of a run in the checkout itself, which the prune
    // finds and keeps with its build
    let shared = format!("{}/vfs/share/native-prune", dir.display());
    crate::vfs::mount_folder(&shared, &workspace);
    let req = Request { workspace: &shared, ..req };
    let plan = Plan { key: 0xdddddddddddddddd, selection: 0x5555555555555555, ..plan };
    write_root(&req, &plan).unwrap();
    let texts: Vec<String> = std::fs::read_dir(native.join("roots")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    assert_eq!(texts.len(), 2, "the checkout's slot, replaced: {texts:?}");
    assert!(texts.contains(&format!("{}\ndddddddddddddddd\n5555555555555555\n", workspace.display())), "{texts:?}");
    let built = [unit("dddddddddddddddd"), unit("cargo-target/5555555555555555")];
    assert_eq!(prune(&native, later, true), 1, "the build the replaced root named");
    for p in &built { assert!(p.join("blob").is_file(), "the shared run's root names {}", p.display()); }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A path package of a shared workspace (RELAX_SHARE) is a folder mount's virtual path, on no
/// disk: the native key hashes its files in the checkout, so an edit in it builds again (read as
/// a disk path, it hashed nothing).
#[cfg(test)]
#[test]
fn a_shared_path_packages_files_are_hashed_in_its_checkout() {
    let dir = std::env::temp_dir().join(format!("relax-native-shared-hash-{}", std::process::id()));
    let pkg = dir.join("checkout/h");
    std::fs::create_dir_all(pkg.join("src")).unwrap();
    std::fs::write(pkg.join("src/lib.rs"), "pub const LEVEL: u8 = 1;\n").unwrap();
    let shared = format!("{}/vfs/share/native-hash", dir.display());
    crate::vfs::mount_folder(&shared, &dir.join("checkout"));
    let hash = |p: &str| {
        let mut h = 0xcbf29ce484222325;
        hash_package_dir(p, &mut h);
        h
    };
    let virtual_dir = format!("{shared}/h");
    let one = hash(&virtual_dir);
    assert_eq!(one, hash(pkg.to_str().unwrap()), "the virtual path hashes the checkout's files");
    std::fs::write(pkg.join("src/lib.rs"), "pub const LEVEL: u8 = 2;\n").unwrap();
    assert_ne!(one, hash(&virtual_dir), "an edit in the shared package changes its hash");
    let _ = std::fs::remove_dir_all(&dir);
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&STARTED, std::sync::atomic::AtomicBool::new(false));
}
