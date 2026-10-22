//! The crate graph of one cargo package for `makepad run`: cargo's unit graph (lib, proc-macro
//! and the package's bin), build-script results (cfgs, link libraries, OUT_DIR, rustc-env) and
//! the per-crate compile-time environment that `env!` reads (CARGO_MANIFEST_DIR,
//! CARGO_PKG_*, OUT_DIR).
//!
//! Build scripts run as Relax programs: this executable started again on the script's units (its
//! build-dependencies and the script as a bin, run_one), with cargo's environment; its `cargo:`
//! lines are read like cargo does, the output kept with cargo's rerun rules. No rustc.

use crate::crates::CrateUnit;
pub use crate::resolve::{GraphUnit, Units};
use relax_session::sync::Mutex;

#[path = "unitgraph_noop.rs"]
mod noop;

/// What platform/build.rs generates (the same file build.rs includes): answered natively here.
#[path = "../../../../platform/build_gen.rs"]
mod platform_gen;

pub struct BuildScript {
    /// crate index (into `Graph::crates`) of the lib the script belongs to
    pub krate: usize,
    /// the package's other crates that depend on the same run of the script (its bins): cargo runs
    /// a package's script once, every target of the package gets its output
    pub also: Vec<usize>,
    pub src_path: String,
    pub edition: u16,
    pub features: Vec<String>,
    /// build-dependencies: (raw unit index, extern crate name), compiled with the host rustc
    pub build_deps: Vec<(usize, String)>,
}

impl BuildScript {
    /// every crate the script's output goes to
    pub fn krates(&self) -> Vec<usize> { std::iter::once(self.krate).chain(self.also.iter().copied()).collect() }
}

/// A unit as cargo lists it (for compiling build-dependencies with the host rustc).
#[derive(Clone)]
pub struct RawUnit {
    pub name: String,
    pub src_path: String,
    pub edition: u16,
    pub features: Vec<String>,
    pub proc_macro: bool,
    pub deps: Vec<(usize, String)>,
    /// a `run-custom-build` unit: running its package's build script (deps[0] compiles it)
    pub run_script: bool,
    /// (manifest dir, package name, package version)
    pub pkg: (String, String, String),
}

pub struct Graph {
    pub crates: Vec<CrateUnit>,
    /// the package's bin (or lib when it has none)
    pub root: usize,
    pub build_scripts: Vec<BuildScript>,
    /// per crate: (manifest dir, package name, package version)
    pub pkg: Vec<(String, String, String)>,
    /// per crate: extra cfgs from its build script (`name` or `name="value"`)
    pub cfgs: Vec<Vec<String>>,
    /// per crate: compile-time environment for env!()
    pub env: Vec<Vec<(String, String)>>,
    /// libraries the build scripts link: (name, framework)
    pub link_libs: Vec<(String, bool)>,
    /// `--allow-c`: the shared library the native dependencies were linked into (native.rs)
    pub native: Option<crate::native::NativeLib>,
    pub raw: Vec<RawUnit>,
    /// `makepad check` of a package with several bins (and no --bin): every bin's crate (cargo
    /// check checks all targets); the root is the first. Empty otherwise.
    pub bins: Vec<usize>,
}

/// "path+file:///a/b/platform#makepad-platform@2.0.0" -> ("/a/b/platform", "makepad-platform", "2.0.0");
/// "path+file:///a/b/counter#1.0.0" -> name = last path component.
/// The directory holding the Cargo.toml nearest above `src` ("" if none).
pub fn manifest_dir_of(src: &str) -> String {
    let mut p = std::path::Path::new(src).parent();
    while let Some(d) = p {
        if crate::vfs::is_file(d.join("Cargo.toml")) {
            return d.display().to_string();
        }
        p = d.parent();
    }
    String::new()
}

/// The path of a file URL's path part: a Windows drive path comes as "/C:/a/b" (RFC 8089).
fn file_url_path(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        p[1..].to_string()
    } else {
        p.to_string()
    }
}

pub(crate) fn parse_pkg_id(id: &str) -> (String, String, String) {
    let (url, frag) = match id.rfind('#') {
        Some(i) => (&id[..i], &id[i + 1..]),
        None => (id, ""),
    };
    let dir = match url.find("file://") {
        Some(i) => file_url_path(&url[i + 7..]),
        None => String::new(),
    };
    let (name, ver) = match frag.rfind('@') {
        Some(j) => (frag[..j].to_string(), frag[j + 1..].to_string()),
        None => {
            let last = match dir.rfind('/') {
                Some(k) => dir[k + 1..].to_string(),
                None => dir.clone(),
            };
            (last, frag.to_string())
        }
    };
    (dir, name, ver)
}

/// The package's unit graph from Relax's own resolution (resolve.rs): the manifests and
/// Cargo.lock, no cargo. Read on every launch (13-43 ms for the apps); nothing to cache.
pub fn unit_graph(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, cargo_flags: &[String]) -> Result<Units, String> {
    crate::resolve::unit_graph(sess, workspace, package, cargo_flags)
}

pub fn load(g: &Units, check_all_bins: bool) -> Result<Graph, String> {
    let units = &g.units;
    let roots = &g.roots;
    let mut crates: Vec<CrateUnit> = Vec::new();
    // per crate: its normalized source path (host and target copies of a crate are one crate)
    let mut crate_paths: Vec<String> = Vec::new();
    let mut pkg = Vec::new();
    let n_units = units.len();
    let mut map: Vec<usize> = vec![usize::MAX; n_units]; // unit index -> crate index or usize::MAX
    let mut root = usize::MAX;
    // (crate index, bin name, package dir) of the bins cargo would build
    let mut root_bins: Vec<(usize, String, String)> = Vec::new();
    // crates built with `--test` (cargo's unit mode "test"): cfg(test) is set for them
    let mut test_crates: Vec<usize> = Vec::new();
    // unit index of a run-custom-build unit -> its script (src_path, edition, features, build unit)
    let mut run_build: Vec<Option<(String, u16, Vec<String>, usize)>> = vec![None; n_units];
    let mut raw: Vec<RawUnit> = vec![RawUnit { name: String::new(), src_path: String::new(), edition: 2015, features: Vec::new(), proc_macro: false, deps: Vec::new(), run_script: false, pkg: Default::default() }; n_units];
    // a crate used on both sides (normal dependency and build-dependency / proc-macro side) is one
    // crate here: the target side's unit decides its features (target units first; host units
    // are cargo's opt-level 0 copies, or Relax's resolver marks them)
    let mut order: Vec<usize> = (0..n_units).filter(|&ui| !units[ui].host).collect();
    order.extend((0..n_units).filter(|&ui| units[ui].host));
    for &ui in &order {
        let u = &units[ui];
        // a unit without a target (cargo lists none such for builds)
        if u.kind.is_empty() && u.name.is_empty() && u.src_path.is_empty() {
            continue;
        }
        let mut lib = false;
        let mut pm = false;
        let mut bin = false;
        let mut build = false;
        for s in &u.kind {
            match s.as_str() {
                "lib" | "rlib" => lib = true,
                "proc-macro" => pm = true,
                // an integration test target: a root like a bin (its harness runs its tests)
                "bin" | "test" => bin = true,
                "custom-build" => build = true,
                _ => {}
            }
        }
        let src_path = u.src_path.clone();
        let edition = u.edition.parse::<u16>().unwrap_or(2015);
        let features = u.features.clone();
        let mode = u.mode.as_str();
        let udeps = u.deps.clone();
        let tname = u.name.replace('-', "_");
        let (pkg_dir, pname, ver) = {
            let (mut d, n, v) = parse_pkg_id(&u.pkg_id);
            if d.is_empty() {
                // a registry / git package id names no path: the package is the directory of the
                // Cargo.toml above its target's source (cargo's CARGO_MANIFEST_DIR)
                d = manifest_dir_of(&src_path);
            }
            (d, n, v)
        };
        raw[ui] = RawUnit { name: tname.clone(), src_path: src_path.clone(), edition, features: features.clone(), proc_macro: pm, deps: udeps.clone(), run_script: build && mode == "run-custom-build", pkg: (pkg_dir.clone(), pname.clone(), ver.clone()) };
        // the run unit depends on the script's compile unit, whose deps are the build-dependencies
        let build_unit = udeps.first().map(|d| d.0).unwrap_or(usize::MAX);
        run_build[ui] = if build && mode == "run-custom-build" { Some((src_path.clone(), edition, features.clone(), build_unit)) } else { None };
        let is_root_bin = bin && roots.contains(&ui);
        if !(lib || pm || is_root_bin) {
            continue;
        }
        // host and target copies of a crate parse the same files: one crate
        let norm = crate::vfs::normalize_path(&src_path);
        if let Some(i) = crate_paths.iter().position(|p| *p == norm) {
            map[ui] = i;
            if is_root_bin || (root == usize::MAX && roots.contains(&ui)) { root = i; }
            continue;
        }
        if is_root_bin {
            root_bins.push((crates.len(), u.name.clone(), pkg_dir.clone()));
        }
        if is_root_bin || (root == usize::MAX && roots.contains(&ui)) {
            root = crates.len();
        }
        map[ui] = crates.len();
        if mode == "test" {
            test_crates.push(crates.len());
        }
        crates.push(CrateUnit { name: tname, src_path, edition, features, deps: udeps,  proc_macro: pm, bin, build_script: false, version: ver.clone() });
        crate_paths.push(norm);
        pkg.push((pkg_dir, pname, ver));
    }
    if root == usize::MAX {
        return Err("unit graph: the package has no bin or lib root".to_string());
    }
    // several bins (no --bin): the package's `default-run`, else cargo run's error
    let mut bins = Vec::new();
    if root_bins.len() > 1 {
        let manifest = crate::vfs::read_to_string(format!("{}/Cargo.toml", root_bins[0].2)).unwrap_or_default();
        let default_run = manifest.lines().find_map(|l| {
            let l = l.trim();
            let v = l.strip_prefix("default-run")?.trim_start().strip_prefix('=')?.trim();
            Some(v.trim_matches('"').to_string())
        });
        match default_run.and_then(|d| root_bins.iter().find(|b| b.1 == d)) {
            // a test graph's integration tests: every one of them is run
            _ if root_bins.iter().all(|b| test_crates.contains(&b.0)) => {
                root = root_bins[0].0;
                bins = root_bins.iter().map(|b| b.0).collect();
            }
            Some(b) => root = b.0,
            // a check runs nothing: it checks every bin, as `cargo check` does
            None if check_all_bins => {
                root = root_bins[0].0;
                bins = root_bins.iter().map(|b| b.0).collect();
            }
            None => {
                let names: Vec<&str> = root_bins.iter().map(|b| b.1.as_str()).collect();
                return Err(format!("could not determine which binary to run: use --bin (available: {}) or set `default-run` in the manifest", names.join(", ")));
            }
        }
    }
    // dependency edges: crates keep crate deps; a run-custom-build dep becomes the crate's script
    let mut build_scripts: Vec<BuildScript> = Vec::new();
    // run-custom-build unit -> its script (lib and bins of a package depend on one run)
    let mut script_of_run: std::collections::HashMap<usize, usize> = Default::default();
    for (ci, c) in crates.iter_mut().enumerate() {
        let mut nd = Vec::new();
        for (i, n) in &c.deps {
            if *i < map.len() && map[*i] != usize::MAX {
                nd.push((map[*i], n.clone()));
            } else if let Some(Some((p, ed, f, bu))) = run_build.get(*i) {
                c.build_script = true;
                if let Some(&s) = script_of_run.get(i) {
                    build_scripts[s].also.push(ci);
                    continue;
                }
                let build_deps = raw.get(*bu).map(|r| r.deps.clone()).unwrap_or_default();
                script_of_run.insert(*i, build_scripts.len());
                build_scripts.push(BuildScript { krate: ci, also: Vec::new(), src_path: p.clone(), edition: *ed, features: f.clone(), build_deps });
            }
        }
        c.deps = nd;
    }

    let n = crates.len();
    let mut env = Vec::new();
    for (dir, name, ver) in &pkg {
        env.push(pkg_env(dir, name, ver));
    }
    for (ci, c) in crates.iter().enumerate() {
        env[ci].push(("CARGO_CRATE_NAME".to_string(), c.name.clone()));
    }
    let mut cfgs = vec![Vec::new(); n];
    for &ci in &test_crates {
        cfgs[ci].push("test".to_string());
    }
    // a single integration test is the graph's only test root: it is listed like several
    if bins.is_empty() && test_crates.contains(&root) && crates[root].bin {
        bins.push(root);
    }
    Ok(Graph { crates, root, build_scripts, pkg, cfgs, env, link_libs: Vec::new(), native: None, raw, bins })
}

/// cargo's per-package compile-time environment (CARGO_MANIFEST_DIR, CARGO_PKG_*).
fn pkg_env(dir: &str, name: &str, ver: &str) -> Vec<(String, String)> {
    let mut e = vec![
        ("CARGO_MANIFEST_DIR".to_string(), dir.to_string()),
        ("CARGO_PKG_NAME".to_string(), name.to_string()),
        ("CARGO_PKG_VERSION".to_string(), ver.to_string()),
    ];
    let (core, pre) = version_parts(ver);
    let parts: Vec<&str> = core.split('.').collect();
    for (k, key) in ["CARGO_PKG_VERSION_MAJOR", "CARGO_PKG_VERSION_MINOR", "CARGO_PKG_VERSION_PATCH"].iter().enumerate() {
        e.push((key.to_string(), parts.get(k).unwrap_or(&"0").to_string()));
    }
    e.push(("CARGO_PKG_VERSION_PRE".to_string(), pre.to_string()));
    e.extend(crate::resolve::package_meta_env(dir));
    e
}

/// A version's numeric core and its pre-release part (`1.2.3-rc.1+build` -> `1.2.3`, `rc.1`; the
/// pre-release empty when none): cargo's CARGO_PKG_VERSION_{MAJOR,MINOR,PATCH,PRE} come from these.
fn version_parts(ver: &str) -> (&str, &str) {
    let core = ver.split('+').next().unwrap_or(ver);
    core.split_once('-').unwrap_or((core, ""))
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The triple the checked crate graph is read for (MAKEPAD_RUN_CHECK_TARGET, else the machine).
fn target_triple(sess: &std::sync::Arc<crate::Session>) -> &'static str {
    crate::cfg::CfgSet::host(sess, &[]).triple()
}

/// The machine Relax runs on: build scripts are host artifacts (cargo's HOST) and run here.
fn host_triple(sess: &std::sync::Arc<crate::Session>) -> &'static str {
    crate::cfg::CfgSet::machine(sess, &[]).triple()
}

/// Compiles (once per source) and runs every build script; records cfgs, link libraries,
/// OUT_DIR and rustc-env per crate. Scripts run in parallel. `target_dir` is the run's relax dir,
/// `<target>/relax` (OUT_DIR = <target>/relax/build/<crate>-<hash>/out: as deep as cargo's
/// <target>/release/build/<crate>-<hash>/out, since scripts look 5 ancestors up for the
/// workspace root).
/// What Relax does with a package's build script. The user's rule (2026-10-10): only
/// makepad-platform keeps a build script; the GPU kernel crates (ai-cuda/ai-metal and the ai crates
/// that read their outputs) keep theirs until their replacement is decided. Probe scripts of
/// third-party crates Relax answers itself. Every other script is to be removed from the source;
/// until it is, it still runs and `makepad run` / `check` list it (`to_remove`).
pub enum ScriptRole {
    /// kept: runs (as a Relax program), its output cached by the rerun rules as cargo does
    Artefact,
    /// makepad-platform's: answered by `platform_answer` from the code build.rs shares (build_gen.rs)
    Platform,
    /// the script's `cargo:` lines as Relax knows them, without running it
    Answered(Vec<String>),
    /// to be removed from the source
    ToRemove,
}

pub fn script_role(pkg: &str, features: &[String]) -> ScriptRole {
    let has = |f: &str| features.iter().any(|x| x == f);
    match pkg {
        "makepad-platform" => ScriptRole::Platform,
        // pending the GPU-kernel decision (no build scripts proposal)
        "makepad-ai-cuda" | "makepad-ai-metal" | "makepad-ai-llm" | "makepad-ai-common" | "makepad-ai-speech" => ScriptRole::Artefact,
        // pkg-config probes: the libraries of the enabled features (none with dlopen)
        "wayland-sys" => {
            if has("dlopen") {
                return ScriptRole::Answered(Vec::new());
            }
            let libs = [("client", "wayland-client"), ("cursor", "wayland-cursor"), ("egl", "wayland-egl"), ("server", "wayland-server")];
            let mut lines: Vec<String> = libs.iter().filter(|(f, _)| has(f)).map(|(_, l)| format!("cargo:rustc-link-lib={}", l)).collect();
            // the .pc files add libm (`Libs: -lwayland-client -lm`)
            if !lines.is_empty() {
                lines.push("cargo:rustc-link-lib=m".to_string());
            }
            ScriptRole::Answered(lines)
        }
        _ => ScriptRole::ToRemove,
    }
}

/// Packages whose build script ran although it is to be removed: (package, script path).
pub static TO_REMOVE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn run_build_scripts(sess: &std::sync::Arc<crate::Session>, g: &mut Graph, target_dir: &str, native: Option<&crate::native::Request>) -> Result<(), String> {
    let results: Mutex<Vec<(Vec<usize>, Result<String, String>, String)>> = Mutex::new(Vec::new());
    // the packages whose scripts compile or link C (native.rs): built by cargo with `--allow-c`,
    // an error naming the flag without it; Relax runs the other scripts
    let mut native_seeds: Vec<usize> = Vec::new();
    for (i, bs) in g.build_scripts.iter().enumerate() {
        let (_, pname, _) = &g.pkg[bs.krate];
        if noop::script(bs, &g.raw) || !matches!(script_role(pname, &bs.features), ScriptRole::ToRemove) {
            continue;
        }
        if let Some(why) = crate::native::native_reason(g, bs) {
            if native.is_none() {
                // a script that runs a C compiler cannot work without one; `links =` / DEP_
                // metadata alone run under Relax as before
                if why.contains("build-dependency") {
                    return Err(format!("`{}` compiles C ({}): it {}", pname, why, relax_session::options::C_GATE));
                }
                continue;
            }
            if sess.runtime.profile {
                eprintln!("makepad-run build-script: {} is native ({})", pname, why);
            }
            native_seeds.push(i);
        }
    }
    if let (Some(req), false) = (native, native_seeds.is_empty()) {
        let built = crate::native::build(g, &native_seeds, req)?;
        g.native = built.lib;
        for n in built.scripts {
            let also = g.build_scripts.iter().find(|b| b.krate == n.krate).map(|b| b.also.clone()).unwrap_or_default();
            for &k in std::iter::once(&n.krate).chain(&also) {
                g.cfgs[k].extend(n.cfgs.iter().cloned());
                g.env[k].extend(n.env.iter().cloned());
                g.env[k].push(("OUT_DIR".to_string(), n.out_dir.clone()));
            }
            for l in n.system {
                if !g.link_libs.contains(&l) {
                    g.link_libs.push(l);
                }
            }
        }
    }
    let native_krates: Vec<usize> = native_seeds.iter().map(|&i| g.build_scripts[i].krate).collect();
    // makepad-platform's script is answered by makepad run itself (`platform_answer`, no process);
    // in no-codegen mode (the platforms that run no generated code, iOS) no other script runs: any
    // other crate with a build script is an error.
    if sess.options.no_codegen {
        for bs in &g.build_scripts {
            let (_, pname, _) = &g.pkg[bs.krate];
            if pname != "makepad-platform" {
                return Err(format!("build scripts are not supported in no-codegen mode: {} ({})", pname, bs.src_path));
            }
        }
    }
    crate::pool::scope(|s| {
        for bs in &g.build_scripts {
            if native_krates.contains(&bs.krate) {
                continue;
            }
            let results = &results;
            let (dir, pname, ver) = g.pkg[bs.krate].clone();
            let name = g.crates[bs.krate].name.clone();
            let raw = &g.raw;
            let cenv = g.env[bs.krate].clone();
            let role = if noop::script(bs, raw) {
                if sess.runtime.profile {
                    eprintln!("makepad-run build-script: {} skipped (no effect under MAKEPAD_RUN_BUILD_SCRIPT)", pname);
                }
                ScriptRole::Answered(Vec::new())
            } else { script_role(&pname, &bs.features) };
            match role {
                ScriptRole::Answered(lines) => {
                    // cargo sets OUT_DIR for every package with a build script
                    let out_dir = format!("{}/build/{}-answered/out", target_dir, name);
                    let _ = std::fs::create_dir_all(&out_dir);
                    results.lock().unwrap().push((bs.krates(), Ok(lines.join("\n")), out_dir));
                    continue;
                }
                ScriptRole::ToRemove => {
                    let mut t = TO_REMOVE.lock().unwrap();
                    if !t.iter().any(|x| x.0 == pname) {
                        crate::zone_alloc::kept_with(|| t.push((pname.clone(), bs.src_path.clone())));
                    }
                }
                ScriptRole::Platform => {
                    let out_dir = format!("{}/build/{}-answered/out", target_dir, name);
                    let r = std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {}", out_dir, e)).and_then(|_| platform_answer(sess, &out_dir));
                    results.lock().unwrap().push((bs.krates(), r.map(|l| l.join("\n")), out_dir));
                    continue;
                }
                ScriptRole::Artefact => {}
            }
            s.spawn(move || {
                let r = run_one(sess, bs, raw, &cenv, &dir, &pname, &ver, &name, target_dir);
                results.lock().unwrap().push((bs.krates(), r.0, r.1));
            });
        }
    });
    apply_script_results(g, results.into_inner().unwrap())
}

/// The scripts' `cargo:` lines into the graph: cfgs, link libraries, env (OUT_DIR included).
fn apply_script_results(g: &mut Graph, results: Vec<(Vec<usize>, Result<String, String>, String)>) -> Result<(), String> {
    for (krates, r, out_dir) in results {
        let stdout = r?;
        for k in krates { apply_script_output(g, k, &stdout, out_dir.clone())?; }
    }
    Ok(())
}

/// One crate's share of a script's output (`stdout`, its OUT_DIR).
fn apply_script_output(g: &mut Graph, k: usize, stdout: &str, out_dir: String) -> Result<(), String> {
    g.env[k].push(("OUT_DIR".to_string(), out_dir));
    for line in stdout.lines() {
        let rest = match line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:")) {
            Some(r) => r,
            None => continue,
        };
        if let Some(c) = rest.strip_prefix("rustc-cfg=") {
            g.cfgs[k].push(c.to_string());
        } else if let Some(l) = rest.strip_prefix("rustc-link-lib=") {
            // [kind=]name[:rename]
            let (kind, name) = match l.find('=') {
                Some(i) => (&l[..i], &l[i + 1..]),
                None => ("", l),
            };
            let name = name.split(':').next().unwrap_or(name).to_string();
            // a static library a script built (C, assembly): only `--allow-c` links one
            if kind == "static" {
                return Err(format!("`{}`'s build script links the static library `{}` (native code): it {}", g.pkg[k].1, name, relax_session::options::C_GATE));
            }
            let fw = kind == "framework";
            if !g.link_libs.contains(&(name.clone(), fw)) {
                g.link_libs.push((name, fw));
            }
        } else if let Some(e) = rest.strip_prefix("rustc-env=") {
            if let Some(i) = e.find('=') {
                g.env[k].push((e[..i].to_string(), e[i + 1..].to_string()));
            }
        }
    }
    Ok(())
}

/// The units a build script runs with: its build-dependencies and everything they reach (their
/// own build scripts included), plus the script itself as the bin root, in cargo's unit-graph
/// JSON (unitgraph::load reads it back in the child).
fn script_units(bs: &BuildScript, raw: &[RawUnit], rewrite: Option<&noop::Rewrite>, target_dir: &str) -> (String, u64) {
    let _why = crate::why::enabled().then(|| crate::why::enter(crate::why::Cause::new(&bs.src_path, 1, "build-script-dependency-hash", "script_units")));
    let build_deps: Vec<_> = bs.build_deps.iter().filter(|(_, name)|
        !rewrite.is_some_and(|r| r.calls.iter().any(|(dep, _)| dep == name))).cloned().collect();
    // reachable raw units, in a stable order
    // Start from retained roots: another build-dependency may still need an
    // omitted helper's crate or some of its transitive dependencies.
    let mut seen: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = build_deps.iter().map(|d| d.0).collect();
    while let Some(i) = stack.pop() {
        if i >= raw.len() || seen.contains(&i) {
            continue;
        }
        seen.push(i);
        stack.extend(raw[i].deps.iter().map(|d| d.0));
    }
    seen.sort();
    // compile units of build scripts: deps[0] of a run unit
    let compile: Vec<usize> = seen.iter().filter(|&&i| raw[i].run_script).filter_map(|&i| raw[i].deps.first().map(|d| d.0)).collect();
    let idx = |i: usize| seen.iter().position(|&x| x == i);
    let q = |s: &str| {
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
    };
    let mut h = fnv(bs.src_path.as_bytes(), 0xcbf29ce484222325);
    let mut units: Vec<String> = Vec::new();
    let deps_json = |deps: &[(usize, String)]| -> String {
        deps.iter().filter_map(|(d, n)| idx(*d).map(|k| format!("{{\"index\":{},\"extern_crate_name\":{}}}", k, q(n)))).collect::<Vec<_>>().join(",")
    };
    for &i in &seen {
        let u = &raw[i];
        h = fnv(u.src_path.as_bytes(), h);
        if let Ok(src) = crate::vfs::read(&u.src_path) {
            h = fnv(&src, h);
        }
        for f in &u.features {
            h = fnv(f.as_bytes(), h);
        }
        let (kind, mode) = if u.run_script {
            ("custom-build", "run-custom-build")
        } else if compile.contains(&i) {
            ("custom-build", "build")
        } else if u.proc_macro {
            ("proc-macro", "build")
        } else {
            ("lib", "build")
        };
        let name = if kind == "custom-build" { "build-script-build".to_string() } else { u.name.clone() };
        let edition = u.edition.to_string();
        let feats: Vec<String> = u.features.iter().map(|f| q(f)).collect();
        units.push(format!(
            "{{\"pkg_id\":{},\"target\":{{\"kind\":[\"{}\"],\"name\":{},\"src_path\":{},\"edition\":\"{}\"}},\"profile\":{{\"opt_level\":\"3\"}},\"mode\":\"{}\",\"features\":[{}],\"dependencies\":[{}]}}",
            q(&format!("path+file://{}#{}@{}", u.pkg.0, u.pkg.1, u.pkg.2)),
            kind,
            q(&name),
            q(&u.src_path),
            edition,
            mode,
            feats.join(","),
            deps_json(&u.deps)
        ));
    }
    if let Some(rewrite) = rewrite {
        h = fnv(rewrite.source.as_bytes(), h);
    } else if let Ok(src) = crate::vfs::read(&bs.src_path) {
        h = fnv(&src, h);
    }
    for f in &bs.features {
        h = fnv(f.as_bytes(), h);
    }
    // the script: a bin of its package
    let pkg_dir = manifest_dir_of(&bs.src_path);
    let script_unit = |src_path: &str| format!(
        "{{\"pkg_id\":{},\"target\":{{\"kind\":[\"bin\"],\"name\":\"build-script-build\",\"src_path\":{},\"edition\":\"{}\"}},\"profile\":{{\"opt_level\":\"3\"}},\"mode\":\"build\",\"features\":[],\"dependencies\":[{}]}}",
        q(&format!("path+file://{}#build-script-build@0.0.0", pkg_dir)),
        q(src_path),
        bs.edition,
        deps_json(&build_deps)
    );
    units.push(script_unit(&bs.src_path));
    let root = units.len() - 1;
    let encode = |units: &[String]| format!("{{\"version\":1,\"units\":[{}],\"roots\":[{}]}}", units.join(","), root);
    let mut json = encode(&units);
    // Hash the logical graph and the source the child actually compiles before
    // substituting the hash-derived path. Edges/aliases affect the rerun key too.
    h = fnv(json.as_bytes(), h);
    if rewrite.is_some() {
        units[root] = script_unit(&format!("{}/build-scripts/{:016x}/build.rs", target_dir, h));
        json = encode(&units);
    }
    (json, h)
}

static SELF_EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The program a build script runs as: Relax running itself again. It is this process's
/// executable unless the host names another one ([`set_self_exe`]): in an Android app this
/// process is `app_process64`, and Relax's own executable is the PIE the APK carries next to the
/// host library.
pub fn set_self_exe(exe: std::path::PathBuf) {
    let _ = SELF_EXE.set(crate::zone_alloc::kept_clone(&exe));
}

fn self_exe() -> std::io::Result<std::path::PathBuf> {
    match SELF_EXE.get() {
        Some(e) => Ok(e.clone()),
        None => std::env::current_exe(),
    }
}

/// (stdout of the script, OUT_DIR). The script runs as a Relax program: this executable started
/// again on the script's units (script_units) -- no rustc, no script binary; Relax re-running
/// itself is the only process it starts. A script of a git run reads its package through the
/// VFS (MAKEPAD_RUN_VFS / MAKEPAD_RUN_WORKSPACE for the child), and writes only OUT_DIR (on disk).
#[allow(clippy::too_many_arguments)]
fn run_one(sess: &std::sync::Arc<crate::Session>, bs: &BuildScript, raw: &[RawUnit], cenv: &[(String, String)], dir: &str, pname: &str, ver: &str, name: &str, target_dir: &str) -> (Result<String, String>, String) {
    if let Err(e) = crate::vfs::read(&bs.src_path) {
        return (Err(format!("{}: {}", bs.src_path, e)), String::new());
    }
    let rewrite = noop::rewrite(bs, raw);
    if sess.runtime.profile {
        if let Some(rewrite) = &rewrite {
            for (dep, function) in &rewrite.calls {
                eprintln!("makepad-run build-script: {} runs without {}::{}() (inert)", pname, dep, function);
            }
        }
    }
    let (units, h) = script_units(bs, raw, rewrite.as_ref(), target_dir);
    let mut fh = h;
    for f in &bs.features {
        fh = fnv(f.as_bytes(), fh);
    }
    let unit_dir = format!("{}/build-scripts/{:016x}", target_dir, h);
    let units_path = format!("{}/units.json", unit_dir);
    let out_dir = format!("{}/build/{}-{:016x}/out", target_dir, name, fh);
    {
        let _writing = path_lock(&units_path);
        let source_path = format!("{}/build.rs", unit_dir);
        // Publish the rewritten source before the units that point to it, using
        // the same path lock and atomic replacement as units.json.
        for (path, content) in rewrite.as_ref().map(|r| (source_path.as_str(), r.source.as_bytes()))
            .into_iter().chain(std::iter::once((units_path.as_str(), units.as_bytes()))) {
            if crate::vfs::read(path).ok().as_deref() != Some(content) {
                let _ = std::fs::create_dir_all(&unit_dir);
                let tmp = format!("{}.tmp{}-{}", path, std::process::id(), TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                if std::fs::write(&tmp, content).and_then(|_| std::fs::rename(&tmp, path)).is_err() {
                    return (Err(format!("{}: cannot write", path)), out_dir);
                }
            }
        }
    }
    let _ = std::fs::create_dir_all(&out_dir);
    let exe = match self_exe() {
        Ok(e) => e,
        Err(e) => return (Err(format!("this executable: {e}")), out_dir),
    };
    // nesting: a build-dependency's own script runs inside a script's run (depth 2); anything
    // deeper is a loop (MAKEPAD_RUN_SCRIPT_DEPTH counts the levels)
    let depth: u32 = std::env::var("MAKEPAD_RUN_SCRIPT_DEPTH").ok().and_then(|d| d.parse().ok()).unwrap_or(0) + 1;
    if depth > 2 {
        return (Err(format!("{}: build scripts nest {} deep (a script that runs itself again?); refused", bs.src_path, depth)), out_dir);
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["run", "--quiet", "--silent", "--run-dir", target_dir, "--units", &units_path, "build-script-build"]);
    cmd.env("OUT_DIR", &out_dir)
        // Build scripts may test this to skip work only rustc needs.
        .env("MAKEPAD_RUN_BUILD_SCRIPT", "1")
        .env("TARGET", target_triple(sess))
        .env("HOST", host_triple(sess))
        .env("PROFILE", "release")
        .env("OPT_LEVEL", "3")
        .env("DEBUG", "false")
        .env("NUM_JOBS", "4")
        .env("CARGO_MANIFEST_DIR", dir)
        .env("CARGO_PKG_NAME", pname)
        .env("CARGO_PKG_VERSION", ver)
        .env("CARGO_PKG_VERSION_PRE", version_parts(ver).1)
        .envs(crate::resolve::package_meta_env(dir))
        // cargo gives scripts RUSTC; theirs is Relax itself, answering `--version` (relax_as_rustc).
        // RUSTDOC too (the `built` crate requires it); nothing documents here
        .env("RUSTC", &exe)
        .env("RUSTDOC", "rustdoc")
        .env("MAKEPAD_RUN_AS_RUSTC", "1")
        .env("MAKEPAD_RUN_SCRIPT_DEPTH", depth.to_string())
        .env_remove("MAKEPAD_RUN_SELF_EXEC")
        // the script is compiled and run for the machine (cargo: build scripts are host
        // artifacts); the target it builds for reaches it only as TARGET / CARGO_CFG_TARGET_*
        .env_remove("MAKEPAD_RUN_CHECK_TARGET");
    for (k, v) in crate::cfg::CfgSet::host(sess, &[]).build_script_env() {
        cmd.env(k, v);
    }
    for (k, v) in cenv {
        cmd.env(k, v);
    }
    cmd.env("CARGO_ENCODED_RUSTFLAGS", "");
    for f in &bs.features {
        cmd.env(format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_")), "1");
    }
    // the package folder is the script's current folder (cargo runs scripts there); a virtual one
    // is handed over as the child's virtual workspace with the mount table
    if crate::vfs::is_virtual(dir) {
        cmd.env("MAKEPAD_RUN_WORKSPACE", dir).env("MAKEPAD_RUN_VFS", crate::vfs::mounts_text());
    } else {
        // a package on disk (a registry crate of a store run): its folder is the current folder,
        // not the run's virtual workspace (Relax's std resolves relative paths against
        // MAKEPAD_RUN_WORKSPACE first: rustix's build.rs probed `src/backend/linux_raw/arch/x86_64.rs`
        // there, found nothing and chose its libc backend)
        cmd.current_dir(dir).env_remove("MAKEPAD_RUN_WORKSPACE");
    }
    // cargo's rerun rules: the previous output stands while the script's units, the environment
    // it was given and its `rerun-if-changed` / `rerun-if-env-changed` inputs are unchanged (a
    // script that names none: no file of its package changed)
    let mut env_key = format!("{:016x}", h);
    for (k, v) in cmd.get_envs() {
        if k == "MAKEPAD_RUN_VFS" {
            continue;
        }
        env_key.push_str(&format!("\n{}={}", k.to_string_lossy(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    let build_dir = dir_of_path(&out_dir);
    let output_path = format!("{}/output", build_dir);
    let key_path = format!("{}/makepad-run-rerun", build_dir);
    let env_path = format!("{}/makepad-run-rerun-env", build_dir);
    let why = match (std::fs::read_to_string(&output_path), std::fs::read_to_string(&key_path)) {
        (Ok(old), Ok(key)) => {
            let now = rerun_key(&old, dir, &env_key).unwrap_or_default();
            if now == key {
                if sess.runtime.profile { eprintln!("makepad-run build-script: {} cached", pname); }
                return (Ok(old), out_dir);
            }
            rerun_reason(&key, &now, &std::fs::read_to_string(&env_path).unwrap_or_default(), &env_key)
        }
        _ => "no previous output".to_string(),
    };
    // why this script runs again, kept beside its output (relax-rerun-why: the last 20) and shown with MAKEPAD_RUN_PROFILE
    if sess.runtime.profile { eprintln!("makepad-run build-script: {} runs: {}", pname, why); }
    note_rerun(&build_dir, &why);
    let _slot = script_slot();
    let started = std::time::Instant::now();
    let output = cmd.output();
    if sess.runtime.profile {
        eprintln!("makepad-run build-script: {} executed {:.3} ms", pname, started.elapsed().as_secs_f64() * 1000.0);
    }
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            if let Some(key) = rerun_key(&stdout, dir, &env_key) {
                let _ = std::fs::write(&output_path, &stdout);
                let _ = std::fs::write(&key_path, key);
                let _ = std::fs::write(&env_path, &env_key);
            } else {
                let _ = std::fs::remove_file(&key_path);
            }
            (Ok(stdout), out_dir)
        }
        Ok(o) => {
            // the cause: the panic or error the script printed (Relax's report lines carry the
            // panic message as "message"), else its last lines
            let err = String::from_utf8_lossy(&o.stderr);
            let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
            let cause = lines
                .iter()
                .find_map(|l| l.split("\"message\":\"").nth(1).and_then(|r| r.split("\",\"").next()))
                .map(|m| m.to_string())
                .or_else(|| lines.iter().find(|l| l.contains("panicked") || l.starts_with("error")).map(|l| l.to_string()))
                .unwrap_or_else(|| lines.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | "));
            {
                #[cfg(unix)]
                let signal = {
                    use std::os::unix::process::ExitStatusExt;
                    o.status.signal()
                };
                #[cfg(not(unix))]
                let signal: Option<i32> = None;
                let how = match (o.status.code(), signal) {
                    (Some(c), _) => format!("exit {}", c),
                    (None, Some(sig)) => format!("killed by signal {}", sig),
                    _ => "exit -1".to_string(),
                };
                (Err(format!("{}: build script failed ({}): {}", bs.src_path, how, cause)), out_dir)
            }
        }
        Err(e) => (Err(format!("{} (build script of {}): {}", exe.display(), name, e)), out_dir),
    }
}

/// Why a stored rerun key no longer matches: the first differing line, and for the environment
/// line the variables that differ from the environment the script last ran with.
fn rerun_reason(stored: &str, now: &str, stored_env: &str, env_key: &str) -> String {
    let (sl, nl): (Vec<&str>, Vec<&str>) = (stored.lines().collect(), now.lines().collect());
    if sl.first() != nl.first() {
        let old: Vec<&str> = stored_env.lines().skip(1).collect();
        let new: Vec<&str> = env_key.lines().skip(1).collect();
        let mut d: Vec<String> = Vec::new();
        if stored_env.lines().next() != env_key.lines().next() {
            d.push("the script's units changed".to_string());
        }
        for l in &new {
            if !old.contains(l) { d.push(format!("+{}", l)); }
        }
        for l in &old {
            if !new.contains(l) { d.push(format!("-{}", l)); }
        }
        if stored_env.is_empty() { d.push("(no stored environment)".to_string()); }
        return format!("environment changed: {}", d.join(" "));
    }
    for (i, l) in nl.iter().enumerate() {
        if sl.get(i) != Some(l) {
            return format!("input changed: {} (was {})", l, sl.get(i).unwrap_or(&"-"));
        }
    }
    format!("input removed: {}", sl.get(nl.len()).unwrap_or(&"-"))
}

/// Appends `why` to <build dir>/relax-rerun-why, keeping the last 20 lines.
fn note_rerun(build_dir: &str, why: &str) {
    let path = format!("{}/makepad-run-rerun-why", build_dir);
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().map(str::to_string).collect();
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    lines.push(format!("{} pid {} {}", t, std::process::id(), why));
    let skip = lines.len().saturating_sub(20);
    let _ = std::fs::write(&path, lines[skip..].join("\n") + "\n");
}

fn dir_of_path(p: &str) -> String {
    std::path::Path::new(p).parent().map(|d| d.display().to_string()).unwrap_or_default()
}

/// The rerun key of a script output: the environment it ran with, then per
/// `rerun-if-changed` path its newest modification time (directories: recursively) and per
/// `rerun-if-env-changed` variable its value; an output that names none: the package's newest file.
fn rerun_key(output: &str, pkg_dir: &str, env_key: &str) -> Option<String> {
    let mut key = format!("{:016x}\n", fnv(env_key.as_bytes(), 0xcbf29ce484222325));
    let mut any = false;
    for line in output.lines() {
        let rest = match line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:")) {
            Some(r) => r,
            None => continue,
        };
        if let Some(p) = rest.strip_prefix("rerun-if-changed=") {
            any = true;
            let path = if std::path::Path::new(p).is_absolute() { p.to_string() } else { format!("{}/{}", pkg_dir, p) };
            match crate::vfs::identity(&path) {
                Some(id) => key.push_str(&format!("f {} {}\n", id, path)),
                None => key.push_str(&format!("f {} {}\n", newest_mtime(std::path::Path::new(&path)), path)),
            }
        } else if let Some(v) = rest.strip_prefix("rerun-if-env-changed=") {
            any = true;
            key.push_str(&format!("e {}={:?}\n", v, std::env::var(v).ok()));
        }
    }
    if !any {
        // cargo: a script that names no inputs reruns when any file of its package changes
        match crate::vfs::identity(pkg_dir) {
            Some(id) => key.push_str(&format!("p {} {}\n", id, pkg_dir)),
            None => key.push_str(&format!("p {} {}\n", newest_mtime_pkg(std::path::Path::new(pkg_dir)), pkg_dir)),
        }
    }
    Some(key)
}

/// newest_mtime of a package directory, without its target directories and hidden entries.
fn newest_mtime_pkg(p: &std::path::Path) -> u128 {
    let mut t = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            if n.starts_with("target") || n.starts_with('.') {
                continue;
            }
            t = t.max(newest_mtime(&e.path()));
        }
    }
    t
}

/// Newest modification time (ns) of a file, or of everything below a directory; 0 if missing.
fn newest_mtime(p: &std::path::Path) -> u128 {
    let Ok(m) = std::fs::metadata(p) else { return 0 };
    let mut t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    if m.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                t = t.max(newest_mtime(&e.path()));
            }
        }
    }
    t
}

/// At most this many build scripts run at once (each is a whole Relax process, most of its time a
/// cold boot of std on a few threads): half the cores, 2..=8. Four slots queued two of Stage's six
/// scripts behind the others.
fn script_slots() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| (std::thread::available_parallelism().map_or(4, |n| n.get()) / 2).clamp(2, 8))
}
static SCRIPT_RUNNING: Mutex<usize> = Mutex::new(0);
static SCRIPT_FREE: relax_session::sync::Condvar = relax_session::sync::Condvar::new();

struct ScriptSlot;
impl Drop for ScriptSlot {
    fn drop(&mut self) {
        *SCRIPT_RUNNING.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        SCRIPT_FREE.notify_one();
    }
}

fn script_slot() -> ScriptSlot {
    let mut n = SCRIPT_RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    while *n >= script_slots() {
        n = SCRIPT_FREE.wait(n).unwrap_or_else(|e| e.into_inner());
    }
    *n += 1;
    ScriptSlot
}

/// A lock per output path, held while that file is written.
fn path_lock(path: &str) -> relax_session::sync::MutexGuard<'static, ()> {
    static LOCKS: Mutex<Vec<(String, &'static Mutex<()>)>> = Mutex::new(Vec::new());
    let m: &'static Mutex<()> = {
        // leaked locks and their table live for the process: the program's heap
        let _k = crate::zone_alloc::kept();
        let mut t = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        match t.iter().find(|e| e.0 == path) {
            Some(e) => e.1,
            None => {
                let m: &'static Mutex<()> = Box::leak(Box::new(Mutex::new(())));
                t.push((path.to_string(), m));
                m
            }
        }
    };
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What makepad-platform's build.rs (platform/build.rs) gives the build, answered by makepad run
/// itself from the code build.rs includes (platform/build_gen.rs), without running the script: its cfgs (`apple_sim` from the target, the
/// `lines` / `apple_bundle` words of the MAKEPAD_BUILD variable) and its two OUT_DIR files, app_meta_gen.rs (the default bundle name and
/// identifier, from the folder that holds the target folder) and app_icon_gen.rs (the icons in
/// that folder's resources/). Its link lines are the host binary's: in this mode nothing is
/// linked at run time. Its side files (makepad-platform.path, a target-folder Info.plist) are for
/// cargo runs only.
fn platform_answer(sess: &std::sync::Arc<crate::Session>, out_dir: &str) -> Result<Vec<String>, String> {
    let env: Vec<(String, String)> = crate::cfg::CfgSet::host(sess, &[]).build_script_env();
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let target_os = get("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target = target_triple(sess);
    let makepad = std::env::var("MAKEPAD_BUILD").ok();
    let lines: Vec<String> = platform_gen::cfgs(makepad.as_deref(), &target_os, target).into_iter().map(|c| format!("cargo:rustc-cfg={}", c)).collect();
    let out = std::path::Path::new(out_dir);
    for (f, text) in [("app_meta_gen.rs", platform_gen::app_meta_gen(out)), ("app_icon_gen.rs", platform_gen::app_icon_gen(out).source)] {
        let path = format!("{}/{}", out_dir, f);
        // unchanged files stay as they are (their stamps decide what is reparsed)
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).map_err(|e| format!("{}: {}", path, e))?;
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod platform_script_tests {
    use super::platform_gen;
    use std::path::Path;
    use std::process::Command;

    /// platform/build.rs, compiled and run for real, writes the bytes `makepad run` answers with
    /// natively (both call platform/build_gen.rs): app_meta_gen.rs, app_icon_gen.rs and the cfg lines.
    #[test]
    fn native_answer_is_byte_identical_to_the_build_script() {
        let platform = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../platform");
        let tmp = std::env::temp_dir().join(format!("relax-platform-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("some app");
        std::fs::create_dir_all(root.join("resources")).unwrap();
        std::fs::write(root.join("resources/icon_64.png"), b"png").unwrap();
        std::fs::write(root.join("resources/icon.ico"), b"ico").unwrap();
        let out = root.join("target/release/build/makepad-platform-0123456789abcdef/out");
        std::fs::create_dir_all(&out).unwrap();
        let exe = tmp.join("build-script");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let built = Command::new(rustc).args(["--edition", "2021", "-o"]).arg(&exe).arg(platform.join("build.rs")).output().expect("rustc");
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        for (os, target, makepad_build) in [("linux", "x86_64-unknown-linux-gnu", None), ("ios", "aarch64-apple-ios-sim", Some("lines+apple_bundle,other"))] {
            let mut cmd = Command::new(&exe);
            cmd.current_dir(&platform).env_clear().env("OUT_DIR", &out).env("CARGO_CFG_TARGET_OS", os).env("TARGET", target);
            if let Some(b) = makepad_build { cmd.env("MAKEPAD_BUILD", b); }
            let ran = cmd.output().expect("build script");
            assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
            let said: Vec<String> = String::from_utf8_lossy(&ran.stdout).lines().filter_map(|l| l.strip_prefix("cargo:rustc-cfg=")).map(str::to_string).collect();
            let native: Vec<String> = platform_gen::cfgs(makepad_build, os, target).into_iter().map(str::to_string).collect();
            assert_eq!(said, native, "{os}");
            assert_eq!(std::fs::read_to_string(out.join("app_meta_gen.rs")).unwrap(), platform_gen::app_meta_gen(&out));
            assert_eq!(std::fs::read_to_string(out.join("app_icon_gen.rs")).unwrap(), platform_gen::app_icon_gen(&out).source);
        }
        assert!(platform_gen::app_meta_gen(&out).contains("\"Some app\""));
        assert!(platform_gen::app_icon_gen(&out).source.contains("icon_64.png"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
