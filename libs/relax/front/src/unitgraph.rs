//! The crate graph of one cargo package for `makepad run`: cargo's unit graph (lib, proc-macro
//! and the package's bin), build-script results (cfgs, link libraries, OUT_DIR, rustc-env) and
//! the per-crate compile-time environment that `env!` reads (CARGO_MANIFEST_DIR,
//! CARGO_PKG_*, OUT_DIR).
//!
//! Build scripts run as Relax programs: this executable started again on the script's units (its
//! build-dependencies and the script as a bin, run_one), with cargo's environment; its `cargo:`
//! lines are read like cargo does, the output kept with cargo's rerun rules. No rustc.

use crate::crates::{CrateUnit, HostUnit};
pub use crate::resolve::{GraphUnit, Units};
use relax_session::sync::Mutex;

#[path = "unitgraph_noop.rs"]
mod noop;

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
    /// the scripts (indices into `Graph::build_scripts`) of the package's dependencies with
    /// `links =`: cargo runs them first and gives this script their metadata as
    /// `DEP_<LINKS>_<KEY>` (the unit graph lists their runs among this run's dependencies)
    pub links_deps: Vec<usize>,
    /// its package's manifest keeps it (`[package.metadata.relax] build-script = "keep"`): it runs
    /// as a Relax program, unless a host answers it (`ScriptAnswer`)
    pub kept: bool,
    /// what the script is told of its profile (PROFILE, OPT_LEVEL, DEBUG): a run's builds are
    /// release's, a test's its profile's (relax-compiler run.rs)
    pub profile: crate::resolve::ScriptProfile,
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
    /// a path package outside the sysroot (its cargo pkg_id is `path+`): its files are its source,
    /// where a registry or git package is its id and a sysroot crate is Relax's own
    pub path_source: bool,
    /// the checks its profile gives the unit as a host unit, which a build script's run builds it
    /// with (`makepad test`'s profile: run.rs; a build script's run: its graph's)
    pub host_checks: Option<crate::resolve::ProfileChecks>,
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

/// `makepad check`'s graph: its target flags are cargo check's (resolve.rs parse_check_flags),
/// `--tests` the lib's and bins' unit tests and the integration tests, `--lib` the plain lib.
pub fn check_unit_graph(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, cargo_flags: &[String]) -> Result<Units, String> {
    crate::resolve::check_unit_graph(sess, workspace, package, cargo_flags)
}

pub fn load(g: &Units, check_all_bins: bool) -> Result<Graph, String> {
    let units = &g.units;
    let roots = &g.roots;
    let mut crates: Vec<CrateUnit> = Vec::new();
    // Host and target copies share a crate; test and build modes do not.
    let mut crate_paths: Vec<(String, bool)> = Vec::new();
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
    let mut raw: Vec<RawUnit> = vec![RawUnit { name: String::new(), src_path: String::new(), edition: 2015, features: Vec::new(), proc_macro: false, deps: Vec::new(), run_script: false, pkg: Default::default(), path_source: false, host_checks: None }; n_units];
    // a crate used on both sides (normal dependency and build-dependency / proc-macro side) is one
    // crate here: keep the target side for the app and the host side for the macro world.
    // Cargo reference features.md#feature-resolver-version-2 keeps their features separate;
    // rustc compiler/rustc_metadata/src/creader.rs::CStore::load_proc_macro loads the host artifact.
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
                // Rust dylibs still expose a Rust crate: compile their source into the JIT
                // graph, without emitting or loading the native dylib artifact.
                "lib" | "rlib" | "dylib" => lib = true,
                "proc-macro" => pm = true,
                // an integration test target: a root like a bin (its harness runs its tests); an
                // example (`--example`) is one too
                "bin" | "test" | "example" => bin = true,
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
        raw[ui] = RawUnit { name: tname.clone(), src_path: src_path.clone(), edition, features: features.clone(), proc_macro: pm, deps: udeps.clone(), run_script: build && mode == "run-custom-build", pkg: (pkg_dir.clone(), pname.clone(), ver.clone()),
            path_source: !u.sysroot && u.pkg_id.starts_with("path+"), host_checks: u.checks };
        // the run unit depends on the script's compile unit, whose deps are the build-dependencies
        let build_unit = udeps.first().map(|d| d.0).unwrap_or(usize::MAX);
        run_build[ui] = if build && mode == "run-custom-build" { Some((src_path.clone(), edition, features.clone(), build_unit)) } else { None };
        let is_root_bin = bin && roots.contains(&ui);
        if !(lib || pm || is_root_bin) {
            continue;
        }
        let norm = (crate::vfs::normalize_path(&src_path), mode == "test");
        if let Some(i) = crate_paths.iter().position(|p| *p == norm) {
            map[ui] = i;
            if u.host {
                crates[i].host = Some(HostUnit { features, deps: udeps, build_outputs: None });
            }
            if is_root_bin || (root == usize::MAX && roots.contains(&ui)) { root = i; }
            continue;
        }
        if is_root_bin {
            // rustc_session output.rs validate_crate_name: non-empty, alphanumeric or `_` only
            // (one error per offending character; the first fatal stops before any input is read)
            if tname.is_empty() {
                return Err("crate name must not be empty".to_string());
            }
            if let Some(c) = tname.chars().find(|c| !(c.is_alphanumeric() || *c == '_')) {
                return Err(format!("invalid character '{c}' in crate name: `{tname}`"));
            }
            root_bins.push((crates.len(), u.name.clone(), pkg_dir.clone()));
        }
        if is_root_bin || (root == usize::MAX && roots.contains(&ui)) {
            root = crates.len();
        }
        map[ui] = crates.len();
        if mode == "test" {
            test_crates.push(crates.len());
        }
        let tname = if u.sysroot { tname } else { crate::crates::own_crate_name(tname) };
        crates.push(CrateUnit { name: tname, src_path, edition, features, deps: udeps, host: None, proc_macro: pm, bin, harness: u.harness, build_script: false, version: ver.clone(), registry: !u.pkg_id.starts_with("path+"), host_only: u.host });
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
                build_scripts.push(BuildScript { krate: ci, also: Vec::new(), src_path: p.clone(), edition: *ed, features: f.clone(), build_deps, links_deps: Vec::new(), kept: kept_script(&pkg[ci].0), profile: crate::resolve::ScriptProfile::RUN });
            }
        }
        c.deps = nd;
        if let Some(h) = &mut c.host {
            h.deps = h.deps.iter().filter_map(|(i, n)| {
                map.get(*i).filter(|&&ci| ci != usize::MAX).map(|&ci| (ci, n.clone()))
            }).collect();
        }
    }
    // a script run's dependencies on other runs: those of its package's dependencies with `links`
    for (&run, &s) in &script_of_run {
        build_scripts[s].links_deps = raw[run].deps.iter().filter_map(|(d, _)| script_of_run.get(d).copied()).filter(|&t| t != s).collect();
    }

    let n = crates.len();
    let mut env = Vec::new();
    for (dir, name, ver) in &pkg {
        env.push(pkg_env(dir, name, ver));
    }
    for (ci, c) in crates.iter().enumerate() {
        env[ci].push(("CARGO_CRATE_NAME".to_string(), c.name.clone()));
    }
    // cargo's integration-test environment: CARGO_TARGET_TMPDIR and CARGO_BIN_EXE_<bin>
    let integration_tests: Vec<usize> = test_crates.iter().copied().filter(|&ci| crates[ci].bin && units.iter().any(|u| crate::vfs::normalize_path(&u.src_path) == crate_paths[ci].0 && u.kind.iter().any(|k| k == "test"))).collect();
    if !integration_tests.is_empty() {
        let target_dir = crate::dirs::relax_dir();
        let tmpdir = target_dir.join("tmp");
        let _ = std::fs::create_dir_all(&tmpdir);
        let tmpdir_str = tmpdir.display().to_string();
        for &ci in &integration_tests {
            env[ci].push(("CARGO_TARGET_TMPDIR".to_string(), tmpdir_str.clone()));
        }
        if !g.bins.is_empty() {
            if let Ok(exe) = self_exe() {
                let (pkg_dir, pkg_name, _) = &pkg[integration_tests[0]];
                let bin_dir = target_dir.join("relax-bin");
                let _ = std::fs::create_dir_all(&bin_dir);
                // absolute: the wrapper runs wherever a test starts it from (its own folder, a copy)
                let absolute = |p: &std::path::Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string();
                let td_str = absolute(&target_dir);
                let pkg_dir = &absolute(std::path::Path::new(pkg_dir));
                let exe_str = exe.display().to_string();
                let sh_quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
                // cargo builds a package's bins with the features its tests are built with (one
                // feature set per package): the run of the bin selects exactly those
                let features = crates[integration_tests[0]].features.join(",");
                let allow_c = std::env::var_os("RELAX_ALLOW_C").is_some_and(|v| v == "1");
                // a file the bin's starts read, replaced whole (another run's test may be reading it)
                let put = |path: &std::path::Path, text: &str| {
                    if std::fs::read(path).ok().as_deref() == Some(text.as_bytes()) {
                        return;
                    }
                    let tmp = bin_dir.join(format!(".{}.tmp{}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
                    if std::fs::write(&tmp, text).is_ok() {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
                        }
                        let _ = std::fs::rename(&tmp, path);
                    }
                };
                for bin_name in &g.bins {
                    // the bin's run: the loader's command line before the bin's own arguments
                    let mut words: Vec<String> = [exe_str.as_str(), "run", "--quiet", "--silent", "--no-live", "--run-dir", td_str.as_str(), "--workspace-dir", pkg_dir.as_str(), pkg_name.as_str(), "--no-default-features"].iter().map(|w| w.to_string()).collect();
                    // the test run's native permission is the binary's, as cargo builds the binary
                    // with the test: a test that clears its child's environment (or takes
                    // RELAX_ALLOW_C out of it) still starts the binary (relax issue 157)
                    if allow_c {
                        words.push("--allow-c".to_string());
                    }
                    if !features.is_empty() {
                        words.extend(["--features".to_string(), features.clone()]);
                    }
                    words.extend(["--bin".to_string(), bin_name.clone(), "--".to_string()]);
                    let wrapper = bin_dir.join(if cfg!(windows) { format!("{bin_name}.cmd") } else { bin_name.clone() });
                    if cfg!(windows) {
                        let line: Vec<String> = words.iter().map(|w| format!("\"{w}\"")).collect();
                        put(&wrapper, &format!("@echo off\r\nset RELAX_SELF_EXEC=\r\nset RELAX_SELF_EXEC_SILENT_ADDED=\r\nset \"RELAX_ARG0=%~f0\"\r\n{} %*\r\n", line.join(" ")));
                        // the words alone, for the `.exe` a test finds above its `deps/` folder,
                        // where cargo has the binary (relax-compiler selfexe.rs add_bins)
                        put(&wrapper.with_extension("words"), &words.join("\u{1f}"));
                    } else {
                        let line: Vec<String> = words.iter().map(|w| sh_quote(w)).collect();
                        put(&wrapper, &format!("#!/bin/sh\nunset RELAX_SELF_EXEC RELAX_SELF_EXEC_SILENT_ADDED\nRELAX_ARG0=$0\nexport RELAX_ARG0\nexec {} \"$@\"\n", line.join(" ")));
                    }
                    let wrapper_str = wrapper.display().to_string();
                    for &ci in &integration_tests {
                        env[ci].push((format!("CARGO_BIN_EXE_{bin_name}"), wrapper_str.clone()));
                    }
                }
            }
        }
    }
    crate::crates::disambiguate_multi_version_crates(&mut crates);
    let mut cfgs = vec![Vec::new(); n];
    for &ci in &test_crates {
        cfgs[ci].push("test".to_string());
    }
    // a build script's run: the checks its parent's profile gives these host units (script_units)
    for (ui, u) in units.iter().enumerate() {
        if let (Some(c), Some(cs)) = (u.checks, map.get(ui).and_then(|&k| cfgs.get_mut(k))) {
            for x in c.cfgs() {
                if !cs.iter().any(|y| y == x) {
                    cs.push(x.to_string());
                }
            }
        }
    }
    // a single integration test is the graph's only test root: it is listed like several
    if bins.is_empty() && test_crates.contains(&root) && crates[root].bin {
        bins.push(root);
    }
    Ok(Graph { crates, root, build_scripts, pkg, cfgs, env, link_libs: Vec::new(), native: None, raw, bins })
}

/// cargo's per-package environment (CARGO_MANIFEST_DIR, CARGO_MANIFEST_PATH, CARGO_PKG_*): its
/// crates' at compile time, its build script's and its targets' at run time.
fn pkg_env(dir: &str, name: &str, ver: &str) -> Vec<(String, String)> {
    // CARGO: the cargo of the build by an absolute path, as cargo gives it to rustc (env!, proc
    // macros that run `$CARGO metadata`) and to the program it runs (relax-compiler run.rs
    // program_env); a build script's is the loader (run_one). None without a cargo, never a bare
    // `cargo` looked up on PATH at each spawn
    let mut e: Vec<(String, String)> = crate::dirs::cargo_exe().map(|c| ("CARGO".to_string(), c)).into_iter().collect();
    e.extend([
        ("CARGO_MANIFEST_DIR".to_string(), dir.to_string()),
        ("CARGO_MANIFEST_PATH".to_string(), crate::crates::join_path(dir, "Cargo.toml")),
        ("CARGO_PKG_NAME".to_string(), name.to_string()),
        ("CARGO_PKG_VERSION".to_string(), ver.to_string()),
    ]);
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

/// The triple the checked crate graph is read for (RELAX_CHECK_TARGET, else the machine).
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
/// What Relax does with a package's build script. A package keeps its script by its manifest
/// (`[package.metadata.relax] build-script = "keep"`), and a host can answer a script natively
/// (`register_script_answer`). Probe scripts of third-party crates Relax answers itself. Every
/// other script is to be removed from the source; until it is, it still runs and `run` / `check`
/// list it (`to_remove`).
pub enum ScriptRole {
    /// kept: runs (as a Relax program), its output cached by the rerun rules as cargo does
    Artefact,
    /// a host's answer (`ScriptAnswer`): the script's `cargo:` lines and OUT_DIR files without
    /// running it
    Native(&'static ScriptAnswer),
    /// the script's `cargo:` lines as Relax knows them, without running it
    Answered(Vec<String>),
    /// a Relax crate's own source stamp (libs/relax/self_stamp.rs): answered by `self_stamp_text`
    SelfStamp,
    /// to be removed from the source
    ToRemove,
}

pub fn script_role(pkg: &str, bs: &BuildScript) -> ScriptRole {
    let has = |f: &str| bs.features.iter().any(|x| x == f);
    if let Some(a) = script_answer(pkg) {
        return ScriptRole::Native(a);
    }
    match pkg {
        // Relax's crates compiled from source by a program (makepad-script-compute's non-provided
        // code reaches relax-backend): their script only writes the source stamp
        "relax-base" | "relax-lex" | "relax-session" | "relax-types" | "relax-backend" | "relax-codegen" | "relax-front" => ScriptRole::SelfStamp,
        _ if bs.kept => ScriptRole::Artefact,
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

/// The value of `key` in the `[package.metadata.relax]` section of the manifest of the package
/// in `dir`, as written (`"keep"`, `true`).
fn relax_meta(dir: &str, key: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    let text = crate::vfs::read_to_string(std::path::Path::new(dir).join("Cargo.toml")).ok()?;
    let mut inside = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            inside = line == "[package.metadata.relax]";
        } else if let Some((k, v)) = line.split_once('=').filter(|_| inside) {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// `[package.metadata.relax] build-script = "keep"` in the manifest of the package in `dir`.
fn kept_script(dir: &str) -> bool {
    relax_meta(dir, "build-script").as_deref() == Some("\"keep\"")
}

/// `[package.metadata.relax] window-host = true`: the package hosts a program's windows (an
/// app's start opens its window early, the main thread serves it, and a run that provides the
/// crate is always hosted).
pub fn window_host(dir: &str) -> bool {
    relax_meta(dir, "window-host").as_deref() == Some("true")
}

/// `[package.metadata.relax] presents = true`: a program that links the package presents frames
/// and ends each with `relax_frame_done` (live edits wait for a present; the first frame is
/// watched).
pub fn presents(dir: &str) -> bool {
    relax_meta(dir, "presents").as_deref() == Some("true")
}

/// `[package.metadata.relax] export-opt-level = <n>`: the opt-level the package's own code ships
/// at in a web export (wasm_export.rs o3_crate: 3 takes the O3 pipeline), as rustc's web build
/// compiles that crate.
pub fn export_opt_level(dir: &str) -> Option<u8> {
    relax_meta(dir, "export-opt-level")?.parse().ok()
}

/// A host's own answer to a package's build script: what the script prints (`cargo:` lines) and
/// writes to its OUT_DIR, from the code the script shares, without running it. Its cfg lines and
/// OUT_DIR files key the crate as a run script's output does (crate_image Record: cfg, files).
pub struct ScriptAnswer {
    pub package: &'static str,
    pub answer: fn(&ScriptEnv) -> Result<Vec<String>, String>,
}

/// What the script would be given: cargo's build-script environment (CARGO_CFG_*, TARGET, ..),
/// the target triple and its OUT_DIR (made, empty or as the last answer left it).
pub struct ScriptEnv<'a> {
    pub env: &'a [(String, String)],
    pub target: &'a str,
    pub out_dir: &'a std::path::Path,
}

static SCRIPT_ANSWERS: Mutex<Vec<&'static ScriptAnswer>> = Mutex::new(Vec::new());

/// A host's answer (Host.script_answers), registered before any graph is made; one per package.
pub fn register_script_answer(a: &'static ScriptAnswer) {
    let mut v = SCRIPT_ANSWERS.lock().unwrap();
    assert!(!v.iter().any(|x| x.package == a.package), "two answers to {}'s build script", a.package);
    v.push(a);
}

fn script_answer(pkg: &str) -> Option<&'static ScriptAnswer> {
    SCRIPT_ANSWERS.lock().unwrap().iter().copied().find(|a| a.package == pkg)
}

/// Packages whose build script ran although it is to be removed: (package, script path).
pub static TO_REMOVE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// `check_only` (`relax check`): a script that compiles C is not run without `--allow-c` (rustc's
/// check never links native code): its crate gets an empty answer (OUT_DIR, no cfgs), said once,
/// and so does a script that reads such a script's `DEP_` metadata. Any other script that fails
/// fails the check, as under `cargo check`.
pub type ScriptsDone<'a> = dyn Fn(&Graph, &[usize], &[Vec<String>], &[Vec<(String, String)>]) -> Option<Box<dyn std::any::Any>> + Sync + 'a;

/// `scripts_done`: called once the scripts Relax runs have finished while cargo is still building
/// the native packages (a cold `--allow-c` start: the C compile takes tens of seconds), with the
/// crates' cfgs and env so far (the native packages' outputs missing). The boot starts the
/// proc-macro world from it, beside the C compile. What it returns is kept until cargo is done.
///
/// `native_receipt`: the native build of the run that started this one (`Inherit`).
///
/// Its result: the receipt of the native build this run made, for the runs that take it: the
/// package binaries its integration tests start (CARGO_BIN_EXE_<name>, `BIN_NATIVE_RECEIPT`) and a
/// run of this program again (run.rs, `RELAX_RUN_NATIVE`).
pub fn run_build_scripts(sess: &std::sync::Arc<crate::Session>, g: &mut Graph, target_dir: &str, native: Option<crate::native::Mode<'_>>, check_only: bool,
    native_receipt: Option<Inherit<'_>>, scripts_done: Option<&ScriptsDone<'_>>) -> Result<Option<String>, String> {
    // the packages whose scripts compile or link C (native.rs): built by cargo with `--allow-c`,
    // an error naming the flag without it; Relax runs the other scripts. A check without the flag
    // reads the cfgs, env and OUT_DIR a native build already in the cache produced, so it needs
    // no cargo and no link of its own.
    let builds = native.is_some_and(|m| m.builds());
    let native_scripts: Vec<(usize, crate::native::NativeReason)> = g.build_scripts.iter().enumerate().filter_map(|(i, bs)| {
        let (_, pname, _) = &g.pkg[bs.krate];
        // A kept GPU-kernel script (`Artefact`) that links native code (makepad-ai-cuda's nvcc
        // archives, `links =`) is cargo's too with `--allow-c`: its archives join the run's
        // native library. Without the flag it runs under Relax as before.
        let kept_native = builds && matches!(script_role(pname, bs), ScriptRole::Artefact);
        if noop::script(bs, &g.raw) || !(kept_native || matches!(script_role(pname, bs), ScriptRole::ToRemove)) {
            return None;
        }
        crate::native::native_reason(g, bs).map(|reason| (i, reason))
    }).collect();
    let seeds: Vec<usize> = native_scripts.iter().map(|(i, _)| *i).collect();
    // the native build's outputs: cached under this run's key, or the receipt of the run that
    // started this one, matched by the scripts' paths (a build script's own child run builds
    // nothing; a binary its test started builds only what its test's build lacks). A binary takes
    // one only in a run with `--allow-c`: a receipt grants no native library (relax issue 168).
    let inherited = match native_receipt {
        Some(Inherit::Script(r)) => Some((r, false)),
        Some(Inherit::Binary(r)) if !seeds.is_empty() && builds => Some((r, true)),
        _ => None,
    }.and_then(|(r, binary)| {
        let mut taken = crate::native::read_receipt(r).and_then(|rc| Some((inherited_scripts(g, &seeds, &rc.scripts, binary)?, rc.lib)));
        if binary && taken.is_none() {
            eprintln!("relax run: the native build {r} does not cover this build's native packages; this run builds its own");
        }
        // a build script's run loads its parent's library only with the parent's `--allow-c`,
        // which the parent passes it (run_one): the receipt names it and grants nothing; its
        // outputs (cfgs, OUT_DIR) still compile the script
        if let Some((_, lib)) = taken.as_mut().filter(|_| !binary && !sess.options.allow_c) {
            if lib.take().is_some() {
                eprintln!("relax run: the native library RELAX_NATIVE_RECEIPT names is not loaded: this run has no --allow-c");
            }
        }
        taken
    });
    let native = match native_receipt {
        Some(Inherit::Binary(_)) if inherited.is_some() => None,
        _ => native,
    };
    let pre = match native {
        Some(m) if !m.builds() && !seeds.is_empty() => crate::native::cached_outputs(g, &seeds, m.request()),
        Some(_) => None,
        None => inherited,
    };
    // what this run compiles the native packages with: a check's cached build links nothing and
    // loads no library; a child run calls into its parent's
    let cached = match (native, &pre) {
        (Some(m), Some((scripts, _))) if !m.builds() => Some((scripts.clone(), None)),
        (None, Some(p)) => Some(p.clone()),
        _ => None,
    };
    let mut native_seeds: Vec<usize> = Vec::new();
    let mut check_skipped: Vec<usize> = Vec::new();
    for (i, reason) in native_scripts {
        let bs = &g.build_scripts[i];
        let (_, pname, _) = &g.pkg[bs.krate];
        let why = reason.describe(pname);
        if builds || cached.is_some() {
            if builds && sess.runtime.profile {
                eprintln!("relax build-script: {} is native ({})", pname, why);
            }
            native_seeds.push(i);
            continue;
        }
        // a script that runs a C compiler cannot work without one; `links =` / DEP_ metadata
        // alone run under Relax as before
        if reason.compiles_c() && check_only {
            eprintln!("relax check: `{}`'s build script compiles C ({}): not run without `--allow-c` (its cfgs and OUT_DIR are missing; a `--allow-c` run fills the cache this check reads)", pname, why);
            check_skipped.push(bs.krate);
            continue;
        }
        if reason.compiles_c() {
            return Err(format!("`{}` compiles C ({}): it {}", pname, why, relax_session::options::C_GATE));
        }
    }
    let native_krates: Vec<usize> = native_seeds.iter().map(|&i| g.build_scripts[i].krate).collect();
    // The scripts Relax runs whose build-dependencies reach a native package: their child runs
    // compile it, so they read the native outputs from a receipt (native.rs `write_receipt`).
    // Only they wait for a cold native build and only their rerun keys hold the receipt's name.
    let reach: Vec<bool> = g.build_scripts.iter().map(|bs| {
        !native_krates.is_empty() && !native_krates.contains(&bs.krate) && !check_skipped.contains(&bs.krate) && reaches_native(g, bs, &native_krates)
    }).collect();
    let mut receipt: Option<String> = None;
    let mut prebuilt = false;
    if reach.contains(&true) {
        let outputs = match native {
            Some(crate::native::Mode::Build(req)) => match crate::native::cached_outputs(g, &seeds, req) {
                Some(cached) => Some(cached),
                None => {
                    // a cold build: its outputs are those scripts' inputs, so it finishes first
                    let built = crate::native::build(g, &native_seeds, req)?;
                    let outputs = (built.scripts.clone(), built.lib.clone());
                    apply_built(g, &native_krates, built)?;
                    prebuilt = true;
                    Some(outputs)
                }
            },
            _ => pre,
        };
        if let Some((scripts, lib)) = outputs {
            receipt = Some(crate::native::write_receipt(g, &seeds, &scripts, lib.as_ref(), target_dir)?);
        }
    }
    // a script a host answers (`ScriptAnswer`) starts no process;
    // in no-codegen mode (the platforms that run no generated code, iOS) no other script runs: any
    // other crate with a build script is an error.
    if sess.options.no_codegen {
        for bs in &g.build_scripts {
            let (_, pname, _) = &g.pkg[bs.krate];
            if !matches!(script_role(pname, bs), ScriptRole::Native(_) | ScriptRole::SelfStamp) && !noop::script(bs, &g.raw) {
                return Err(format!("build scripts are not supported in no-codegen mode: {} ({})", pname, bs.src_path));
            }
        }
    }
    // cargo builds the native packages (C compiles, a cold build's longest step) while Relax runs
    // the other scripts: they share nothing (each script's output is its own crate's)
    let gr: &Graph = g;
    let meta: LinksMeta = Default::default();
    let root = BuildRoot::new(sess, gr, target_dir, check_only);
    let (native_built, results) = std::thread::scope(|scope| {
        let native_job = match (native, native_seeds.is_empty()) {
            (Some(crate::native::Mode::Build(req)), false) if !prebuilt => Some(scope.spawn(|| {
                let _z = crate::zone_alloc::compiler_thread();
                crate::native::build(gr, &native_seeds, req)
            })),
            _ => None,
        };
        // with the hook: the scripts of the proc-macro world's crates first (the hook starts the
        // world from their outputs), the others beside the world's build
        let world: Vec<bool> = match (&native_job, scripts_done) {
            (Some(_), Some(_)) => {
                let names = crate::macro_reg::macro_closure_names();
                gr.crates.iter().map(|c| names.contains(&c.name)).collect()
            }
            _ => Vec::new(),
        };
        let receipts: Vec<Option<&str>> = reach.iter().map(|&r| if r { receipt.as_deref() } else { None }).collect();
        let mut results = relax_scripts(sess, gr, target_dir, &root, &native_krates, &check_skipped, &receipts, &meta, &|k| world.is_empty() || world[k]);
        let mut _kept = None;
        if let (Some(job), Some(done)) = (&native_job, scripts_done) {
            if !job.is_finished() {
                let (mut cfgs, mut env, mut libs) = (gr.cfgs.clone(), gr.env.clone(), gr.link_libs.clone());
                let mut ok = true;
                for (krates, r, out_dir) in &results {
                    let Ok(stdout) = r else { ok = false; break };
                    for &k in krates {
                        if script_output_into(&mut cfgs, &mut env, &mut libs, &gr.pkg, k, stdout, out_dir.clone(), check_only).is_err() { ok = false; }
                    }
                }
                if ok { _kept = done(gr, &native_krates, &cfgs, &env); }
            }
        }
        if !world.is_empty() {
            results.extend(relax_scripts(sess, gr, target_dir, &root, &native_krates, &check_skipped, &receipts, &meta, &|k| !world[k]));
        }
        (native_job.map(|j| j.join().unwrap_or_else(|_| Err("--allow-c: the native build panicked".to_string()))), results)
    });
    root.finish();
    // the package's binaries its integration tests start, and a run of this program again,
    // compile and link with this build
    let named = !check_only && !native_seeds.is_empty();
    let mut for_bins = if named && prebuilt { receipt.clone() } else { None };
    if let Some(built) = native_built {
        let built = built?;
        if named {
            for_bins = Some(crate::native::write_receipt(g, &native_seeds, &built.scripts, built.lib.as_ref(), target_dir)?);
        }
        apply_built(g, &native_krates, built)?;
    } else if let Some((scripts, lib)) = cached {
        // a check's cached build (no lib), or a child run's receipt: the same outputs cargo
        // would have produced, no cargo. A receipt's library can hold packages this program does
        // not link (a build script's run takes its parent's): their C's callbacks are not its own.
        let own: Vec<String> = native_krates.iter().map(|&k| format!("{} {}", g.pkg[k].1, g.pkg[k].2)).collect();
        g.native = lib.map(|mut l| {
            l.partial = l.crates.iter().any(|c| !own.contains(c));
            l
        });
        apply_native_scripts(g, scripts);
    }
    apply_script_results(g, results, check_only)?;
    Ok(for_bins)
}

/// The native build a run takes from the run that started it, by the receipt that run wrote
/// (`native::write_receipt`), instead of a build of its own.
#[derive(Clone, Copy)]
pub enum Inherit<'a> {
    /// A build script's own child run (`NATIVE_RECEIPT`): the native packages its
    /// build-dependencies reach, compiled for its host, which is its target.
    Script(&'a str),
    /// A package binary its integration test started (`BIN_NATIVE_RECEIPT`): cargo test builds the
    /// binary in the test's build, so the test's run built its native packages. The binary needs
    /// no cargo and no C compiler where it runs, with the PATH the test gave it (relax issue 151).
    /// A binary with a native package the test's build lacks builds its own, as any run.
    Binary(&'a str),
}

/// A `--allow-c` build into the graph: every native package's script ran under cargo (cargo sets
/// OUT_DIR for each package with a build script); one cargo did not build would compile without
/// its outputs.
fn apply_built(g: &mut Graph, native_krates: &[usize], built: crate::native::Built) -> Result<(), String> {
    for &k in native_krates {
        let (_, n, v) = &g.pkg[k];
        if !built.scripts.iter().any(|o| o.krate == k || (g.pkg[o.krate].1 == *n && g.pkg[o.krate].2 == *v)) {
            return Err(format!("--allow-c: cargo did not run the build script of `{n} {v}` (no build-script-executed message): its OUT_DIR and cfgs are unknown"));
        }
    }
    g.native = built.lib;
    apply_native_scripts(g, built.scripts);
    Ok(())
}

/// Whether `bs`'s build-dependencies reach one of the native packages (`native_krates`): its
/// child run compiles that package (a build-dependency is the same package built for the host).
fn reaches_native(g: &Graph, bs: &BuildScript, native_krates: &[usize]) -> bool {
    let native: Vec<(&str, &str)> = native_krates.iter().map(|&k| (g.pkg[k].1.as_str(), g.pkg[k].2.as_str())).collect();
    let mut seen = vec![false; g.raw.len()];
    let mut stack: Vec<usize> = bs.build_deps.iter().map(|d| d.0).collect();
    while let Some(i) = stack.pop() {
        if i >= g.raw.len() || std::mem::replace(&mut seen[i], true) {
            continue;
        }
        let u = &g.raw[i];
        if native.contains(&(u.pkg.1.as_str(), u.pkg.2.as_str())) {
            return true;
        }
        stack.extend(u.deps.iter().map(|d| d.0));
    }
    false
}

/// The native outputs a child run's receipt names, onto this graph's native scripts by their
/// paths (`native::write_receipt`). None unless every one is covered: a partial receipt is none.
/// A package the parent built with a separate host feature set has two entries: a build script's
/// child run takes the host one (its copy is a build-dependency's), a binary its test started
/// (`binary`) the target one, and the host one for its own host units.
fn inherited_scripts(g: &Graph, seeds: &[usize], records: &[crate::native::ReceiptScript], binary: bool) -> Option<Vec<crate::native::ScriptOut>> {
    let mut out = Vec::new();
    for &s in seeds {
        let bs = &g.build_scripts[s];
        let of = |r: &crate::native::ReceiptScript, host: bool| crate::native::ScriptOut { krate: bs.krate, host, cfgs: r.cfgs.clone(), env: r.env.clone(), out_dir: r.out_dir.clone(), system: r.system.clone() };
        let mine = records.iter().filter(|r| r.path == bs.src_path);
        if binary {
            // the test's target build, and its host build where its build had one
            out.push(of(mine.clone().find(|r| !r.host)?, false));
            out.extend(mine.filter(|r| r.host).map(|r| of(r, true)));
        } else {
            out.push(of(mine.max_by_key(|r| r.host)?, false));
        }
    }
    Some(out)
}

/// A native build's script outputs into the graph (cfgs, link libraries, env, OUT_DIR): what
/// cargo wrote for `--allow-c`, or what `cached_scripts` read back from its receipt.
fn apply_native_scripts(g: &mut Graph, scripts: Vec<crate::native::ScriptOut>) {
    for n in scripts {
        let also = g.build_scripts.iter().find(|b| b.krate == n.krate).map(|b| b.also.clone()).unwrap_or_default();
        for &k in std::iter::once(&n.krate).chain(&also) {
            if n.host {
                if g.crates[k].host.is_some() {
                    let (dir, name, ver) = &g.pkg[k];
                    let mut env = pkg_env(dir, name, ver);
                    env.push(("CARGO_CRATE_NAME".into(), g.crates[k].name.clone()));
                    env.extend(n.env.iter().cloned());
                    env.push(("OUT_DIR".into(), n.out_dir.clone()));
                    // Keep host-feature cfgs out of the application crate.
                    g.crates[k].host.as_mut().unwrap().build_outputs = Some((n.cfgs.clone(), env));
                }
                continue;
            }
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

/// What each script handed on to the scripts of its dependents, by script index: the metadata
/// its output carried (`script_metadata`), or why it has none (its script was not run, or failed).
/// The metadata a check without `--allow-c` hands on for a script it did not run for its C compile
/// (C_NOT_RUN), or for that script's `DEP_` metadata (C_DEP_NOT_RUN): a script that reads it is not
/// run either, and is no failure of the check.
const C_NOT_RUN: &str = "its build script compiles C, which is not run without `--allow-c`";
const C_DEP_NOT_RUN: &str = "its build script was not run, for a C build script's metadata it reads";

/// One table for both calls of `relax_scripts` (the macro world's scripts first).
type LinksMeta = Mutex<std::collections::HashMap<usize, Result<Vec<(String, String)>, String>>>;

/// The scripts Relax runs itself (every one but the native packages' and a check's skipped
/// ones), on the pool: (crates, stdout or error, OUT_DIR) per script. A script whose package
/// depends on `links` packages runs after their scripts, given their metadata (cargo orders the
/// runs the same way); most graphs have no such pair and run every script at once.
fn relax_scripts(sess: &std::sync::Arc<crate::Session>, g: &Graph, target_dir: &str, root: &BuildRoot, native_krates: &[usize], check_skipped: &[usize],
    receipts: &[Option<&str>], meta: &LinksMeta,
    only: &(dyn Fn(usize) -> bool + Sync)) -> Vec<(Vec<usize>, Result<String, String>, String)> {
    let results: Mutex<Vec<(Vec<usize>, Result<String, String>, String)>> = Mutex::new(Vec::new());
    let done = |i: usize, r: Result<String, String>, out_dir: String| {
        meta.lock().unwrap().insert(i, match &r {
            Ok(stdout) => Ok(script_metadata(stdout)),
            Err(_) => Err("its build script failed".to_string()),
        });
        results.lock().unwrap().push((g.build_scripts[i].krates(), r, out_dir));
    };
    let unbuilt = |bs: &BuildScript| {
        let build_dir = root.take(&format!("{}-unbuilt", g.crates[bs.krate].name));
        let out_dir = format!("{}/out", build_dir);
        let _ = std::fs::create_dir_all(&out_dir);
        out_dir
    };
    let mut to_run: Vec<usize> = Vec::new();
    for (i, bs) in g.build_scripts.iter().enumerate() {
        if native_krates.contains(&bs.krate) || !only(bs.krate) {
            continue;
        }
        if check_skipped.contains(&bs.krate) {
            let out_dir = unbuilt(bs);
            results.lock().unwrap().push((bs.krates(), Ok(String::new()), out_dir));
            meta.lock().unwrap().insert(i, Err(C_NOT_RUN.to_string()));
            continue;
        }
        let (dir, pname, _) = &g.pkg[bs.krate];
        let name = &g.crates[bs.krate].name;
        let role = if noop::script(bs, &g.raw) {
            if sess.runtime.profile {
                eprintln!("relax build-script: {} skipped (no effect under RELAX_BUILD_SCRIPT)", pname);
            }
            ScriptRole::Answered(Vec::new())
        } else { script_role(pname, bs) };
        match role {
            ScriptRole::Answered(lines) => {
                // cargo sets OUT_DIR for every package with a build script
                let out_dir = format!("{}/out", root.take(&format!("{}-answered", name)));
                let r = std::fs::create_dir_all(&out_dir).map(|_| lines.join("\n")).map_err(|e| format!("{}: {}", out_dir, e));
                done(i, r, out_dir);
                continue;
            }
            ScriptRole::ToRemove => {
                let mut t = TO_REMOVE.lock().unwrap();
                if !t.iter().any(|x| x.0 == *pname) {
                    crate::zone_alloc::kept_with(|| t.push((pname.clone(), bs.src_path.clone())));
                }
            }
            ScriptRole::SelfStamp => {
                let text = self_stamp_text(dir, pname, &bs.features);
                // (a shared workspace's checkouts have one run dir, vfs.rs SHARE: a stamp per
                // content, where each checkout wrote its own over the other's)
                let build_dir = match &text {
                    Ok(text) if crate::vfs::in_folder(dir) => root.take(&format!("{}-answered-{:016x}", name, crate::sip::hash_bytes(text.as_bytes()) as u64)),
                    _ => root.take(&format!("{}-answered", name)),
                };
                let out_dir = format!("{}/out", build_dir);
                let r = text.and_then(|text| {
                    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {}", out_dir, e))?;
                    let out = format!("{}/src_stamp.rs", out_dir);
                    if std::fs::read_to_string(&out).ok().as_deref() != Some(text.as_str()) {
                        std::fs::write(&out, text).map_err(|e| format!("{}: {}", out, e))?;
                    }
                    Ok(String::new())
                });
                done(i, r, out_dir);
                continue;
            }
            ScriptRole::Native(a) => {
                let out_dir = format!("{}/out", root.take(&format!("{}-answered", name)));
                let env: Vec<(String, String)> = crate::cfg::CfgSet::host(sess, &[]).build_script_env();
                let r = std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {}", out_dir, e))
                    .and_then(|_| (a.answer)(&ScriptEnv { env: &env, target: target_triple(sess), out_dir: std::path::Path::new(&out_dir) }));
                done(i, r.map(|l| l.join("\n")), out_dir);
                continue;
            }
            ScriptRole::Artefact => {}
        }
        to_run.push(i);
    }
    // the level of each script to run: one past the deepest `links` script of this call it takes
    // metadata from (cargo refuses a dependency cycle; the rounds end one anyway)
    let mut level = vec![0usize; g.build_scripts.len()];
    let mut in_call = vec![false; g.build_scripts.len()];
    for &i in &to_run {
        in_call[i] = true;
    }
    for _ in 0..to_run.len() {
        let mut changed = false;
        for &i in &to_run {
            let l = g.build_scripts[i].links_deps.iter().filter(|&&d| in_call[d]).map(|&d| level[d] + 1).max().unwrap_or(0);
            if l != level[i] {
                level[i] = l;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let levels = to_run.iter().map(|&i| level[i]).max().map_or(0, |l| l + 1);
    for l in 0..levels {
        crate::pool::scope(|s| {
            for &i in to_run.iter().filter(|&&i| level[i] == l) {
                let bs = &g.build_scripts[i];
                let mut cenv = g.env[bs.krate].clone();
                match links_env(g, bs, meta) {
                    Ok(dep_env) => cenv.extend(dep_env),
                    // a check without `--allow-c`: a script that needed a skipped C package (aws-lc-rs
                    // reads aws-lc-sys's DEP_ metadata) leaves its crate unconfigured, said once, not fatal
                    Err((e, true)) => {
                        eprintln!("relax check: {} (its crate is checked without the script's cfgs)", e.lines().next().unwrap_or(""));
                        results.lock().unwrap().push((bs.krates(), Ok(String::new()), unbuilt(bs)));
                        meta.lock().unwrap().insert(i, Err(C_DEP_NOT_RUN.to_string()));
                        continue;
                    }
                    Err((e, false)) => {
                        let out_dir = unbuilt(bs);
                        done(i, Err(e), out_dir);
                        continue;
                    }
                }
                let (dir, pname, ver) = g.pkg[bs.krate].clone();
                let name = g.crates[bs.krate].name.clone();
                let raw = &g.raw;
                let done = &done;
                let receipt = receipts[i];
                s.spawn(move || {
                    let r = run_one(sess, bs, raw, &cenv, &dir, &pname, &ver, &name, target_dir, root, receipt);
                    done(i, r.0, r.1);
                });
            }
        });
    }
    results.into_inner().unwrap()
}

/// The `DEP_<LINKS>_<KEY>` variables cargo gives a script (custom_build.rs build_work): the
/// metadata the scripts of its package's `links` dependencies printed. Err: such a script handed
/// nothing on (not run, or failed) and this script reads `DEP_` variables, so it is not run: it
/// would fail on the missing value, or answer other than under cargo; with true when it is missing
/// for a check's C skip (C_NOT_RUN, C_DEP_NOT_RUN). A dependency built by cargo (`--allow-c`) is
/// not in the table: a script reading its metadata is cargo's too.
fn links_env(g: &Graph, bs: &BuildScript, meta: &LinksMeta) -> Result<Vec<(String, String)>, (String, bool)> {
    let mut env = Vec::new();
    for &d in &bs.links_deps {
        let (ddir, dname, _) = &g.pkg[g.build_scripts[d].krate];
        let Some(links) = crate::resolve::package_links(ddir) else { continue };
        match meta.lock().unwrap().get(&d) {
            Some(Ok(pairs)) => {
                for (k, v) in pairs {
                    env.push((format!("DEP_{}_{}", envify(&links), envify(k)), v.clone()));
                }
            }
            Some(Err(why)) if crate::native::reads_dep_metadata(bs) => {
                return Err((format!("{}: the build script of `{}` was not run: it reads the `DEP_` metadata of `{}` (links = \"{}\"), and {}",
                    bs.src_path, g.pkg[bs.krate].1, dname, links, why), why == C_NOT_RUN || why == C_DEP_NOT_RUN));
            }
            _ => {}
        }
    }
    Ok(env)
}

/// cargo's `envify`: a `links` name or metadata key as part of a variable name.
fn envify(s: &str) -> String {
    s.chars().flat_map(|c| c.to_uppercase()).map(|c| if c == '-' { '_' } else { c }).collect()
}

/// The keys of the old `cargo:KEY=VALUE` syntax that are instructions to cargo; every other key
/// is metadata (cargo custom_build.rs BuildOutput::parse).
const SCRIPT_INSTRUCTIONS: [&str; 17] = ["rustc-flags", "rustc-link-lib", "rustc-link-search", "rustc-link-arg-cdylib", "rustc-cdylib-link-arg",
    "rustc-link-arg-bins", "rustc-link-arg-bin", "rustc-link-arg-tests", "rustc-link-arg-benches", "rustc-link-arg-examples", "rustc-link-arg",
    "rustc-cfg", "rustc-check-cfg", "rustc-env", "warning", "rerun-if-changed", "rerun-if-env-changed"];

/// The metadata a script's output hands to the scripts of the packages that depend on its
/// package (when that package has `links`): `cargo::metadata=KEY=VALUE`, and `cargo:KEY=VALUE`
/// with a key that is no instruction.
fn script_metadata(stdout: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let pair = match line.strip_prefix("cargo::") {
            Some(rest) => rest.strip_prefix("metadata=").and_then(|kv| kv.split_once('=')),
            None => line.strip_prefix("cargo:").and_then(|kv| kv.split_once('=')).filter(|(k, _)| !SCRIPT_INSTRUCTIONS.contains(k)),
        };
        if let Some((k, v)) = pair {
            out.push((k.to_string(), v.trim_end().to_string()));
        }
    }
    out
}

/// The scripts' `cargo:` lines into the graph: cfgs, link libraries, env (OUT_DIR included).
fn apply_script_results(g: &mut Graph, results: Vec<(Vec<usize>, Result<String, String>, String)>, check_only: bool) -> Result<(), String> {
    for (krates, r, out_dir) in results {
        // a script that failed fails a check too, as under `cargo check` (the scripts a check did
        // not run for C answered empty: relax_scripts)
        let stdout = r?;
        for k in krates { apply_script_output(g, k, &stdout, out_dir.clone(), check_only)?; }
    }
    Ok(())
}

/// One crate's share of a script's output (`stdout`, its OUT_DIR).
fn apply_script_output(g: &mut Graph, k: usize, stdout: &str, out_dir: String, check_only: bool) -> Result<(), String> {
    script_output_into(&mut g.cfgs, &mut g.env, &mut g.link_libs, &g.pkg, k, stdout, out_dir, check_only)
}

/// apply_script_output into these tables (the graph's, or copies of them).
fn script_output_into(cfgs: &mut [Vec<String>], env: &mut [Vec<(String, String)>], link_libs: &mut Vec<(String, bool)>, pkg: &[(String, String, String)],
    k: usize, stdout: &str, out_dir: String, check_only: bool) -> Result<(), String> {
    env[k].push(("OUT_DIR".to_string(), out_dir));
    for line in stdout.lines() {
        let rest = match line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:")) {
            Some(r) => r,
            None => continue,
        };
        if let Some(c) = rest.strip_prefix("rustc-cfg=") {
            cfgs[k].push(c.to_string());
        } else if let Some(l) = rest.strip_prefix("rustc-link-lib=") {
            // [kind=]name[:rename]
            let (kind, name) = match l.find('=') {
                Some(i) => (&l[..i], &l[i + 1..]),
                None => ("", l),
            };
            let name = name.split(':').next().unwrap_or(name).to_string();
            // a static library a script built (C, assembly): only `--allow-c` links one
            // (a check links nothing: the library is not needed)
            if kind == "static" && check_only {
                continue;
            }
            if kind == "static" {
                return Err(format!("`{}`'s build script links the static library `{}` (native code): it {}", pkg[k].1, name, relax_session::options::C_GATE));
            }
            let fw = kind == "framework";
            if !link_libs.contains(&(name.clone(), fw)) {
                link_libs.push((name, fw));
            }
        } else if let Some(e) = rest.strip_prefix("rustc-env=") {
            if let Some(i) = e.find('=') {
                env[k].push((e[..i].to_string(), e[i + 1..].to_string()));
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
    let mut h = fnv(crate::dirs::anchored(&bs.src_path).as_bytes(), 0xcbf29ce484222325);
    // a shared workspace's checkouts (vfs.rs SHARE) have one run dir and one path for the script:
    // its text names its units' folder and its output's, or a checkout with another script would
    // write its rewritten source and its output over this one's
    if crate::vfs::in_folder(&bs.src_path) {
        if let Ok(src) = crate::vfs::read(&bs.src_path) {
            h = fnv(&src, h);
        }
    }
    // the worktree: payloads still hold its absolute paths (Target B removes this per payload)
    h = fnv(&crate::dirs::anchors_key().to_le_bytes(), h);
    let mut units: Vec<String> = Vec::new();
    let deps_json = |deps: &[(usize, String)]| -> String {
        deps.iter().filter_map(|(d, n)| idx(*d).map(|k| format!("{{\"index\":{},\"extern_crate_name\":{}}}", k, q(n)))).collect::<Vec<_>>().join(",")
    };
    for &i in &seen {
        let u = &raw[i];
        h = fnv(crate::dirs::anchored(&u.src_path).as_bytes(), h);
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
            "{{\"pkg_id\":{},\"target\":{{\"kind\":[\"{}\"],\"name\":{},\"src_path\":{},\"edition\":\"{}\"}},\"profile\":{{\"opt_level\":\"3\"{}}},\"mode\":\"{}\",\"features\":[{}],\"dependencies\":[{}]}}",
            q(&format!("path+file://{}#{}@{}", u.pkg.0, u.pkg.1, u.pkg.2)),
            kind,
            q(&name),
            q(&u.src_path),
            edition,
            checks_json(u.host_checks),
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
    let sfeats: Vec<String> = bs.features.iter().map(|f| q(f)).collect();
    // the script's own checks: its compile unit's
    let script_checks = checks_json(raw.iter().find(|u| u.src_path == bs.src_path && !u.run_script).and_then(|u| u.host_checks));
    let script_unit = |src_path: &str| format!(
        "{{\"pkg_id\":{},\"target\":{{\"kind\":[\"bin\"],\"name\":\"build-script-build\",\"src_path\":{},\"edition\":\"{}\"}},\"profile\":{{\"opt_level\":\"3\"{}}},\"mode\":\"build\",\"features\":[{}],\"dependencies\":[{}]}}",
        q(&format!("path+file://{}#build-script-build@0.0.0", pkg_dir)),
        q(src_path),
        bs.edition,
        script_checks,
        sfeats.join(","),
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

/// A unit's checks in a build script's unit graph (its `profile`), which the script's run builds
/// it with (resolve.rs GraphUnit `checks`): part of the rerun key, as the graph's text is.
fn checks_json(c: Option<crate::resolve::ProfileChecks>) -> String {
    c.map_or_else(String::new, |c| format!(",\"debug_assertions\":{},\"overflow_checks\":{}", c.debug_assertions, c.overflow_checks))
}

static SELF_EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The program a build script runs as: Relax running itself again. It is the loader
/// (dirs::loader_exe) unless the host names another one ([`set_self_exe`]): in an Android app this
/// process is `app_process64`, and Relax's own executable is the PIE the APK carries next to the
/// host library.
pub fn set_self_exe(exe: std::path::PathBuf) {
    let _ = SELF_EXE.set(crate::zone_alloc::kept_clone(&exe));
}

fn self_exe() -> std::io::Result<std::path::PathBuf> {
    match SELF_EXE.get() {
        Some(e) => Ok(e.clone()),
        None => crate::dirs::loader_exe(),
    }
}

/// (stdout of the script, OUT_DIR). The script runs as a Relax program: this executable started
/// again on the script's units (script_units) -- no rustc, no script binary; Relax re-running
/// itself is the only process it starts. A script of a git run reads its package through the
/// VFS (RELAX_VFS / RELAX_WORKSPACE for the child), and writes only OUT_DIR (on disk).
#[allow(clippy::too_many_arguments)]
fn run_one(sess: &std::sync::Arc<crate::Session>, bs: &BuildScript, raw: &[RawUnit], cenv: &[(String, String)], dir: &str, pname: &str, ver: &str, name: &str, target_dir: &str, root: &BuildRoot, receipt: Option<&str>) -> (Result<String, String>, String) {
    if let Err(e) = crate::vfs::read(&bs.src_path) {
        return (Err(format!("{}: {}", bs.src_path, e)), String::new());
    }
    let rewrite = noop::rewrite(bs, raw);
    if sess.runtime.profile {
        if let Some(rewrite) = &rewrite {
            for (dep, function) in &rewrite.calls {
                eprintln!("relax build-script: {} runs without {}::{}() (inert)", pname, dep, function);
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
    // a package of a shared workspace (vfs.rs SHARE): its script's output is kept per content of
    // its inputs (`shared_build_dir`), and `base_dir` holds the claim and what the script declared
    let shared: Option<SharedContent> = crate::vfs::in_folder(dir).then(Default::default);
    let base_dir = root.take(&format!("{}-{:016x}", name, fh));
    let mut out_dir = format!("{}/out", base_dir);
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
    let _ = std::fs::create_dir_all(if shared.is_some() { &base_dir } else { &out_dir });
    let exe = match self_exe() {
        Ok(e) => e,
        Err(e) => return (Err(format!("this executable: {e}")), out_dir),
    };
    // nesting: a build-dependency's own script runs inside a script's run, and its
    // build-dependencies' scripts inside that, as deep as the graph goes (cargo refuses a cyclic
    // one). RELAX_SCRIPT_DEPTH marks a script's run; RELAX_SCRIPT_CHAIN names the
    // packages whose scripts are running, each as name@version (two versions of one package may
    // both sit in a chain). A package already in the chain is a script that runs itself again.
    let depth: u32 = std::env::var("RELAX_SCRIPT_DEPTH").ok().and_then(|d| d.parse().ok()).unwrap_or(0) + 1;
    let chain = std::env::var("RELAX_SCRIPT_CHAIN").unwrap_or_default();
    let me = format!("{pname}@{ver}");
    if chain.split('>').any(|p| p == me) {
        return (Err(format!("{}: the build script of `{}` runs again inside its own run (build-dependency cycle: {} -> {}); refused", bs.src_path, me, chain.replace('>', " -> "), me)), out_dir);
    }
    let chain = if chain.is_empty() { me } else { format!("{chain}>{me}") };
    let mut cmd = std::process::Command::new(&exe);
    // a run of its own: nothing this run says of itself crosses; its folder's mount is set below,
    // and this invocation's `--allow-c` is the script's too
    crate::progenv::scrub_run_facts(&mut cmd);
    if sess.options.allow_c {
        cmd.env("RELAX_ALLOW_C", "1");
    }
    cmd.args(["run", "--quiet", "--silent", "--run-dir", target_dir, "--units", &units_path, "build-script-build"]);
    cmd.env("OUT_DIR", &out_dir)
        // Build scripts may test this to skip work only rustc needs.
        .env("RELAX_BUILD_SCRIPT", "1")
        .env("TARGET", target_triple(sess))
        .env("HOST", host_triple(sess))
        .envs(bs.profile.env())
        .env("NUM_JOBS", "4")
        .envs(pkg_env(dir, pname, ver))
        // RUSTDOC too (the `built` crate requires it); nothing documents here
        .env("RUSTDOC", "rustdoc")
        .env("RELAX_AS_RUSTC", "build-script")
        .env("RELAX_SCRIPT_DEPTH", depth.to_string())
        .env("RELAX_SCRIPT_CHAIN", &chain)
        // the script is compiled and run for the machine (cargo: build scripts are host
        // artifacts); the target it builds for reaches it only as TARGET / CARGO_CFG_TARGET_*
        .env_remove("RELAX_CHECK_TARGET");
    for (k, v) in crate::cfg::CfgSet::host(sess, &[]).build_script_env() {
        cmd.env(k, v);
    }
    match crate::resolve::package_links(dir) {
        Some(l) => cmd.env("CARGO_MANIFEST_LINKS", l),
        None => cmd.env_remove("CARGO_MANIFEST_LINKS"),
    };
    // the native build's outputs for the native packages this script's build-dependencies reach
    // (run_build_scripts): named by its content, so the rerun key below changes exactly when they do
    match receipt {
        Some(r) => cmd.env(NATIVE_RECEIPT, r),
        None => cmd.env_remove(NATIVE_RECEIPT),
    };
    for (k, v) in cenv {
        cmd.env(k, v);
    }
    // Both tools are the loader: RUSTC probes use its target model, CARGO metadata its resolver
    // and VFS (a virtual or shared workspace's manifests are not on the disk a cargo reads). Set
    // after the crate's environment, whose CARGO is the build's cargo (pkg_env). The runner
    // consumes `build-script` on entry, then marks tool calls.
    cmd.env("RUSTC", &exe).env("CARGO", &exe);
    cmd.env("CARGO_ENCODED_RUSTFLAGS", "");
    for f in &bs.features {
        cmd.env(format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_")), "1");
    }
    // the package folder is the script's current folder (cargo runs scripts there); a virtual one
    // is handed over as the child's virtual workspace with the mount table
    if crate::vfs::is_virtual(dir) {
        cmd.env("RELAX_WORKSPACE", dir).env("RELAX_VFS", crate::vfs::mounts_text());
        if let Some(e) = std::env::var_os("RELAX_EDITS") {
            cmd.env("RELAX_EDITS", e);
        }
        if shared.is_some() {
            // a shared workspace's package is a folder of the checkout too: the script's run is
            // told its mount is a checkout's, and starts in that folder, where the script's own
            // children run (they get no mount, progenv.rs; cargo runs a script in its package)
            if let Some(n) = std::env::var_os("RELAX_SHARE") {
                cmd.env("RELAX_SHARE", n);
            }
            cmd.current_dir(&*crate::vfs::on_disk(dir));
        }
    } else {
        // a package on disk (a registry crate of a store run): its folder is the current folder,
        // and no virtual workspace crosses (Relax's std resolves relative paths against
        // RELAX_WORKSPACE first: rustix's build.rs probed `src/backend/linux_raw/arch/x86_64.rs`
        // there, found nothing and chose its libc backend)
        cmd.current_dir(dir);
    }
    // cargo's `[env]` configuration, after every variable set above (cargo apply_env_config)
    for (k, v) in crate::crates::config_env() {
        if !cmd.get_envs().any(|(n, _)| n == k.as_str()) {
            cmd.env(k, v);
        }
    }
    // cargo's rerun rules: the previous output stands while the script's units, the environment
    // it was given and its `rerun-if-changed` / `rerun-if-env-changed` inputs are unchanged (a
    // script that names none: no file of its package changed)
    let mut env_key = format!("{:016x}", h);
    for (k, v) in cmd.get_envs() {
        // no receipt is no part of the key (it held none before there were receipts); a shared
        // workspace's OUT_DIR is named by this key (`shared_build_dir`)
        if SCRIPT_RUN_SHAPE_VARS.iter().any(|s| k == *s) || (k == NATIVE_RECEIPT && v.is_none()) || (shared.is_some() && k == "OUT_DIR") {
            continue;
        }
        env_key.push_str(&format!("\n{}={}", k.to_string_lossy(), v.map(|v| v.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    // Ok: the output in `build_dir`, which stands; Err: why the script runs again
    let previous = |build_dir: &str| match (std::fs::read_to_string(format!("{}/output", build_dir)), std::fs::read_to_string(format!("{}/relax-rerun", build_dir))) {
        (Ok(old), Ok(key)) => {
            let own = std::path::Path::new(&format!("{}/{}", build_dir, CHECKOUT_MARK)).is_file();
            let keyed = if own { format!("{CHECKOUT_LINE}\n{old}") } else { old.clone() };
            let now = rerun_key(&keyed, dir, &env_key, shared.as_ref()).unwrap_or_default();
            if now == key { Ok(old) } else { Err(rerun_reason(&key, &now, &std::fs::read_to_string(format!("{}/relax-rerun-env", build_dir)).unwrap_or_default(), &env_key)) }
        }
        _ => Err("no previous output".to_string()),
    };
    let declared_path = format!("{}/relax-declared", base_dir);
    // where this run's output is (its build dir, named in the pass's root before it is read), and
    // whether one stands there
    let place = || match &shared {
        None => (base_dir.clone(), previous(&base_dir)),
        Some(content) => {
            let (at, standing) = shared_build_dir(&base_dir, &std::fs::read_to_string(&declared_path).unwrap_or_default(),
                &|declared| rerun_key(declared, dir, &env_key, Some(content)).unwrap_or_default(), &previous);
            let at = root.take(at.rsplit('/').next().unwrap_or_default());
            // (a prune may have taken it before the root named it)
            let standing = standing.and_then(|old| if std::path::Path::new(&format!("{}/output", at)).is_file() { Ok(old) } else { Err("no previous output".to_string()) });
            (at, standing)
        }
    };
    let (mut build_dir, standing) = place();
    let mut why = match standing {
        Ok(old) => {
            if sess.runtime.profile { eprintln!("relax build-script: {} cached", pname); }
            return (Ok(old), format!("{}/out", build_dir));
        }
        Err(why) => why,
    };
    // One run at a time makes a script's output (its OUT_DIR is one folder for every run of this
    // run dir): the run holds `relax-claim` beside it locked while its script runs. A run
    // that finds it held waits and takes the output the other made, where two runs started
    // together each compiled and ran every script. The OS drops the lock with a run that dies.
    // The script slot (this process's) comes first: a run holding the claim and waiting for a slot
    // could wait on a slot whose script's nested run waits for this claim (a package that is
    // both a dependency here and a build-dependency of another script).
    // The run that holds it names itself in it (its pid and its devhub lease, `claim_holder`):
    // a waiter says whom it waits on, and a hang names its holder. A relax-runner test sat 26 min
    // on its build-script claim, held by a hung run in another lease (11-13).
    let _slot = script_slot();
    let claim_path = format!("{}/relax-claim", base_dir);
    let claim = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&claim_path).ok();
    if let Some(claim) = claim.as_ref().filter(|claim| claim.try_lock().is_err()) {
        // (a silent run's stderr is its program's: the line goes to the session log alone)
        crate::progress::say(&format!("relax build-script: {} waits for its claim {}: {}", pname, claim_path, claim_holder(&claim_path)));
        let _ = claim.lock();
        // (the wait can be long: the checkout's files are read again)
        if let Some(content) = &shared {
            content.borrow_mut().clear();
        }
        let (at, standing) = place();
        build_dir = at;
        match standing {
            Ok(old) => {
                if sess.runtime.profile { eprintln!("relax build-script: {} made by another run", pname); }
                return (Ok(old), format!("{}/out", build_dir));
            }
            Err(now) => why = now,
        }
    }
    if let Some(claim) = claim.as_ref() { name_claim(claim); }
    if shared.is_some() {
        out_dir = format!("{}/out", build_dir);
        let _ = std::fs::create_dir_all(&out_dir);
        cmd.env("OUT_DIR", &out_dir);
    }
    let output_path = format!("{}/output", build_dir);
    let key_path = format!("{}/relax-rerun", build_dir);
    let env_path = format!("{}/relax-rerun-env", build_dir);
    // why this script runs again, kept beside its output (relax-rerun-why: the last 20) and shown with RELAX_PROFILE
    if sess.runtime.profile { eprintln!("relax build-script: {} runs: {}", pname, why); }
    note_rerun(&build_dir, &why);
    let started = std::time::Instant::now();
    let output = cmd.output();
    if sess.runtime.profile {
        eprintln!("relax build-script: {} executed {:.3} ms", pname, started.elapsed().as_secs_f64() * 1000.0);
    }
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let stdout = if shared.is_some() { shared_output(&stdout) } else { stdout };
            // an output that names its checkout stands for that checkout alone (`names_checkout`)
            let own = shared.is_some() && names_checkout(&stdout, &out_dir);
            let mark = format!("{}/{}", build_dir, CHECKOUT_MARK);
            if own {
                let _ = std::fs::write(&mark, crate::vfs::folder_disks().join("\n"));
            } else {
                let _ = std::fs::remove_file(&mark);
            }
            let keyed = if own { format!("{CHECKOUT_LINE}\n{stdout}") } else { stdout.clone() };
            if let Some(key) = rerun_key(&keyed, dir, &env_key, shared.as_ref()) {
                let _ = std::fs::write(&output_path, &stdout);
                let _ = std::fs::write(&key_path, key);
                let _ = std::fs::write(&env_path, &env_key);
            } else {
                let _ = std::fs::remove_file(&key_path);
            }
            // what the script declared names the place of its next output (`shared_build_dir`)
            if shared.is_some() {
                let declared = declared_inputs(&keyed);
                if std::fs::read_to_string(&declared_path).ok().as_deref() != Some(declared.as_str()) {
                    let _ = std::fs::write(&declared_path, &declared);
                }
            }
            (Ok(stdout), out_dir)
        }
        Ok(o) => {
            let cause = script_error_cause(&String::from_utf8_lossy(&o.stderr));
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

/// The cause a failing build script's stderr gives: the panic it printed (Relax's report lines
/// carry the message as "message"), else the first errors its compile reported, else the first
/// line that says it panicked or failed, else its last lines. A script that does not compile ends
/// with `error: could not compile ..: not started`, which names no cause: its diagnostics come
/// before it, in Relax's `path:line:col: E0308: ..` or rustc's `error[E0308]: ..` shape.
fn script_error_cause(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let is_error = |l: &str| match located_message(l) {
        Some(m) => !["warning", "note", "help"].iter().any(|k| m.starts_with(k)),
        None => l.starts_with("error[") || (l.starts_with("error:") && !l.starts_with("error: could not compile")),
    };
    let errors: Vec<&str> = lines.iter().copied().filter(|l| is_error(l)).take(3).collect();
    lines
        .iter()
        .find_map(|l| l.split("\"message\":\"").nth(1).and_then(|r| r.split("\",\"").next()))
        .map(|m| m.to_string())
        .or_else(|| (!errors.is_empty()).then(|| errors.join(" | ")))
        .or_else(|| lines.iter().find(|l| l.contains("panicked") || l.starts_with("error")).map(|l| l.to_string()))
        .unwrap_or_else(|| lines.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | "))
}

/// The message of a `path:line:col: message` line (Relax's diagnostics), None for another line.
fn located_message(l: &str) -> Option<&str> {
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mut from = 0;
    while let Some(i) = l[from..].find(": ") {
        let at = from + i;
        let mut head = l[..at].rsplitn(3, ':');
        if let (Some(col), Some(line), Some(path)) = (head.next(), head.next(), head.next()) {
            if number(col) && number(line) && !path.is_empty() {
                return Some(&l[at + 2..]);
            }
        }
        from = at + 2;
    }
    None
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
/// What a script's run is given that says how the run is shaped, not what the script reads: left
/// out of its rerun key. The mount's table and edits folder name where the sources are read, not
/// what they are; the nesting depth and chain say which script's run this one sits in, so a
/// package reached at the top and inside another script's run (a dependency here and a
/// build-dependency there) keeps one output (relax issue 69). No cargo script can read them.
/// The variable that hands a build script's child run its parent's native receipt.
pub const NATIVE_RECEIPT: &str = "RELAX_NATIVE_RECEIPT";
/// The variable that hands the package binaries an integration test starts (CARGO_BIN_EXE_<name>)
/// its run's native receipt (`Inherit::Binary`): set in the test's environment. Empty: none, the
/// binary builds its own natives. A binary whose test cleared it from its environment (or cleared
/// the whole environment) takes the receipt the test's run named beside its wrapper
/// (`bin_native`).
pub const BIN_NATIVE_RECEIPT: &str = "RELAX_BIN_NATIVE_RECEIPT";

/// Beside the CARGO_BIN_EXE_<bin> wrappers of the run dir `run_dir` (`--run-dir`): the file that
/// names the native receipt of the test run that last started `bin` (relax issue 157: a test's
/// cleared environment held none).
pub fn bin_native(run_dir: &std::path::Path, bin: &str) -> std::path::PathBuf {
    run_dir.join("relax-bin").join(format!("{bin}.native"))
}

const SCRIPT_RUN_SHAPE_VARS: [&str; 4] = ["RELAX_VFS", "RELAX_EDITS", "RELAX_SCRIPT_DEPTH", "RELAX_SCRIPT_CHAIN"];

fn note_rerun(build_dir: &str, why: &str) {
    let path = format!("{}/relax-rerun-why", build_dir);
    let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().map(str::to_string).collect();
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    lines.push(format!("{} pid {} {}", t, std::process::id(), why));
    let skip = lines.len().saturating_sub(20);
    let _ = std::fs::write(&path, lines[skip..].join("\n") + "\n");
}


/// The rerun key of a script output: the environment it ran with, then per
/// `rerun-if-changed` path its newest modification time (directories: recursively) and per
/// `rerun-if-env-changed` variable its value; an output that names none: the package's newest file.
/// A shared workspace's (`shared`, vfs.rs SHARE): a path of its checkout keyed by what it holds
/// (`shared_content`), not its time (its output names the mount's paths, `shared_output`).
fn rerun_key(output: &str, pkg_dir: &str, env_key: &str, shared: Option<&SharedContent>) -> Option<String> {
    let mut key = format!("{:016x}\n", fnv(env_key.as_bytes(), 0xcbf29ce484222325));
    let mut any = false;
    for line in output.lines() {
        if line == CHECKOUT_LINE {
            // (an output kept per checkout: this run's checkout, on disk)
            key.push_str(&format!("k {}\n", crate::vfs::folder_disks().join(" ")));
            continue;
        }
        let rest = match line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:")) {
            Some(r) => r,
            None => continue,
        };
        if let Some(p) = rest.strip_prefix("rerun-if-changed=") {
            any = true;
            let path = if std::path::Path::new(p).is_absolute() { p.to_string() } else { format!("{}/{}", pkg_dir, p) };
            match (shared_content(shared, &path, false), crate::vfs::identity(&path)) {
                (Some(c), _) => key.push_str(&format!("f c{:032x} {}\n", c, path)),
                (None, Some(id)) => key.push_str(&format!("f {} {}\n", id, path)),
                (None, None) => key.push_str(&format!("f {} {}\n", newest_mtime(std::path::Path::new(&path)), path)),
            }
        } else if let Some(v) = rest.strip_prefix("rerun-if-env-changed=") {
            any = true;
            key.push_str(&format!("e {}={:?}\n", v, std::env::var(v).ok()));
        }
    }
    if !any {
        // cargo: a script that names no inputs reruns when any file of its package changes
        match (shared_content(shared, pkg_dir, true), crate::vfs::identity(pkg_dir)) {
            (Some(c), _) => key.push_str(&format!("p c{:032x} {}\n", c, pkg_dir)),
            (None, Some(id)) => key.push_str(&format!("p {} {}\n", id, pkg_dir)),
            (None, None) => key.push_str(&format!("p {} {}\n", newest_mtime_pkg(std::path::Path::new(pkg_dir)), pkg_dir)),
        }
    }
    Some(key)
}

/// What the files of a shared workspace's checkout hold, per (virtual path, package folder), for one
/// script's rerun keys (`content`).
type SharedContent = std::cell::RefCell<std::collections::HashMap<(String, bool), u128>>;

/// What a file holds, or a folder with everything below it (each entry's name and content; 0:
/// missing). `pkg`: a package's own folder, without its target directories and hidden entries
/// (`newest_mtime_pkg`). Links are followed, as cargo's walk of a rerun-if-changed folder follows
/// them; a folder that is its own ancestor (a link back up) ends there, so a loop ends.
fn content(p: &std::path::Path, pkg: bool) -> u128 {
    // (unix: device and inode; elsewhere the canonical path)
    fn identity(p: &std::path::Path, m: &std::fs::Metadata) -> (u64, u64, String) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let _ = p;
            (m.dev(), m.ino(), String::new())
        }
        #[cfg(not(unix))]
        {
            let _ = m;
            (0, 0, std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default())
        }
    }
    fn walk(p: &std::path::Path, pkg: bool, ancestors: &mut Vec<(u64, u64, String)>) -> u128 {
        let Ok(m) = std::fs::metadata(p) else { return 0 };
        let mut h = crate::sip::Sip128::new();
        if !m.is_dir() {
            let Ok(bytes) = std::fs::read(p) else { return 0 };
            h.str("file");
            h.u64(bytes.len() as u64);
            h.write(&bytes);
            return h.finish128();
        }
        let id = identity(p, &m);
        h.str("folder");
        if ancestors.contains(&id) {
            h.str("ancestor");
            return h.finish128();
        }
        ancestors.push(id);
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        names.sort();
        for n in names {
            let name = n.to_string_lossy();
            if pkg && (name.starts_with("target") || name.starts_with('.')) {
                continue;
            }
            h.str(&name);
            h.u128(walk(&p.join(&n), false, ancestors));
        }
        ancestors.pop();
        h.finish128()
    }
    walk(p, pkg, &mut Vec::new())
}

/// `path`'s part of a shared workspace's rerun key (vfs.rs SHARE): what the checkout holds there,
/// not when it was written. The checkouts of one repository have one run dir: a file's time says
/// which of them was written last. None: no shared workspace, or a path outside its folder mount (a
/// registry package's, the same folder for every checkout).
fn shared_content(shared: Option<&SharedContent>, path: &str, pkg: bool) -> Option<u128> {
    let shared = shared.filter(|_| crate::vfs::in_folder(path))?;
    let at = (crate::vfs::normalize_path(path), pkg);
    if let Some(c) = shared.borrow().get(&at) {
        return Some(*c);
    }
    let c = content(std::path::Path::new(&*crate::vfs::on_disk(&at.0)), pkg);
    shared.borrow_mut().insert(at, c);
    Some(c)
}

/// The first line of a rerun key's text (and of `declared`) for a script output kept per checkout
/// (`names_checkout`): its key holds the checkout's folder on disk. `CHECKOUT_MARK`, beside such
/// an output, says so.
const CHECKOUT_LINE: &str = "relax:per-checkout";
const CHECKOUT_MARK: &str = "relax-checkout";

/// Whether a shared workspace's script output (vfs.rs SHARE) is its checkout's own: its stdout
/// (rerun-if-changed paths already the mount's) or a file it left in its OUT_DIR names the
/// checkout's folder on disk (a `rustc-env` of the source root, generated code holding its paths),
/// or it declares an input in the repository's git metadata (`rerun-if-changed=.git/HEAD`: the
/// same text in every clone, while the commit and the files are each checkout's). Cargo never
/// shares an OUT_DIR between checkouts; such an output stands for its own checkout alone, else
/// another checkout's run took the first one's paths and commit (relax review B-M8).
fn names_checkout(stdout: &str, out_dir: &str) -> bool {
    // each folder as mounted and as the script's current folder gives it (links resolved: a temp
    // folder under /var is /private/var on macOS)
    let mut disks = crate::vfs::folder_disks();
    for d in disks.clone() {
        if let Ok(c) = std::fs::canonicalize(&d) {
            disks.push(crate::vfs::normalize_path(&c.to_string_lossy()));
        }
    }
    disks.sort();
    disks.dedup();
    if disks.iter().any(|d| stdout.contains(d.as_str())) {
        return true;
    }
    let git = stdout.lines().filter_map(|l| l.strip_prefix("cargo::").or_else(|| l.strip_prefix("cargo:"))?.strip_prefix("rerun-if-changed="))
        .any(|p| p.split(['/', '\\']).any(|c| c == ".git"));
    git || (!disks.is_empty() && folder_mentions(std::path::Path::new(out_dir), &disks))
}

/// Whether a file under `dir` (links not followed) holds one of `needles`.
fn folder_mentions(dir: &std::path::Path, needles: &[String]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if folder_mentions(&e.path(), needles) {
                return true;
            }
        } else if t.is_file() {
            let Ok(bytes) = std::fs::read(e.path()) else { continue };
            if needles.iter().any(|n| holds(&bytes, n.as_bytes())) {
                return true;
            }
        }
    }
    false
}

/// Whether `hay` holds `needle` (a scan for its first byte, then a compare).
fn holds(hay: &[u8], needle: &[u8]) -> bool {
    let Some((&first, _)) = needle.split_first() else { return true };
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        match hay[at..hay.len() - needle.len() + 1].iter().position(|&b| b == first) {
            Some(i) => {
                if &hay[at + i..at + i + needle.len()] == needle {
                    return true;
                }
                at += i + 1;
            }
            None => return false,
        }
    }
    false
}

/// A shared workspace's script output as it is kept and keyed (vfs.rs SHARE): each
/// `rerun-if-changed` path the script gave under its checkout on disk (its current folder) as the
/// folder mount's (`vfs::virtual_of`), so the run of another checkout evaluates its own file there,
/// not the first checkout's.
fn shared_output(output: &str) -> String {
    let mut out = String::with_capacity(output.len());
    for piece in output.split_inclusive('\n') {
        let line = piece.trim_end_matches(['\n', '\r']);
        let mapped = ["cargo::", "cargo:"].iter().find_map(|prefix| {
            let p = line.strip_prefix(prefix)?.strip_prefix("rerun-if-changed=")?;
            Some(format!("{prefix}rerun-if-changed={}", crate::vfs::virtual_of(p)))
        });
        out.push_str(mapped.as_deref().unwrap_or(line));
        out.push_str(&piece[line.len()..]);
    }
    out
}

/// The inputs a script's output declares (its `rerun-if` lines, sorted, once each).
fn declared_inputs(output: &str) -> String {
    let mut lines: Vec<&str> = output.lines().filter(|l| {
        if *l == CHECKOUT_LINE {
            return true;
        }
        let rest = l.strip_prefix("cargo::").or_else(|| l.strip_prefix("cargo:")).unwrap_or("");
        rest.starts_with("rerun-if-changed=") || rest.starts_with("rerun-if-env-changed=")
    }).collect();
    lines.sort();
    lines.dedup();
    lines.join("\n")
}

/// Where the output of a shared workspace's script is (vfs.rs SHARE), and whether one stands
/// there (`previous`: the output of a build dir, or why it does not stand). The checkouts of one
/// repository have one run dir. With one output per package, a checkout whose inputs differ ran
/// the script into the folder another checkout's run was reading, and each made it again after
/// the other. So an output is kept per content of its inputs, in `<base_dir>-<hash>`: the hash of
/// the rerun key (`key_of`) that `declared`, the inputs the script's last output named, gives for
/// this checkout now (none yet: the package's files, cargo's rule for a script that names none).
/// An output made before its script's inputs were known, or one whose script names other inputs
/// from run to run, has another name: when the named one does not stand, any output of this
/// script that does is taken. The build prune (`prune_build`) keeps what the roots name.
fn shared_build_dir(base_dir: &str, declared: &str, key_of: &dyn Fn(&str) -> String, previous: &dyn Fn(&str) -> Result<String, String>) -> (String, Result<String, String>) {
    let named = format!("{}-{:016x}", base_dir, crate::sip::hash_bytes(key_of(declared).as_bytes()) as u64);
    let why = match previous(&named) {
        Ok(old) => return (named, Ok(old)),
        Err(why) => why,
    };
    let Some((parent, name)) = base_dir.rsplit_once('/') else { return (named, Err(why)) };
    let prefix = format!("{name}-");
    let mut others: Vec<String> = std::fs::read_dir(parent)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.starts_with(&prefix)).collect())
        .unwrap_or_default();
    others.sort();
    for other in others {
        let at = format!("{parent}/{other}");
        if at == named {
            continue;
        }
        if let Ok(old) = previous(&at) {
            return (at, Ok(old));
        }
    }
    (named, Err(why))
}

/// The build dirs one script pass uses (`<run dir>/build/<name>`): named in the pass's root,
/// `build/roots/<slot>`, before each is used, and use-locked for the life of the process. Its slot:
/// the checkout on disk and what the pass is for (the build script chain it runs in, the target, a
/// check, the root package), never content, so a pass of the same selection replaces its root.
/// The root's text: the checkout's folder on disk, then the build dirs' names. `prune_build`
/// (cache.rs prune_root) removes the build dirs no root names.
pub struct BuildRoot {
    build: std::path::PathBuf,
    path: std::path::PathBuf,
    head: String,
    /// (what the root file says now, the names this pass took)
    names: Mutex<(Vec<String>, Vec<String>)>,
}

/// The use locks this process holds (shared locks on `<build dir>/relax-use`), kept for its life.
static USE_LOCKS: Mutex<Vec<(std::path::PathBuf, std::fs::File)>> = Mutex::new(Vec::new());

fn build_lock(build: &std::path::Path, wait: bool) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(build.join("lock")).ok()?;
    let locked = if wait { f.lock().is_ok() } else { f.try_lock().is_ok() };
    locked.then_some(f)
}

impl BuildRoot {
    pub fn new(sess: &std::sync::Arc<crate::Session>, g: &Graph, target_dir: &str, check_only: bool) -> BuildRoot {
        let checkout = crate::vfs::on_disk(&crate::dirs::workspace_dir()).into_owned();
        let chain = std::env::var("RELAX_SCRIPT_CHAIN").unwrap_or_default();
        let subject = g.pkg.get(g.root).map(|(d, n, v)| format!("{}\t{n}@{v}", crate::dirs::anchored(d))).unwrap_or_default();
        let what = [chain.as_str(), target_triple(sess), if check_only { "check" } else { "run" }, subject.as_str()];
        BuildRoot::at(std::path::Path::new(target_dir).join("build"), checkout, &what)
    }

    /// The root of a pass in `build` of the checkout `checkout` (on disk) for `what`.
    fn at(build: std::path::PathBuf, checkout: String, what: &[&str]) -> BuildRoot {
        let mut slot = crate::sip::Sip128::new();
        slot.str("build-root-v1");
        slot.str(&checkout);
        for part in what {
            slot.str(part);
        }
        let path = build.join("roots").join(format!("{:032x}", slot.finish128()));
        let now: Vec<String> = std::fs::read_to_string(&path).unwrap_or_default().lines().skip(1).map(str::to_string).collect();
        BuildRoot { build, path, head: checkout, names: Mutex::new((now, Vec::new())) }
    }

    fn text(&self, names: &[String]) -> String {
        let mut text = format!("{}\n", self.head);
        for n in names {
            text.push_str(n);
            text.push('\n');
        }
        text
    }

    fn write(&self, names: &[String]) {
        let text = self.text(names);
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t == text) {
            return;
        }
        let Some(roots) = self.path.parent() else { return };
        let _ = std::fs::create_dir_all(roots);
        let Some(_lock) = build_lock(&self.build, true) else { return };
        let tmp = roots.join(format!(".{}.{}", self.path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
        let _ = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path));
    }

    /// The build dir `name` for this pass: named in the root first (written only when the root
    /// does not name it yet: a warm pass writes nothing), made, and use-locked. Its full path.
    pub fn take(&self, name: &str) -> String {
        let dir = self.build.join(name);
        {
            let mut names = self.names.lock().unwrap();
            if !names.1.iter().any(|n| n == name) {
                names.1.push(name.to_string());
            }
            if !names.0.iter().any(|n| n == name) {
                names.0.push(name.to_string());
                self.write(&names.0);
            }
        }
        let _ = std::fs::create_dir_all(&dir);
        let mut locks = USE_LOCKS.lock().unwrap();
        if !locks.iter().any(|(p, _)| *p == dir) {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join("relax-use")) {
                let _ = f.lock_shared();
                crate::zone_alloc::kept_with(|| locks.push((dir.clone(), f)));
            }
        }
        dir.to_string_lossy().into_owned()
    }

    /// The pass is over: its root names exactly what it took.
    pub fn finish(&self) {
        let mut names = self.names.lock().unwrap();
        names.1.sort();
        names.0 = names.1.clone();
        self.write(&names.0);
    }
}

/// Keeps `build/` to what the roots name (`BuildRoot`): every other build dir goes, unless a run
/// holds its use lock or it changed since `since` (a Relax build without roots may be writing it),
/// and so does the root of a checkout that is gone. A base dir (`<name>-<16 hex>`, the claim and
/// what a shared script declared) stays while a dir of it (`<base>-<16 hex>`) does. Under the build
/// lock, taken only if free (else the next start prunes), or with `wait` once it is (a child being
/// spawned may hold it a moment after its holder let go, native.rs `prune`): what goes is moved
/// to `build/trash` there and deleted after it. Returns how many dirs went.
pub fn prune_build(build: &std::path::Path, since: std::time::SystemTime, wait: bool) -> usize {
    if !build.is_dir() {
        return 0;
    }
    let recent = |p: &std::path::Path| [p.to_path_buf(), p.join("out")].iter().any(|p| std::fs::symlink_metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t >= since));
    let trash = build.join("trash");
    let mut gone = 0;
    {
        let Some(_lock) = build_lock(build, wait) else { return 0 };
        let mut named: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in std::fs::read_dir(build.join("roots")).into_iter().flatten().flatten() {
            let p = e.path();
            // (`.<slot>.<pid>`: a root being written, or what a writer that ended midway left)
            if e.file_name().to_string_lossy().starts_with('.') {
                if !recent(&p) {
                    let _ = std::fs::remove_file(&p);
                }
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let mut lines = text.lines();
            match lines.next() {
                Some(checkout) if std::path::Path::new(checkout).is_dir() => named.extend(lines.map(str::to_string)),
                // a checkout that is gone
                _ => { let _ = std::fs::remove_file(&p); }
            }
        }
        let mut doomed: Vec<String> = Vec::new();
        let names: Vec<String> = std::fs::read_dir(build).into_iter().flatten().flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !matches!(n.as_str(), "roots" | "trash") && !n.starts_with('.'))
            .collect();
        for name in &names {
            let p = build.join(name);
            if named.contains(name) || recent(&p) {
                continue;
            }
            // a run holds it (its use lock: shared while the run lives)
            let held = std::fs::OpenOptions::new().read(true).write(true).open(p.join("relax-use")).is_ok_and(|f| f.try_lock().is_err());
            if !held {
                doomed.push(name.clone());
            }
        }
        // a base dir stays while one of its outputs does
        let staying: Vec<&String> = names.iter().filter(|n| !doomed.contains(n)).collect();
        doomed.retain(|base| !staying.iter().any(|n| n.len() == base.len() + 17 && n.starts_with(base.as_str()) && n.as_bytes()[base.len()] == b'-'));
        if !doomed.is_empty() && std::fs::create_dir_all(&trash).is_ok() {
            for name in &doomed {
                if std::fs::rename(build.join(name), trash.join(format!("{name}.{}", std::process::id()))).is_ok() {
                    gone += 1;
                }
            }
        }
    }
    // (what a prune that ended midway left there goes too)
    if trash.is_dir() {
        let _ = std::fs::remove_dir_all(&trash);
    }
    gone
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
/// cold boot of std on a few threads): half the cores, 2..=8. Four slots queued two of a large app's six
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
        n = crate::pool::wait(&SCRIPT_FREE, &SCRIPT_RUNNING, n).unwrap_or_else(|e| e.into_inner());
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

/// A Relax crate's build script (libs/relax/self_stamp.rs), answered without running it: the text
/// of its OUT_DIR/src_stamp.rs. The stamp hashes what the script hashes of the package itself (its files,
/// nested packages, `target`, hidden entries and its tests, benches and examples left out, read
/// through the VFS; the enabled features), with this crate's own build in place of rustc's
/// toolchain and flags. Only relax-compiler's codegen identity reads a stamp, from its native
/// build: a program compiling these crates from source never compares it.
fn self_stamp_text(dir: &str, pkg: &str, features: &[String]) -> Result<String, String> {
    fn files(dir: &std::path::Path, rel: &str, out: &mut Vec<(String, std::path::PathBuf)>) {
        let Ok(mut rd) = crate::vfs::read_dir(dir) else { return };
        rd.sort();
        for (name, is_dir) in rd {
            if name.starts_with('.') {
                continue;
            }
            let p = dir.join(&name);
            let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
            if is_dir {
                if name == "target" || crate::vfs::is_file(p.join("Cargo.toml")) || (rel.is_empty() && matches!(name.as_str(), "tests" | "benches" | "examples")) {
                    continue;
                }
                files(&p, &r, out);
            } else {
                out.push((r, p));
            }
        }
    }
    let mut list = Vec::new();
    files(std::path::Path::new(dir), "", &mut list);
    let mut buf = Vec::new();
    let put = |b: &[u8], buf: &mut Vec<u8>| {
        buf.extend_from_slice(&(b.len() as u64).to_le_bytes());
        buf.extend_from_slice(b);
    };
    put(b"relax-src-stamp 1", &mut buf);
    put(pkg.as_bytes(), &mut buf);
    for (rel, p) in &list {
        put(rel.as_bytes(), &mut buf);
        put(&crate::vfs::read(p).map_err(|e| format!("{}: {}", p.display(), e))?, &mut buf);
    }
    let mut fs: Vec<String> = features.iter().map(|f| format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_"))).collect();
    fs.sort();
    for f in &fs {
        put(f.as_bytes(), &mut buf);
    }
    put(b"makepad run", &mut buf);
    put(env!("CARGO_PKG_VERSION").as_bytes(), &mut buf);
    let d = relax_base::sha256::sha256(&buf);
    let stamp = u128::from_le_bytes(d[..16].try_into().unwrap());
    Ok(format!("/// This crate's source stamp (libs/relax/self_stamp.rs).\npub const SRC_STAMP: u128 = 0x{:032x};\n", stamp))
}

static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The lease a devhub runs this process in (`MAKEPAD_DEVHUB_LEASE`: its ledger dir and the
/// lease), else `none`.
fn devhub_lease() -> String {
    std::env::var("MAKEPAD_DEVHUB_LEASE").ok().and_then(|mark| mark.rsplit('/').next().filter(|id| !id.is_empty()).map(str::to_string)).unwrap_or_else(|| "none".to_string())
}

/// A build script's run that took its `relax-claim` writes in it who holds it: `<pid> <lease>`.
fn name_claim(claim: &std::fs::File) {
    use std::io::{Seek, Write};
    let mut f = claim;
    let _ = f.set_len(0);
    let _ = f.seek(std::io::SeekFrom::Start(0));
    let _ = writeln!(f, "{} {}", std::process::id(), devhub_lease());
}

/// Who holds the claim at `path`, said: "held by pid N (lease L...)", from what its holder wrote in
/// it ([`name_claim`]), and on Linux the kernel's lock table, which names a holder that wrote
/// nothing (a run of an older relax).
fn claim_holder(path: &str) -> String {
    let named = std::fs::read_to_string(path).unwrap_or_default();
    let mut words = named.split_whitespace();
    let (pid, lease) = (words.next().and_then(|p| p.parse::<u32>().ok()), words.next().map(str::to_string));
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let locked = std::fs::metadata(path).ok().and_then(|m| {
            let ino = m.ino().to_string();
            std::fs::read_to_string("/proc/locks").ok()?.lines().find_map(|line| {
                let f: Vec<&str> = line.split_whitespace().collect();
                (f.get(1) == Some(&"FLOCK") && f.get(5).and_then(|dev| dev.rsplit(':').next()) == Some(ino.as_str())).then(|| f.get(4)?.parse::<u32>().ok()).flatten()
            })
        });
        if let Some(holder) = locked.filter(|holder| Some(*holder) != pid) {
            return format!("held by pid {holder} (lease unknown: it named none)");
        }
    }
    match (pid, lease) {
        (Some(pid), Some(lease)) => format!("held by pid {pid} (lease {lease})"),
        _ => "held by a run that has not named itself (lease unknown)".to_string(),
    }
}

#[cfg(test)]
mod script_error_tests {
    /// The cause a failing build script names, by what its stderr holds (relax PR 59).
    #[test]
    fn a_failing_scripts_cause_is_what_it_printed_first() {
        let summary = "error: could not compile `build_script_build` due to 1 previous error: not started (`relax run` checks the app's own crates whole before they run, as cargo builds them)";
        let relax = format!("/app/build.rs:2:9: warning: unused variable: `unused` [unused_variables]\n/app/build.rs:3:21: E0308: mismatched types: expected `u8`, found `&str`\n{summary}\n");
        assert_eq!(super::script_error_cause(&relax), "/app/build.rs:3:21: E0308: mismatched types: expected `u8`, found `&str`", "a compile failure names its error, not a warning or the summary");
        let rustc = "error[E0308]: mismatched types\n --> build.rs:3:21\n  |\nerror: could not compile `app` (build script) due to 1 previous error\n";
        assert_eq!(super::script_error_cause(rustc), "error[E0308]: mismatched types", "rustc's own diagnostic shape");
        assert_eq!(super::script_error_cause(&format!("{{\"message\":\"index out of bounds\",\"type\":\"panic\"}}\n{summary}\n")), "index out of bounds", "a panic's message");
        assert_eq!(super::script_error_cause("one\ntwo\nthree\nfour\n"), "two | three | four", "else its last lines");
    }

    /// A build script's claim names its holder to whoever waits on it: the pid and devhub lease
    /// its holder wrote in it, the kernel's lock holder on Linux, else that it named none.
    #[test]
    fn a_claims_waiter_hears_who_holds_it() {
        let path = std::env::temp_dir().join(format!("relax-claim-holder-{}", std::process::id()));
        let path_s = path.to_string_lossy().to_string();
        std::fs::write(&path, "4242 L7\n").unwrap();
        assert_eq!(super::claim_holder(&path_s), "held by pid 4242 (lease L7)", "what its holder wrote");
        std::fs::write(&path, "").unwrap();
        assert_eq!(super::claim_holder(&path_s), "held by a run that has not named itself (lease unknown)");
        let claim = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        claim.lock().unwrap();
        super::name_claim(&claim);
        assert_eq!(super::claim_holder(&path_s), format!("held by pid {} (lease {})", std::process::id(), super::devhub_lease()), "the run that took it named itself");
        drop(claim);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod shared_script_tests {
    use super::*;
    use std::path::Path;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("relax-unitgraph-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A shared workspace's script output (vfs.rs SHARE) is keyed and named by what its inputs
    /// hold, under the mount's paths: two checkouts with the same files share it, a checkout whose
    /// file differs gets its own, also where the script declared a path under its folder on disk.
    #[test]
    fn a_shared_scripts_output_is_named_by_what_its_inputs_hold() {
        let base = scratch("shared");
        let (a, b) = (base.join("a"), base.join("b"));
        for c in [&a, &b] {
            std::fs::create_dir_all(c.join("proto/inner")).unwrap();
            std::fs::create_dir_all(c.join("target/x")).unwrap();
            std::fs::write(c.join("proto/inner/m.proto"), "message A").unwrap();
            std::fs::write(c.join("build.rs"), "fn main() {}").unwrap();
            std::fs::write(c.join("target/x/junk"), "junk").unwrap();
        }
        // content: what the files hold, wherever and whenever they were written
        assert_eq!(content(&a.join("proto"), false), content(&b.join("proto"), false), "two checkouts, one content");
        std::fs::write(b.join("target/x/junk"), "other").unwrap();
        std::fs::write(b.join(".hidden"), "h").unwrap();
        assert_eq!(content(&a, true), content(&b, true), "a package's target dirs and hidden entries are not its files");
        assert_ne!(content(&a, false), content(&b, false), "a declared folder's are");
        assert_ne!(content(&a.join("build.rs"), false), content(&a.join("proto/inner/m.proto"), false));
        assert_eq!(content(&a.join("none"), false), 0);
        #[cfg(unix)]
        {
            // a linked folder is walked (cargo follows links); a link back up ends there
            std::fs::create_dir_all(base.join("elsewhere")).unwrap();
            std::fs::write(base.join("elsewhere/f"), "1").unwrap();
            std::os::unix::fs::symlink(base.join("elsewhere"), base.join("linked")).unwrap();
            let before = content(&base.join("linked"), false);
            std::fs::write(base.join("elsewhere/f"), "2").unwrap();
            assert_ne!(before, content(&base.join("linked"), false), "a change behind a linked folder");
            std::os::unix::fs::symlink(base.join("elsewhere"), base.join("elsewhere/up")).unwrap();
            let looped = content(&base.join("linked"), false);
            assert_eq!(looped, content(&base.join("linked"), false), "a loop ends, the same each time");
        }

        let root = crate::vfs::normalize_path(&base.join("vfs/share/ws").to_string_lossy());
        let disk_a = crate::vfs::normalize_path(&a.to_string_lossy());
        crate::vfs::mount_folder(&root, &a);
        let raw = format!("cargo:rerun-if-changed={disk_a}/proto\r\ncargo:rustc-cfg=x\ncargo::rerun-if-env-changed=PROTOC\ncargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\n");
        let output = shared_output(&raw);
        assert_eq!(output, format!("cargo:rerun-if-changed={root}/proto\r\ncargo:rustc-cfg=x\ncargo::rerun-if-env-changed=PROTOC\ncargo:rerun-if-changed=build.rs\ncargo:rerun-if-changed=build.rs\n"),
            "a path under the checkout on disk is the mount's; every other byte is kept");
        assert_eq!(declared_inputs(&output), format!("cargo::rerun-if-env-changed=PROTOC\ncargo:rerun-if-changed={root}/proto\ncargo:rerun-if-changed=build.rs"), "sorted, once each");

        let key_in = |checkout: &Path| {
            crate::vfs::mount_folder(&root, checkout);
            rerun_key(&output, &root, "env", Some(&SharedContent::default())).unwrap()
        };
        let ka = key_in(&a);
        assert!(!ka.contains(&disk_a) && ka.contains("f c") && ka.contains(&format!(" {root}/proto\n")), "a key names the mount, never a checkout: {ka}");
        assert_eq!(ka, key_in(&b), "checkout B evaluates its own files, the same content");
        std::fs::write(b.join("proto/inner/m.proto"), "message B").unwrap();
        assert_ne!(ka, key_in(&b), "its own file differs");
        let shared = SharedContent::default();
        let none = rerun_key("cargo:rustc-cfg=x\n", &root, "env", Some(&shared)).unwrap();
        assert!(none.contains("p c") && none.contains(&format!(" {root}\n")), "a script that names nothing: its package's files: {none}");

        // the build dir of one content of the inputs, else any of this script's that stands
        let build = base.join("build");
        std::fs::create_dir_all(&build).unwrap();
        let base_dir = format!("{}/s-0123456789abcdef", crate::vfs::normalize_path(&build.to_string_lossy()));
        let stands = |at: &str| std::fs::read_to_string(format!("{at}/output")).map_err(|_| "no previous output".to_string());
        let key_of = |declared: &str| format!("key of {declared}");
        let (named, r) = shared_build_dir(&base_dir, "decl", &key_of, &stands);
        assert_eq!((r, named.len()), (Err("no previous output".to_string()), base_dir.len() + 17));
        assert!(named.starts_with(&format!("{base_dir}-")));
        assert_eq!(named, shared_build_dir(&base_dir, "decl", &key_of, &stands).0, "one name per key");
        assert_ne!(named, shared_build_dir(&base_dir, "other", &key_of, &stands).0);
        let older = format!("{base_dir}-00000000000000aa");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::write(format!("{older}/output"), "old").unwrap();
        assert_eq!(shared_build_dir(&base_dir, "decl", &key_of, &stands), (older.clone(), Ok("old".to_string())), "an output made under another name");
        std::fs::create_dir_all(&named).unwrap();
        std::fs::write(format!("{named}/output"), "named").unwrap();
        assert_eq!(shared_build_dir(&base_dir, "decl", &key_of, &stands), (named, Ok("named".to_string())), "the named one first");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// `build/` keeps what the roots name: a pass names each build dir in its root before it uses
    /// it and leaves the root naming exactly what it took; the prune removes what no root names and
    /// no run holds, the root of a checkout that is gone with what only it named, and keeps a base
    /// dir while one of its outputs stays; it does nothing while another holds the build lock.
    #[test]
    fn the_build_prune_keeps_what_the_roots_name() {
        let base = scratch("prune");
        let (build, checkout) = (base.join("build"), base.join("checkout"));
        std::fs::create_dir_all(&checkout).unwrap();
        let ck = checkout.to_string_lossy().into_owned();
        let root_text = || {
            let roots: Vec<_> = std::fs::read_dir(build.join("roots")).unwrap().flatten().map(|e| e.path()).collect();
            assert_eq!(roots.len(), 1, "{roots:?}");
            std::fs::read_to_string(&roots[0]).unwrap()
        };
        let one = BuildRoot::at(build.clone(), ck.clone(), &["run"]);
        assert_eq!(one.take("held-0123456789abcdef"), build.join("held-0123456789abcdef").to_string_lossy());
        assert_eq!(root_text(), format!("{ck}\nheld-0123456789abcdef\n"), "named before it is used");
        one.finish();
        let two = BuildRoot::at(build.clone(), ck.clone(), &["run"]);
        two.take("again-0123456789abcdef");
        two.take("again-0123456789abcdef");
        assert_eq!(root_text(), format!("{ck}\nheld-0123456789abcdef\nagain-0123456789abcdef\n"));
        two.finish();
        assert_eq!(root_text(), format!("{ck}\nagain-0123456789abcdef\n"), "the pass's root names what it took");

        for n in ["named-0123456789abcdef", "s-0123456789abcdef", "s-0123456789abcdef-00000000000000aa", "t-0123456789abcdef", "unnamed-0123456789abcdef", "gone-0123456789abcdef"] {
            std::fs::create_dir_all(build.join(n).join("out")).unwrap();
        }
        let roots = build.join("roots");
        std::fs::write(roots.join("other"), format!("{ck}\nnamed-0123456789abcdef\ns-0123456789abcdef-00000000000000aa\n")).unwrap();
        std::fs::write(roots.join("gone"), format!("{}\ngone-0123456789abcdef\n", base.join("deleted").display())).unwrap();
        std::fs::write(roots.join(".writing.1"), "").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        {
            let _other = build_lock(&build, true).unwrap();
            assert_eq!(prune_build(&build, later, false), 0, "the build lock is another's");
        }
        assert_eq!(prune_build(&build, std::time::UNIX_EPOCH, true), 0, "what changed since `since` stays");
        assert_eq!(prune_build(&build, later, true), 3);
        let mut left: Vec<String> = std::fs::read_dir(&build).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["again-0123456789abcdef", "held-0123456789abcdef", "lock", "named-0123456789abcdef", "roots", "s-0123456789abcdef", "s-0123456789abcdef-00000000000000aa"],
            "named, held by this run (its use lock), and a base dir with an output; no trash");
        let mut roots_left: Vec<String> = std::fs::read_dir(&roots).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        roots_left.retain(|n| n.len() != 32);
        assert_eq!(roots_left, ["other"], "a gone checkout's root and a stale root write go");
        std::fs::remove_dir_all(&base).unwrap();
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&TO_REMOVE, Mutex::new(Vec::new()));
    reset(&SCRIPT_RUNNING, Mutex::new(0));
}
