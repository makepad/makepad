//! Relax's own crate resolution: the unit graph of one package as `cargo build --release -p <pkg>
//! --unit-graph` gives it for the host target, computed from the manifests (Cargo.toml of the
//! workspace and of every package reached, Cargo.lock for registry packages), without running
//! cargo. The output is cargo's unit graph (the fields of its JSON that unitgraph::load reads),
//! handed over in memory (`Units`; a build script's child process reads it as that JSON). It was
//! checked equal to cargo's, unit by unit, for every app on macOS, Linux and Windows before
//! cargo's path was deleted (2026-10-06).
//!
//! Rules followed (cargo, resolver 2; https://doc.rust-lang.org/cargo/reference/):
//! - packages: path dependencies (relative to the manifest), `[patch.crates-io]` of the
//!   workspace root, registry packages from a vendored directory when `.cargo/config.toml`
//!   replaces crates-io with one (`[source.crates-io] replace-with`, `[source.<x>] directory`;
//!   the config files of the start folder and every folder above it, the nearest first, as cargo
//!   reads them), else from Cargo.lock (sources under $CARGO_HOME/registry/src);
//!   `package = ".."` renames; `workspace = true` inheritance from `[workspace.dependencies]` /
//!   `[workspace.package]`;
//! - features: `[features]` with `dep:x`, `x/f`, `x?/f`; implicit features of optional deps that
//!   no `dep:` value names; `default` unless a dependency says `default-features = false`;
//!   features unify per package and per side: build-dependencies and proc macros (and everything
//!   they reach) resolve apart from normal dependencies (the host side);
//! - platforms: `[target.<triple or cfg(..)>.*dependencies]` against the host (CfgSet::host);
//!   dev-dependencies are not part of a build;
//! - targets: `[lib]` (name, path, proc-macro, crate-type, edition), `[[bin]]` plus discovered
//!   bins (src/main.rs, src/bin/*.rs, src/bin/*/main.rs) unless `autobins = false`, bins whose
//!   `required-features` are off are skipped; build scripts (`build = ..` or build.rs): a compile
//!   unit on the host side (deps = build-dependencies) and a run unit that the lib depends on
//!   (deps = the compile unit and the run units of dependencies with `links`).
//! - registry packages neither vendored nor in $CARGO_HOME: fetched from crates.io (registry.rs:
//!   sparse index, SHA-256, a cache under the Relax cache root, offline-first); with Cargo.lock its
//!   version, without, the newest the requirement accepts (one version per package);
//! Unsupported (an error naming it): git dependencies, registry packages missing from the vendored
//! directory (never looked up anywhere else, never fetched, then), registry packages that compile C
//! (a cc/cmake build-dependency), unknown command-line flags.

use crate::toml::{parse_toml, Toml, TomlTable};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One dependency entry of a manifest (a key may have several: normal + build + per platform).
#[derive(Clone, Debug)]
struct Dep {
    /// the key in the manifest (the extern crate name when renamed)
    key: String,
    /// the package it names (`package = ..`, else the key)
    package: String,
    build: bool,
    /// a `[dev-dependencies]` entry: part of the package's test targets only
    dev: bool,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    /// the platform predicate (`cfg(..)` or a triple); None = every platform
    platform: Option<String>,
    /// a path package directory, or None for a registry package
    path: Option<String>,
    /// the version requirement (`version = ..`; "*" when none)
    req: String,
}

#[derive(Clone, Debug)]
struct Target {
    name: String,
    path: String,
    edition: String,
    kind: Vec<String>,
    required_features: Vec<String>,
    /// a test target built with the libtest harness (`harness = false`: its own `main`)
    harness: bool,
}

#[derive(Debug)]
struct Manifest {
    dir: String,
    name: String,
    version: String,
    edition: String,
    links: Option<String>,
    build: Option<String>,
    lib: Option<Target>,
    bins: Vec<Target>,
    /// integration test targets: `[[test]]` plus discovered tests/*.rs, tests/*/main.rs
    tests: Vec<Target>,
    features: BTreeMap<String, Vec<String>>,
    deps: Vec<Dep>,
    proc_macro: bool,
    registry: bool,
}

impl Manifest {
    fn pkg_id(&self) -> String {
        if self.registry {
            return format!("registry+https://github.com/rust-lang/crates.io-index#{}@{}", self.name, self.version);
        }
        let last = self.dir.rsplit('/').next().unwrap_or("");
        // a file URL's path starts with `/`: a Windows drive path is `/C:/..` (RFC 8089), as cargo spells it
        let url_path = if self.dir.starts_with('/') { self.dir.clone() } else { format!("/{}", self.dir) };
        if last == self.name {
            format!("path+file://{}#{}", url_path, self.version)
        } else {
            format!("path+file://{}#{}@{}", url_path, self.name, self.version)
        }
    }
    /// Optional dep keys some feature names as `dep:x` (they get no implicit feature).
    fn dep_syntax(&self, key: &str) -> bool {
        self.features.values().flatten().any(|v| v.strip_prefix("dep:") == Some(key))
    }
    fn is_optional_dep(&self, key: &str) -> bool {
        self.deps.iter().any(|d| d.key == key && d.optional)
    }
    fn has_dep(&self, key: &str) -> bool {
        self.deps.iter().any(|d| d.key == key)
    }
}

/// Lexical normalisation of an absolute path (`.` and `..` removed, no symlinks followed: cargo's
/// paths::normalize_path).
fn normalize(p: &str) -> String {
    let p = crate::vfs::normalize_path(p);
    if p.starts_with('/') || (p.len() > 1 && p.as_bytes()[1] == b':') { p } else { crate::vfs::normalize_path(&format!("/{p}")) }
}

fn join(dir: &str, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') || (rel.len() > 1 && rel.as_bytes()[1] == b':') {
        normalize(&rel)
    } else {
        normalize(&format!("{}/{}", dir, rel))
    }
}

fn s(t: &TomlTable, k: &str) -> Option<String> {
    t.get(k).and_then(|v| v.as_str()).map(str::to_string)
}

fn b(t: &TomlTable, k: &str) -> Option<bool> {
    t.get(k).and_then(|v| v.as_bool())
}

fn strs(t: &TomlTable, k: &str) -> Vec<String> {
    t.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn table<'a>(t: &'a TomlTable, k: &str) -> Option<&'a TomlTable> {
    t.get(k).and_then(|v| v.as_table())
}

/// The workspace: root manifest dir, `[workspace.dependencies]`, `[workspace.package]`,
/// `[patch.crates-io]`, Cargo.lock's registry versions.
struct Workspace {
    root: String,
    ws_deps: TomlTable,
    ws_package: TomlTable,
    patches: HashMap<String, String>,
    lock: HashMap<String, Vec<String>>,
    members: Vec<String>,
    excludes: Vec<String>,
    /// the root manifest has a `[package]`: cargo makes the root package a member
    root_package: bool,
}

pub struct Resolver {
    ws: Workspace,
    /// crates-io's replacement directory (`cargo vendor`'s layout: `<name>` or `<name>-<version>`)
    /// and the config file that names it
    vendor: Option<(String, String)>,
    /// registry packages fetched for a program with neither a vendored directory nor a locked
    /// version: name -> (version, sources, the requirements that chose it)
    fetched: std::cell::RefCell<HashMap<String, (String, String, Vec<String>)>>,
    manifests: Vec<Manifest>,
    by_dir: HashMap<String, usize>,
    cfg: crate::cfg::CfgSet,
    /// `--allow-c`: registry packages that compile C are accepted (cargo builds them, native.rs)
    allow_c: bool,
}

fn read_toml(path: &str) -> Result<TomlTable, String> {
    let text = crate::vfs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    parse_toml(&text).map(|d| d.root).map_err(|e| format!("{}: {}", path, e))
}

impl Workspace {
    fn load(root: &str) -> Result<Workspace, String> {
        let root = normalize(root);
        let t = read_toml(&format!("{}/Cargo.toml", root))?;
        let w = table(&t, "workspace").cloned().unwrap_or_default();
        let mut patches = HashMap::new();
        if let Some(p) = table(&t, "patch").and_then(|p| table(p, "crates-io")) {
            for (k, v) in p {
                if let Some(path) = v.as_table().and_then(|v| s(v, "path")) {
                    patches.insert(k.clone(), join(&root, &path));
                }
            }
        }
        let mut lock: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(l) = read_toml(&format!("{}/Cargo.lock", root)) {
            if let Some(pkgs) = l.get("package").and_then(|p| p.as_array_of_tables()) {
                for p in pkgs {
                    if s(p, "source").is_some_and(|x| x.starts_with("registry+")) {
                        if let (Some(n), Some(v)) = (s(p, "name"), s(p, "version")) {
                            lock.entry(n).or_default().push(v);
                        }
                    }
                }
            }
        }
        Ok(Workspace {
            root_package: table(&t, "package").is_some(),
            members: strs(&w, "members"),
            excludes: strs(&w, "exclude").iter().map(|e| join(&root, e)).collect(),
            ws_deps: table(&w, "dependencies").cloned().unwrap_or_default(),
            ws_package: table(&w, "package").cloned().unwrap_or_default(),
            root,
            patches,
            lock,
        })
    }

    /// Member directories: the `members` entries with `*` segments expanded (folders only).
    fn member_dirs(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.root_package {
            out.push(self.root.clone());
        }
        for m in &self.members {
            let mut cur = vec![self.root.clone()];
            for seg in m.split('/') {
                let mut next = Vec::new();
                for c in &cur {
                    if seg.contains('*') {
                        let (pre, post) = seg.split_once('*').unwrap_or((seg, ""));
                        if let Ok(rd) = crate::vfs::read_dir(c) {
                            for (name, dir) in rd {
                                if dir && name.starts_with(pre) && name.ends_with(post) && !name.starts_with('.') {
                                    next.push(format!("{}/{}", c, name));
                                }
                            }
                        }
                    } else {
                        let p = join(c, seg);
                        if crate::vfs::is_dir(&p) {
                            next.push(p);
                        }
                    }
                }
                cur = next;
            }
            out.extend(cur);
        }
        out.retain(|d| !self.excludes.iter().any(|e| d == e || d.starts_with(&format!("{}/", e))));
        out
    }

    fn excluded(&self, dir: &str) -> bool {
        self.excludes.iter().any(|e| dir == e || dir.starts_with(&format!("{}/", e)))
    }

    /// Is `dir` a member? Cargo's members are the listed ones plus every path dependency of a
    /// member that lies inside the workspace root and is not excluded, transitively, through any
    /// dependency table (normal, dev, build, `[target.*]`; `workspace = true` entries by the
    /// workspace's own path): apps/commercial names Stage's and Sandbox's crates that way.
    fn has_member(&self, dir: &str) -> bool {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut todo = self.member_dirs();
        while let Some(m) = todo.pop() {
            let m = normalize(&m);
            if !seen.insert(m.clone()) {
                continue;
            }
            if m == dir {
                return true;
            }
            let Ok(t) = read_toml(&format!("{}/Cargo.toml", m)) else { continue };
            let mut tables: Vec<&TomlTable> = Vec::new();
            for k in ["dependencies", "dev-dependencies", "build-dependencies"] {
                tables.extend(table(&t, k));
            }
            if let Some(targets) = table(&t, "target") {
                for (_, tt) in targets {
                    if let Some(tt) = tt.as_table() {
                        for k in ["dependencies", "dev-dependencies", "build-dependencies"] {
                            tables.extend(table(tt, k));
                        }
                    }
                }
            }
            for tbl in tables {
                for (key, spec) in tbl {
                    let Some(spec) = spec.as_table() else { continue };
                    let path = if b(spec, "workspace") == Some(true) {
                        self.ws_deps.get(key).and_then(|w| w.as_table()).and_then(|w| s(w, "path")).map(|p| join(&self.root, &p))
                    } else {
                        s(spec, "path").map(|p| join(&m, &p))
                    };
                    if let Some(p) = path {
                        let p = normalize(&p);
                        if (p == self.root || p.starts_with(&format!("{}/", self.root))) && !self.excluded(&p) {
                            todo.push(p);
                        }
                    }
                }
            }
        }
        false
    }
}

impl Resolver {
    /// `dir` is where `makepad run` starts (the current folder, a git run's workspace): the
    /// workspace is found as cargo finds it (find_root).
    pub fn new(sess: &std::sync::Arc<crate::Session>, dir: &str) -> Result<Resolver, String> {
        Ok(Resolver { ws: Workspace::load(&find_root(dir)?)?, vendor: vendor_dir(dir)?, fetched: Default::default(), manifests: Vec::new(), by_dir: HashMap::new(), cfg: crate::cfg::CfgSet::host(sess, &[]), allow_c: sess.options.allow_c })
    }

    fn registry_dir(&self, name: &str, req: &str) -> Result<String, String> {
        if let Some((vdir, config)) = &self.vendor {
            return self.vendored_dir(vdir, config, name);
        }
        let Some(vers) = self.ws.lock.get(name) else {
            return self.fetched_dir(name, req);
        };
        // Cargo.lock may hold several versions of one package (bitflags 1 and 2 in one graph):
        // only the ones the requirement accepts are candidates; without a parsable match, all.
        let matching: Vec<String> = vers.iter().filter(|v| crate::registry::Version::parse(v).is_some_and(|ver| crate::registry::matches(req, &ver))).cloned().collect();
        let vers: &Vec<String> = if matching.is_empty() { vers } else { &matching };
        let home = std::env::var("CARGO_HOME").ok().or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/.cargo", h))).unwrap_or_default();
        let src = format!("{}/registry/src", home);
        for v in vers {
            if let Ok(rd) = std::fs::read_dir(&src) {
                for e in rd.flatten() {
                    let d = format!("{}/{}/{}-{}", src, e.file_name().to_string_lossy(), name, v);
                    if std::path::Path::new(&d).join("Cargo.toml").is_file() {
                        return Ok(d);
                    }
                }
            }
        }
        // the locked version from the crates.io index (registry.rs: cache first)
        let mut last = String::new();
        for v in vers {
            match crate::registry::exact(name, v).and_then(|r| crate::registry::sources(name, &r)) {
                Ok(d) => return Ok(d),
                Err(e) => last = e,
            }
        }
        Err(format!("registry package `{}` ({}) is not in {} and could not be fetched: {}", name, vers.join(", "), src, last))
    }

    /// A registry package of a program without Cargo.lock: the newest release the first
    /// requirement met accepts (one version per package: a later requirement must accept it too).
    fn fetched_dir(&self, name: &str, req: &str) -> Result<String, String> {
        if let Some((v, d, reqs)) = self.fetched.borrow_mut().get_mut(name) {
            let ver = crate::registry::Version::parse(v).ok_or_else(|| format!("registry package `{}`: bad version {}", name, v))?;
            if !crate::registry::matches(req, &ver) {
                return Err(format!("registry package `{}`: {} (chosen for {}) does not satisfy `{}` -- add a Cargo.lock that picks one version", name, v, reqs.join(", "), req));
            }
            reqs.push(req.to_string());
            return Ok(d.clone());
        }
        let r = crate::registry::pick(name, req, &[])?;
        let d = crate::registry::sources(name, &r)?;
        self.fetched.borrow_mut().insert(name.to_string(), (r.version.clone(), d.clone(), vec![req.to_string()]));
        Ok(d)
    }

    /// Registry package `name` in the vendored directory `vdir`: `<name>` (one version) or
    /// `<name>-<version>` (cargo vendor's names when it holds several); with Cargo.lock, the locked
    /// version, without, the only one there.
    fn vendored_dir(&self, vdir: &str, config: &str, name: &str) -> Result<String, String> {
        let mut found: Vec<(String, String)> = Vec::new();
        for (entry, is_dir) in crate::vfs::read_dir(vdir).map_err(|e| format!("{} (crates-io's replacement, {}): {}", vdir, config, e))? {
            if !is_dir || !(entry == name || entry.strip_prefix(name).is_some_and(|r| r.starts_with('-') && r[1..].starts_with(|c: char| c.is_ascii_digit()))) {
                continue;
            }
            let d = format!("{}/{}", vdir, entry);
            if let Ok(t) = read_toml(&format!("{}/Cargo.toml", d)) {
                if let Some(v) = table(&t, "package").and_then(|p| s(p, "version")) {
                    if table(&t, "package").and_then(|p| s(p, "name")).as_deref() == Some(name) {
                        found.push((v, d));
                    }
                }
            }
        }
        let locked = self.ws.lock.get(name);
        let pick: Vec<&(String, String)> = match locked {
            Some(vers) => found.iter().filter(|(v, _)| vers.contains(v)).collect(),
            None => found.iter().collect(),
        };
        match pick.as_slice() {
            [(_, d)] => Ok(d.clone()),
            [] => Err(format!(
                "registry package `{}`{} is not vendored in {} (crates-io's replacement, {}): a vendored workspace never fetches from crates.io",
                name,
                locked.map_or(String::new(), |v| format!(" ({})", v.join(", "))),
                vdir,
                config
            )),
            _ => Err(format!("registry package `{}`: {} has versions {} and no Cargo.lock picks one", name, vdir, pick.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }

    /// The manifest of the package at `dir` (loaded once).
    fn load(&mut self, dir: &str, registry: bool) -> Result<usize, String> {
        let dir = normalize(dir);
        if let Some(&i) = self.by_dir.get(&dir) {
            return Ok(i);
        }
        let t = read_toml(&format!("{}/Cargo.toml", dir))?;
        let pkg = table(&t, "package").ok_or_else(|| format!("{}/Cargo.toml: no [package]", dir))?;
        let inherit = |k: &str| -> Option<String> {
            match pkg.get(k) {
                Some(Toml::Str(v, _)) => Some(v.clone()),
                Some(Toml::Table(x)) if b(x, "workspace") == Some(true) => s(&self.ws.ws_package, k),
                _ => None,
            }
        };
        let name = inherit("name").ok_or_else(|| format!("{}/Cargo.toml: no package name", dir))?;
        let version = inherit("version").unwrap_or_else(|| "0.0.0".to_string());
        let edition = inherit("edition").unwrap_or_else(|| "2015".to_string());
        let links = s(pkg, "links");
        let build = match pkg.get("build") {
            Some(Toml::Str(p, _)) => Some(join(&dir, p)),
            Some(Toml::Bool(false, _)) => None,
            _ => {
                let p = format!("{}/build.rs", dir);
                crate::vfs::is_file(&p).then_some(p)
            }
        };
        // [lib]
        let lt = table(&t, "lib");
        let lib_path = lt.and_then(|l| s(l, "path")).map(|p| join(&dir, &p)).or_else(|| {
            let p = format!("{}/src/lib.rs", dir);
            (pkg.get("autolib").and_then(|v| v.as_bool()) != Some(false) && crate::vfs::is_file(&p)).then_some(p)
        });
        let proc_macro = lt.is_some_and(|l| b(l, "proc-macro") == Some(true) || b(l, "proc_macro") == Some(true));
        let lib = lib_path.map(|path| {
            let crate_types = lt.map(|l| strs(l, "crate-type")).unwrap_or_default();
            let kind = if proc_macro {
                vec!["proc-macro".to_string()]
            } else if crate_types.is_empty() {
                vec!["lib".to_string()]
            } else {
                crate_types
            };
            Target {
                name: lt.and_then(|l| s(l, "name")).unwrap_or_else(|| name.replace('-', "_")),
                path,
                edition: lt.and_then(|l| s(l, "edition")).unwrap_or_else(|| edition.clone()),
                kind,
                required_features: lt.map(|l| strs(l, "required-features")).unwrap_or_default(),
                harness: lt.and_then(|l| b(l, "harness")).unwrap_or(true),
            }
        });
        // [[bin]] + discovered bins
        let mut bins: Vec<Target> = Vec::new();
        let explicit = t.get("bin").and_then(|b| b.as_array_of_tables()).map(|v| v.to_vec()).unwrap_or_default();
        for bt in &explicit {
            let Some(bname) = s(bt, "name") else { continue };
            let path = match s(bt, "path") {
                Some(p) => join(&dir, &p),
                None => {
                    let cands = [format!("{}/src/bin/{}.rs", dir, bname), format!("{}/src/bin/{}/main.rs", dir, bname), format!("{}/src/main.rs", dir)];
                    let pick = if bname == name { cands.iter().rev().find(|c| crate::vfs::is_file(c)) } else { cands[..2].iter().find(|c| crate::vfs::is_file(c)) };
                    match pick {
                        Some(p) => p.clone(),
                        None => continue,
                    }
                }
            };
            bins.push(Target { name: bname, path, edition: s(bt, "edition").unwrap_or_else(|| edition.clone()), kind: vec!["bin".to_string()], required_features: strs(bt, "required-features"), harness: true });
        }
        // autobins: default on, except for edition 2015 packages with explicit [[bin]] targets
        let autobins = match b(pkg, "autobins") {
            Some(v) => v,
            None => edition != "2015" || explicit.is_empty(),
        };
        if autobins {
            let mut found: Vec<(String, String)> = Vec::new();
            let main = format!("{}/src/main.rs", dir);
            if crate::vfs::is_file(&main) {
                found.push((name.clone(), main));
            }
            if let Ok(rd) = crate::vfs::read_dir(format!("{}/src/bin", dir)) {
                for (n, is_dir) in rd {
                    if is_dir {
                        let p = format!("{}/src/bin/{}/main.rs", dir, n);
                        if crate::vfs::is_file(&p) {
                            found.push((n, p));
                        }
                    } else if let Some(stem) = n.strip_suffix(".rs") {
                        found.push((stem.to_string(), format!("{}/src/bin/{}", dir, n)));
                    }
                }
            }
            for (n, p) in found {
                // an explicit target with that name or path replaces the discovered one
                if !bins.iter().any(|b| b.name == n || b.path == p) {
                    bins.push(Target { name: n, path: p, edition: edition.clone(), kind: vec!["bin".to_string()], required_features: Vec::new(), harness: true });
                }
            }
        }
        // [[test]] + discovered tests (cargo's target discovery: tests/*.rs and tests/*/main.rs
        // unless `autotests = false`; an explicit target replaces the discovered one of its name
        // or path; edition 2015 packages with explicit [[test]] targets discover none)
        let mut tests: Vec<Target> = Vec::new();
        let explicit_tests = t.get("test").and_then(|b| b.as_array_of_tables()).map(|v| v.to_vec()).unwrap_or_default();
        for tt in &explicit_tests {
            let Some(tname) = s(tt, "name") else { continue };
            let path = match s(tt, "path") {
                Some(p) => join(&dir, &p),
                None => {
                    let cands = [format!("{}/tests/{}.rs", dir, tname), format!("{}/tests/{}/main.rs", dir, tname)];
                    match cands.iter().find(|c| crate::vfs::is_file(c)) {
                        Some(p) => p.clone(),
                        None => continue,
                    }
                }
            };
            tests.push(Target { name: tname, path, edition: s(tt, "edition").unwrap_or_else(|| edition.clone()), kind: vec!["test".to_string()], required_features: strs(tt, "required-features"), harness: b(tt, "harness").unwrap_or(true) });
        }
        let autotests = match b(pkg, "autotests") {
            Some(v) => v,
            None => edition != "2015" || explicit_tests.is_empty(),
        };
        if autotests {
            let mut found: Vec<(String, String)> = Vec::new();
            if let Ok(rd) = crate::vfs::read_dir(format!("{}/tests", dir)) {
                for (n, is_dir) in rd {
                    if is_dir {
                        let p = format!("{}/tests/{}/main.rs", dir, n);
                        if crate::vfs::is_file(&p) {
                            found.push((n, p));
                        }
                    } else if let Some(stem) = n.strip_suffix(".rs") {
                        found.push((stem.to_string(), format!("{}/tests/{}", dir, n)));
                    }
                }
            }
            found.sort();
            for (n, p) in found {
                if !tests.iter().any(|x| x.name == n || x.path == p) {
                    tests.push(Target { name: n, path: p, edition: edition.clone(), kind: vec!["test".to_string()], required_features: Vec::new(), harness: true });
                }
            }
        }
        // [features]
        let mut features = BTreeMap::new();
        if let Some(f) = table(&t, "features") {
            for (k, v) in f {
                features.insert(k.clone(), v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default());
            }
        }
        // dependencies
        let mut deps = Vec::new();
        let add = |deps: &mut Vec<Dep>, tbl: Option<&TomlTable>, build: bool, dev: bool, platform: Option<String>| -> Result<(), String> {
            let Some(tbl) = tbl else { return Ok(()) };
            for (key, spec) in tbl {
                let (mut st, from_ws) = match spec {
                    Toml::Str(v, _) => {
                        let mut t = TomlTable::new();
                        t.insert("version".to_string(), Toml::Str(v.clone(), Default::default()));
                        (t, false)
                    }
                    Toml::Table(t) => (t.clone(), b(t, "workspace") == Some(true)),
                    _ => continue,
                };
                let mut base_dir = dir.clone();
                if from_ws {
                    // [workspace.dependencies] entry, member's features added, optional from the member
                    let w = match self.ws.ws_deps.get(key) {
                        Some(Toml::Str(v, _)) => {
                            let mut t = TomlTable::new();
                            t.insert("version".to_string(), Toml::Str(v.clone(), Default::default()));
                            t
                        }
                        Some(Toml::Table(t)) => t.clone(),
                        _ => return Err(format!("{}/Cargo.toml: `{}` inherits from [workspace.dependencies], which has no such entry", dir, key)),
                    };
                    let mut merged = w;
                    let mut feats = strs(&merged, "features");
                    feats.extend(strs(&st, "features"));
                    merged.insert("features".to_string(), Toml::Array(feats.into_iter().map(|f| Toml::Str(f, Default::default())).collect()));
                    if let Some(o) = st.get("optional") {
                        merged.insert("optional".to_string(), o.clone());
                    }
                    st = merged;
                    base_dir = self.ws.root.clone();
                }
                // a dev-dependency is built only for the root package's tests: a git one (which
                // makepad run cannot resolve) is left out here, and a test that names it fails to resolve it
                if dev && st.contains_key("git") {
                    continue;
                }
                if st.contains_key("git") {
                    return Err(format!("{}/Cargo.toml: `{}` is a git dependency; makepad run resolves path and registry packages only", dir, key));
                }
                let package = s(&st, "package").unwrap_or_else(|| key.clone());
                let path = match s(&st, "path") {
                    Some(p) => Some(join(&base_dir, &p)),
                    None => self.ws.patches.get(&package).cloned(),
                };
                deps.push(Dep {
                    key: key.clone(),
                    package,
                    build,
                    dev,
                    optional: b(&st, "optional") == Some(true),
                    default_features: b(&st, "default-features").or_else(|| b(&st, "default_features")).unwrap_or(true),
                    features: strs(&st, "features"),
                    platform: platform.clone(),
                    path,
                    req: s(&st, "version").unwrap_or_else(|| "*".to_string()),
                });
            }
            Ok(())
        };
        add(&mut deps, table(&t, "dependencies"), false, false, None)?;
        add(&mut deps, table(&t, "build-dependencies").or_else(|| table(&t, "build_dependencies")), true, false, None)?;
        // a registry package's dev-dependencies are never built (only the root package's tests are)
        if !registry {
            add(&mut deps, table(&t, "dev-dependencies").or_else(|| table(&t, "dev_dependencies")), false, true, None)?;
        }
        if let Some(tt) = table(&t, "target") {
            for (plat, v) in tt {
                if let Some(v) = v.as_table() {
                    add(&mut deps, table(v, "dependencies"), false, false, Some(plat.clone()))?;
                    add(&mut deps, table(v, "build-dependencies").or_else(|| table(v, "build_dependencies")), true, false, Some(plat.clone()))?;
                    if !registry {
                        add(&mut deps, table(v, "dev-dependencies").or_else(|| table(v, "dev_dependencies")), false, true, Some(plat.clone()))?;
                    }
                }
            }
        }
        let i = self.manifests.len();
        self.manifests.push(Manifest { dir: dir.clone(), name, version, edition, links, build, lib, bins, tests, features, deps, proc_macro, registry });
        self.by_dir.insert(dir, i);
        Ok(i)
    }

    /// Does a dependency's platform predicate hold on the host?
    fn platform_ok(&self, p: &Option<String>) -> bool {
        let Some(p) = p else { return true };
        match p.strip_prefix("cfg(").and_then(|r| r.strip_suffix(')')) {
            Some(expr) => cfg_holds(expr, &self.cfg),
            None => p == self.cfg.triple(),
        }
    }

    /// The package directory a dependency entry names.
    fn dep_dir(&self, d: &Dep) -> Result<(String, bool), String> {
        match &d.path {
            Some(p) => Ok((p.clone(), false)),
            None => self.registry_dir(&d.package, &d.req).map(|r| (r, true)),
        }
    }

    /// The workspace member named `package` (members, and path dependencies under the root).
    fn find_member(&mut self, package: &str) -> Result<usize, String> {
        let mut queue: Vec<String> = self.ws.member_dirs();
        // fast path: a member whose manifest names the package (parsed only when the text matches)
        let needle = format!("\"{}\"", package);
        for d in &queue {
            let path = format!("{}/Cargo.toml", d);
            if crate::vfs::read_to_string(&path).is_ok_and(|t| t.contains(&needle)) {
                if let Ok(i) = self.load(d, false) {
                    if self.manifests[i].name == package {
                        return Ok(i);
                    }
                }
            }
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(d) = queue.pop() {
            let d = normalize(&d);
            if !seen.insert(d.clone()) || !crate::vfs::is_file(format!("{}/Cargo.toml", d)) {
                continue;
            }
            let Ok(i) = self.load(&d, false) else { continue };
            if self.manifests[i].name == package {
                return Ok(i);
            }
            for dep in self.manifests[i].deps.clone() {
                if let Some(p) = dep.path {
                    if p.starts_with(&format!("{}/", self.ws.root)) {
                        queue.push(p);
                    }
                }
            }
        }
        Err(format!("package `{}` is not a member of the workspace at {}", package, self.ws.root))
    }
}

/// The test programs of the workspace member `package`: whether it has a lib target, and its
/// integration test targets by name with their source paths (cargo test's order: the lib's unit
/// tests, then the test targets by name).
pub fn test_targets(sess: &std::sync::Arc<crate::Session>, dir: &str, package: &str) -> Result<(Option<String>, Vec<(String, String)>), String> {
    let mut r = Resolver::new(sess, dir)?;
    let i = r.find_member(package)?;
    let m = &r.manifests[i];
    let mut tests: Vec<(String, String)> = m.tests.iter().map(|t| (t.name.clone(), t.path.clone())).collect();
    tests.sort();
    Ok((m.lib.as_ref().map(|l| l.path.clone()), tests))
}

/// The folder of the workspace member `package` of the workspace containing `dir`.
pub fn package_dir(sess: &std::sync::Arc<crate::Session>, dir: &str, package: &str) -> Result<String, String> {
    let mut r = Resolver::new(sess, &find_root(dir)?)?;
    let i = r.find_member(package)?;
    Ok(r.manifests[i].dir.clone())
}

/// What a package asks of its run's environment: its manifest's
/// `[package.metadata.makepad.run-env]` table of `NAME = "value"` lines. The loader applies it to every run of the package, from a
/// checkout or from the stores; a variable the environment already sets wins.
pub fn run_env(manifest: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            inside = line == "[package.metadata.makepad.run-env]";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            if let Some(v) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    out
}

/// The workspace root for a start folder, as cargo finds it: the nearest folder at or above
/// `dir` with a Cargo.toml (the current manifest); then the nearest manifest at or above that
/// one with a `[workspace]` table whose members include it (or it is the root) and whose
/// excludes do not; no such workspace: the current manifest's folder is the root (a standalone
/// package).
/// crates-io's replacement directory as cargo configures it: the `.cargo/config.toml` (or
/// `.cargo/config`) files of `dir` and every folder above it, the nearest first; the first that
/// sets `[source.crates-io] replace-with = "<x>"` names the source, the first that sets
/// `[source.<x>] directory` gives its path (relative to the folder holding that `.cargo`).
/// Another replacement kind (a registry or git source) is an error: Relax reads local sources only.
fn vendor_dir(dir: &str) -> Result<Option<(String, String)>, String> {
    let mut configs: Vec<(String, TomlTable)> = Vec::new();
    let mut cur = Some(normalize(dir));
    while let Some(d) = cur {
        for f in ["config.toml", "config"] {
            let path = format!("{}/.cargo/{}", d, f);
            if crate::vfs::is_file(&path) {
                configs.push((d.clone(), read_toml(&path)?));
                break;
            }
        }
        cur = parent(&d);
    }
    let source = |n: &str| configs.iter().find_map(|(d, t)| table(t, "source").and_then(|s| table(s, n)).map(|x| (d, x)));
    let Some(with) = source("crates-io").and_then(|(_, t)| s(t, "replace-with")) else { return Ok(None) };
    let Some((d, src)) = source(&with) else {
        return Err(format!("cargo config: crates-io is replaced with `{}`, which no [source.{}] defines", with, with));
    };
    match s(src, "directory") {
        Some(p) => Ok(Some((join(d, &p), format!("{}/.cargo", d)))),
        None => Err(format!("{}/.cargo: [source.{}] is not a `directory` source: makepad run reads vendored crates only", d, with)),
    }
}

/// cargo's target directory for a start folder (what `cargo metadata` reports as
/// `target_directory`): CARGO_TARGET_DIR, else CARGO_BUILD_TARGET_DIR (relative to the current
/// folder), else `[build] target-dir` of the nearest `.cargo/config.toml` (or `.cargo/config`)
/// at or above `dir`, then of $CARGO_HOME's config (relative to the folder holding that
/// `.cargo`), else `<workspace root>/target` (find_root).
pub fn target_dir(dir: &str) -> Result<String, String> {
    for k in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
        if let Some(v) = std::env::var(k).ok().filter(|v| !v.is_empty()) {
            let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
            return Ok(join(&cwd, &v));
        }
    }
    let config_target = |d: &str, path: &str| -> Result<Option<String>, String> {
        Ok(table(&read_toml(path)?, "build").and_then(|b| s(b, "target-dir")).map(|t| join(d, &t)))
    };
    let mut cur = Some(normalize(dir));
    while let Some(d) = cur {
        for f in ["config.toml", "config"] {
            let path = format!("{}/.cargo/{}", d, f);
            if crate::vfs::is_file(&path) {
                if let Some(t) = config_target(&d, &path)? {
                    return Ok(t);
                }
                break;
            }
        }
        cur = parent(&d);
    }
    let home = std::env::var("CARGO_HOME").ok().filter(|h| !h.is_empty()).map(|h| normalize(&h)).or_else(|| {
        std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok().map(|h| format!("{}/.cargo", normalize(&h)))
    });
    if let Some(home) = home {
        for f in ["config.toml", "config"] {
            let path = format!("{}/{}", home, f);
            if crate::vfs::is_file(&path) {
                if let Some(t) = config_target(&parent(&home).unwrap_or_default(), &path)? {
                    return Ok(t);
                }
                break;
            }
        }
    }
    Ok(format!("{}/target", find_root(dir)?))
}

pub fn find_root(dir: &str) -> Result<String, String> {
    let start = normalize(dir);
    let mut cur = Some(start.clone());
    let mut current: Option<String> = None;
    while let Some(d) = cur {
        if crate::vfs::is_file(format!("{}/Cargo.toml", d)) {
            current = Some(d);
            break;
        }
        cur = parent(&d);
    }
    let current = current.ok_or_else(|| format!("no Cargo.toml in {} or any folder above it", start))?;
    let mut cur = Some(current.clone());
    while let Some(d) = cur {
        let path = format!("{}/Cargo.toml", d);
        if crate::vfs::is_file(&path) {
            if let Ok(t) = read_toml(&path) {
                if t.contains_key("workspace") {
                    if d == current {
                        return Ok(d);
                    }
                    if let Ok(ws) = Workspace::load(&d) {
                        if !ws.excluded(&current) && ws.has_member(&current) {
                            return Ok(d);
                        }
                    }
                    // a workspace that does not hold this package: keep looking above (cargo
                    // would refuse; a standalone package is what is meant)
                }
            }
        }
        cur = parent(&d);
    }
    Ok(current)
}

fn parent(d: &str) -> Option<String> {
    let t = d.trim_end_matches('/');
    let i = t.rfind('/')?;
    if i == 0 {
        if t.len() > 1 { Some("/".to_string()) } else { None }
    } else {
        Some(t[..i].to_string())
    }
}

/// `cfg(..)` predicate text against the host cfg set.
fn cfg_holds(expr: &str, cfg: &crate::cfg::CfgSet) -> bool {
    fn parse(t: &mut &str, cfg: &crate::cfg::CfgSet) -> bool {
        *t = t.trim_start();
        let end = t.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(t.len());
        let name = t[..end].to_string();
        *t = t[end..].trim_start();
        if t.starts_with('(') && (name == "all" || name == "any" || name == "not") {
            *t = &t[1..];
            let mut vals = Vec::new();
            loop {
                *t = t.trim_start();
                if t.starts_with(')') {
                    *t = &t[1..];
                    break;
                }
                if t.is_empty() {
                    break;
                }
                vals.push(parse(t, cfg));
                *t = t.trim_start();
                if t.starts_with(',') {
                    *t = &t[1..];
                }
            }
            return match name.as_str() {
                "all" => vals.iter().all(|v| *v),
                "any" => vals.iter().any(|v| *v),
                _ => vals.len() == 1 && !vals[0],
            };
        }
        if t.starts_with('=') {
            *t = t[1..].trim_start();
            let v = if let Some(r) = t.strip_prefix('"') {
                let e = r.find('"').unwrap_or(r.len());
                let v = r[..e].to_string();
                *t = &r[(e + 1).min(r.len())..];
                v
            } else {
                String::new()
            };
            return cfg.holds(&format!("{}={}", name, v));
        }
        cfg.holds(&name)
    }
    let mut t = expr;
    parse(&mut t, cfg)
}

/// One node of the feature resolution: a package on one side (host = build-dependency / proc
/// macro side).
#[derive(Default)]
struct Node {
    features: BTreeSet<String>,
    /// optional dependency keys switched on
    deps_on: BTreeSet<String>,
    /// `x?/f` and `x/f` features waiting for dependency x
    pending: Vec<(String, String)>,
    /// (dep entry index in the manifest, target node)
    edges: Vec<(usize, (usize, bool))>,
    active: bool,
}

#[derive(Debug)]
enum Req {
    Feature(String),
    Dep(String),
    DepFeature(String, String, bool),
}

pub struct Options {
    pub no_default_features: bool,
    pub all_features: bool,
    pub features: Vec<String>,
    pub bin: Option<String>,
    /// `--lib`, `--tests`, `--test <name>` (cargo test's target selection): the graph of the
    /// package's test harness. The lib's unit tests and the integration tests are separate
    /// programs (the lib is cfg(test) in the first and plain in the second), so one graph holds
    /// either the lib test or the integration tests.
    pub test: Option<TestSel>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TestSel {
    /// the lib target built with `--test`
    Lib,
    /// these integration test targets (empty: all of them)
    Integration(Vec<String>),
}

pub fn parse_flags(flags: &[String]) -> Result<Options, String> {
    let mut o = Options { no_default_features: false, all_features: false, features: Vec::new(), bin: None, test: None };
    let mut lib = false;
    let mut tests: Option<Vec<String>> = None;
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            flags.get(*i).cloned().ok_or_else(|| format!("{} needs a value", f))
        };
        match f {
            "--no-default-features" => o.no_default_features = true,
            "--all-features" => o.all_features = true,
            "--features" | "-F" => {
                let v = value(&mut i)?;
                o.features.extend(v.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string));
            }
            "--bin" => o.bin = Some(value(&mut i)?),
            "--lib" => lib = true,
            "--tests" => {
                tests.get_or_insert_with(Vec::new);
            }
            "--test" => {
                let v = value(&mut i)?;
                tests.get_or_insert_with(Vec::new).push(v);
            }
            "--release" => {}
            _ => {
                if let Some(v) = f.strip_prefix("--features=") {
                    o.features.extend(v.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string));
                } else if let Some(v) = f.strip_prefix("--bin=") {
                    o.bin = Some(v.to_string());
                } else if let Some(v) = f.strip_prefix("--test=") {
                    tests.get_or_insert_with(Vec::new).push(v.to_string());
                } else {
                    return Err(format!("unit graph: flag `{}` is not supported by makepad run's resolver", f));
                }
            }
        }
        i += 1;
    }
    o.test = match (lib, tests) {
        (true, Some(_)) => return Err("unit graph: --lib and integration tests are separate test programs; select one".to_string()),
        (true, None) => Some(TestSel::Lib),
        (false, Some(t)) => Some(TestSel::Integration(t)),
        (false, None) => None,
    };
    Ok(o)
}

struct Feat<'a> {
    r: &'a mut Resolver,
    /// the node whose dev-dependencies are part of the graph (the root of a test graph)
    dev_of: Option<(usize, bool)>,
    nodes: HashMap<(usize, bool), Node>,
    work: Vec<((usize, bool), Req)>,
    order: Vec<(usize, bool)>,
}

impl<'a> Feat<'a> {
    fn node(&mut self, k: (usize, bool)) -> &mut Node {
        self.nodes.entry(k).or_default()
    }

    /// Activates a node: its non-optional dependencies (for this platform) are followed.
    fn activate(&mut self, k: (usize, bool)) -> Result<(), String> {
        if self.node(k).active {
            return Ok(());
        }
        self.node(k).active = true;
        self.order.push(k);
        let deps = self.r.manifests[k.0].deps.clone();
        for (di, d) in deps.iter().enumerate() {
            if !d.optional && (!d.dev || self.dev_of == Some(k)) {
                self.follow(k, di, d)?;
            }
        }
        Ok(())
    }

    /// Follows dependency entry `di` of node `k`: the target node, its requested features.
    fn follow(&mut self, k: (usize, bool), di: usize, d: &Dep) -> Result<(), String> {
        if !self.r.platform_ok(&d.platform) {
            return Ok(());
        }
        if self.node(k).edges.iter().any(|e| e.0 == di) {
            return Ok(());
        }
        // a registry package that compiles C or C++ (Relax runs no C compiler): decided here, on a
        // build-dependency the feature resolution switched on, not on the manifest's list -- an
        // optional `cc` behind a feature that is off (objc-sys's `unstable-exception`, rav1e's and
        // av-scenechange's `asm`, signal-hook's `cc`) builds no C; and before the dependency is
        // looked up (a vendored tree need not hold `cc` to say so)
        if d.build && self.r.manifests[k.0].registry && !self.r.allow_c {
            if let Some(c) = crate::registry::c_build_dep(std::slice::from_ref(&d.package)) {
                let m = &self.r.manifests[k.0];
                return Err(format!("registry package `{}` {} compiles C or C++ (build-dependency `{}`): it {}", m.name, m.version, c, relax_session::options::C_GATE));
            }
        }
        let (dir, registry) = self.r.dep_dir(d)?;
        let t = self.r.load(&dir, registry)?;
        if self.r.manifests[t].name != d.package {
            return Err(format!("{}/Cargo.toml: dependency `{}` names package `{}`, but {} holds `{}`", self.r.manifests[k.0].dir, d.key, d.package, dir, self.r.manifests[t].name));
        }
        let host = k.1 || d.build || self.r.manifests[t].proc_macro;
        let tk = (t, host);
        self.node(k).edges.push((di, tk));
        self.activate(tk)?;
        if d.default_features {
            self.work.push((tk, Req::Feature("default".to_string())));
        }
        for f in &d.features {
            self.work.push((tk, Req::Feature(f.clone())));
        }
        // features asked of this dependency before it was on
        let key = d.key.clone();
        let pend: Vec<(String, String)> = self.node(k).pending.iter().filter(|p| p.0 == key).cloned().collect();
        for (_, f) in pend {
            self.work.push((tk, Req::Feature(f)));
        }
        Ok(())
    }

    fn switch_dep(&mut self, k: (usize, bool), key: &str) -> Result<(), String> {
        if !self.node(k).deps_on.insert(key.to_string()) {
            return Ok(());
        }
        let deps = self.r.manifests[k.0].deps.clone();
        for (di, d) in deps.iter().enumerate() {
            if d.key == key && d.optional {
                self.follow(k, di, d)?;
            }
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        while let Some((k, req)) = self.work.pop() {
            let m = &self.r.manifests[k.0];
            let mname = m.name.clone();
            let fvals = if let Req::Feature(f) = &req { m.features.get(f).cloned() } else { None };
            let (opt, syntax, has) = match &req {
                Req::Feature(x) | Req::Dep(x) | Req::DepFeature(x, _, _) => (m.is_optional_dep(x), m.dep_syntax(x), m.has_dep(x)),
            };
            match req {
                Req::Feature(f) => {
                    if self.nodes.get(&k).is_some_and(|n| n.features.contains(&f)) {
                        continue;
                    }
                    if let Some(vals) = fvals {
                        self.node(k).features.insert(f);
                        for v in vals {
                            let r = if let Some(d) = v.strip_prefix("dep:") {
                                Req::Dep(d.to_string())
                            } else if let Some((d, f2)) = v.split_once("?/") {
                                Req::DepFeature(d.to_string(), f2.to_string(), true)
                            } else if let Some((d, f2)) = v.split_once('/') {
                                Req::DepFeature(d.to_string(), f2.to_string(), false)
                            } else {
                                Req::Feature(v)
                            };
                            self.work.push((k, r));
                        }
                    } else if opt && !syntax {
                        // the implicit feature of an optional dependency
                        self.node(k).features.insert(f.clone());
                        self.work.push((k, Req::Dep(f)));
                    } else if f != "default" {
                        return Err(format!("package `{}` has no feature `{}`", mname, f));
                    }
                }
                Req::Dep(d) => self.switch_dep(k, &d)?,
                Req::DepFeature(d, f, weak) => {
                    if !has {
                        // `x/f` where x is a feature, not a dependency: cargo reads it as a dependency only
                        return Err(format!("package `{}`: feature value `{}/{}` names no dependency", mname, d, f));
                    }
                    if !weak && opt {
                        if !syntax {
                            self.work.push((k, Req::Feature(d.clone())));
                        }
                        self.switch_dep(k, &d)?;
                    }
                    self.node(k).pending.push((d.clone(), f.clone()));
                    let edges = self.node(k).edges.clone();
                    let targets: Vec<(usize, bool)> = edges.iter().filter(|e| self.r.manifests[k.0].deps[e.0].key == d).map(|e| e.1).collect();
                    for tk in targets {
                        self.work.push((tk, Req::Feature(f.clone())));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One unit of a unit graph: the fields of cargo's `--unit-graph` JSON unit that Relax reads.
#[derive(Clone, Debug, Default)]
pub struct GraphUnit {
    pub pkg_id: String,
    /// target kinds (lib, rlib, proc-macro, bin, custom-build)
    pub kind: Vec<String>,
    pub name: String,
    pub src_path: String,
    pub edition: String,
    /// a host-side unit (build-dependency / proc-macro side: cargo's opt-level 0 copy)
    pub host: bool,
    /// build | run-custom-build
    pub mode: String,
    pub features: Vec<String>,
    /// (unit index, extern crate name)
    pub deps: Vec<(usize, String)>,
}

/// A unit graph: its units and the indexes of its roots.
#[derive(Clone, Debug, Default)]
pub struct Units {
    pub units: Vec<GraphUnit>,
    pub roots: Vec<usize>,
}

impl Units {
    /// A unit graph from cargo's `--unit-graph` JSON (a build script's child gets its units so).
    pub fn from_json(data: &[u8]) -> Result<Units, String> {
        let j = crate::json::parse(data)?;
        let units = match j.get("units") {
            Some(u) => u.arr(),
            None => return Err("unit graph: no units".to_string()),
        };
        let s = |x: Option<&crate::json::Json>| x.map(|v| v.str().to_string()).unwrap_or_default();
        let list = |x: Option<&crate::json::Json>| x.map(|v| v.arr().iter().map(|s| s.str().to_string()).collect()).unwrap_or_default();
        let units = units
            .iter()
            .map(|u| {
                let t = u.get("target");
                GraphUnit {
                    pkg_id: s(u.get("pkg_id")),
                    kind: list(t.and_then(|t| t.get("kind"))),
                    name: s(t.and_then(|t| t.get("name"))),
                    src_path: s(t.and_then(|t| t.get("src_path"))),
                    edition: s(t.and_then(|t| t.get("edition"))),
                    host: u.get("profile").and_then(|p| p.get("opt_level")).map(|o| o.str() == "0").unwrap_or(false) || matches!(u.get("relax_host"), Some(crate::json::Json::Bool(true))),
                    mode: s(u.get("mode")),
                    features: list(u.get("features")),
                    deps: u.get("dependencies").map(|d| d.arr().iter().filter_map(|dep| Some((dep.get("index")?.num() as usize, s(dep.get("extern_crate_name"))))).collect()).unwrap_or_default(),
                }
            })
            .collect();
        let roots = j.get("roots").map(|r| r.arr().iter().map(|x| x.num() as usize).collect()).unwrap_or_default();
        Ok(Units { units, roots })
    }
}

/// cargo's CARGO_PKG_* environment of the package at `dir` beyond its name and version, from the
/// manifest's `[package]` (a `key.workspace = true` field from the workspace root's
/// `[workspace.package]`): authors (joined by `:`), description, homepage, repository, license,
/// license-file, rust-version and readme; the empty string for a field the manifest lacks, as cargo
/// gives them. Build scripts and crates read them (the `built` crate requires every one).
pub fn package_meta_env(dir: &str) -> Vec<(String, String)> {
    const KEYS: [(&str, &str); 8] = [
        ("CARGO_PKG_AUTHORS", "authors"),
        ("CARGO_PKG_DESCRIPTION", "description"),
        ("CARGO_PKG_HOMEPAGE", "homepage"),
        ("CARGO_PKG_REPOSITORY", "repository"),
        ("CARGO_PKG_LICENSE", "license"),
        ("CARGO_PKG_LICENSE_FILE", "license-file"),
        ("CARGO_PKG_RUST_VERSION", "rust-version"),
        ("CARGO_PKG_README", "readme"),
    ];
    let pkg = read_toml(&format!("{}/Cargo.toml", dir)).ok().and_then(|t| table(&t, "package").cloned()).unwrap_or_default();
    // the workspace root's [workspace.package], read when a field inherits from it
    let mut ws: Option<TomlTable> = None;
    let mut out = Vec::with_capacity(KEYS.len());
    for (env, key) in KEYS {
        let mut v = pkg.get(key).cloned();
        if let Some(Toml::Table(x)) = &v {
            if b(x, "workspace") == Some(true) {
                let w = ws.get_or_insert_with(|| {
                    find_root(dir).ok().and_then(|r| read_toml(&format!("{}/Cargo.toml", r)).ok()).and_then(|t| table(&t, "workspace").and_then(|w| table(w, "package")).cloned()).unwrap_or_default()
                });
                v = w.get(key).cloned();
            }
        }
        let text = match v {
            Some(Toml::Str(x, _)) => x,
            Some(Toml::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(":"),
            // `readme = true` / `false`, a number: no text to give
            _ => String::new(),
        };
        out.push((env.to_string(), text));
    }
    out
}

/// The unit graph of `package` in the workspace at `workspace` (see the module comment).
pub fn unit_graph(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, flags: &[String]) -> Result<Units, String> {
    let o = parse_flags(flags)?;
    let mut r = Resolver::new(sess, workspace)?;
    let root = r.find_member(package)?;
    let rk = (root, false);
    let mut f = Feat { r: &mut r, dev_of: o.test.as_ref().map(|_| rk), nodes: HashMap::new(), work: Vec::new(), order: Vec::new() };
    f.activate(rk)?;
    if !o.no_default_features {
        f.work.push((rk, Req::Feature("default".to_string())));
    }
    if o.all_features {
        let m = &f.r.manifests[root];
        let mut all: Vec<String> = m.features.keys().cloned().collect();
        for d in &m.deps {
            if d.optional && !m.dep_syntax(&d.key) {
                all.push(d.key.clone());
            }
        }
        for x in all {
            f.work.push((rk, Req::Feature(x)));
        }
    }
    for x in &o.features {
        let req = match x.split_once('/') {
            Some((d, ft)) => Req::DepFeature(d.to_string(), ft.to_string(), false),
            None => Req::Feature(x.clone()),
        };
        f.work.push((rk, req));
    }
    f.run()?;
    let (nodes, order) = (std::mem::take(&mut f.nodes), std::mem::take(&mut f.order));
    emit(&r, &nodes, &order, root, &o)
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum UKind {
    Lib,
    /// the root's lib target built with `--test` (its unit tests)
    LibTest,
    Bin(usize),
    /// the root's integration test target
    Test(usize),
    BuildCompile,
    BuildRun,
}

fn emit(r: &Resolver, nodes: &HashMap<(usize, bool), Node>, order: &[(usize, bool)], root: usize, o: &Options) -> Result<Units, String> {
    // units: (node, kind) -> index
    let mut index: HashMap<((usize, bool), UKind), usize> = HashMap::new();
    let mut units: Vec<((usize, bool), UKind)> = Vec::new();
    let mut add = |k: (usize, bool), u: UKind, units: &mut Vec<((usize, bool), UKind)>| -> usize {
        *index.entry((k, u.clone())).or_insert_with(|| {
            units.push((k, u));
            units.len() - 1
        })
    };
    // every active node with a lib gets a lib unit; the root's bins; build scripts
    let mut roots = Vec::new();
    for &k in order {
        let m = &r.manifests[k.0];
        if m.lib.is_some() {
            if k == (root, false) && o.test == Some(TestSel::Lib) {
                roots.push(add(k, UKind::LibTest, &mut units));
            } else {
                let i = add(k, UKind::Lib, &mut units);
                if k == (root, false) && o.test.is_none() {
                    roots.push(i);
                }
            }
        }
        if m.build.is_some() && (m.lib.is_some() || k.0 == root) {
            add(k, UKind::BuildCompile, &mut units);
            add(k, UKind::BuildRun, &mut units);
        }
    }
    let rn = &nodes[&(root, false)];
    let rm = &r.manifests[root];
    match &o.test {
        Some(TestSel::Lib) if rm.lib.is_none() => return Err(format!("no library targets found in package `{}`", rm.name)),
        Some(TestSel::Integration(names)) => {
            for n in names {
                if !rm.tests.iter().any(|t| &t.name == n) {
                    return Err(format!("no test target named `{}` in package `{}`", n, rm.name));
                }
            }
            for (ti, tt) in rm.tests.iter().enumerate() {
                if !names.is_empty() && !names.contains(&tt.name) {
                    continue;
                }
                if !tt.required_features.iter().all(|f| rn.features.contains(f)) {
                    continue;
                }
                if !tt.harness {
                    return Err(format!("test target `{}` of package `{}` has `harness = false`: makepad test runs libtest-harness targets only", tt.name, rm.name));
                }
                roots.push(add((root, false), UKind::Test(ti), &mut units));
            }
        }
        _ => {}
    }
    for (bi, bt) in r.manifests[root].bins.iter().enumerate() {
        if o.test.is_some() {
            break;
        }
        if o.bin.as_ref().is_some_and(|b| b != &bt.name) {
            continue;
        }
        if bt.required_features.iter().all(|f| rn.features.contains(f)) {
            roots.push(add((root, false), UKind::Bin(bi), &mut units));
        }
    }
    if let Some(b) = &o.bin {
        if !r.manifests[root].bins.iter().any(|t| &t.name == b) {
            return Err(format!("no bin target named `{}` in package `{}`", b, r.manifests[root].name));
        }
    }
    let lib_name = |t: usize| r.manifests[t].lib.as_ref().map(|l| l.name.replace('-', "_")).unwrap_or_default();
    let extern_name = |d: &Dep, t: usize| if d.package != d.key { d.key.replace('-', "_") } else { lib_name(t) };
    let mut out = Vec::with_capacity(units.len());
    for (k, u) in &units {
        let m = &r.manifests[k.0];
        let node = &nodes[k];
        let host = k.1 || matches!(u, UKind::BuildCompile);
        let (kind, name, path, edition, mode): (Vec<String>, String, String, String, &str) = match u {
            UKind::Lib | UKind::LibTest => {
                let l = m.lib.as_ref().unwrap();
                (l.kind.clone(), l.name.clone(), l.path.clone(), l.edition.clone(), if matches!(u, UKind::LibTest) { "test" } else { "build" })
            }
            UKind::Test(ti) => {
                let t = &m.tests[*ti];
                (t.kind.clone(), t.name.clone(), t.path.clone(), t.edition.clone(), "test")
            }
            UKind::Bin(bi) => {
                let b = &m.bins[*bi];
                (b.kind.clone(), b.name.clone(), b.path.clone(), b.edition.clone(), "build")
            }
            UKind::BuildCompile | UKind::BuildRun => {
                let p = m.build.clone().unwrap();
                let stem = p.rsplit('/').next().unwrap_or("build.rs").trim_end_matches(".rs").to_string();
                let stem = if stem == "build" { "build-script-build".to_string() } else { format!("build-script-{}", stem) };
                (vec!["custom-build".to_string()], stem, p, m.edition.clone(), if matches!(u, UKind::BuildRun) { "run-custom-build" } else { "build" })
            }
        };
        // dependencies
        let mut deps: Vec<(usize, String)> = Vec::new();
        // dev-dependencies: only the test targets' (the plain lib of a test graph has none)
        let dev = matches!(u, UKind::LibTest | UKind::Test(_));
        let normal_edges = |deps: &mut Vec<(usize, String)>, build: bool| {
            for (di, tk) in &node.edges {
                let d = &m.deps[*di];
                if d.build != build || (d.dev && !dev) || r.manifests[tk.0].lib.is_none() {
                    continue;
                }
                if let Some(&ti) = index.get(&(*tk, UKind::Lib)) {
                    let e = (ti, extern_name(d, tk.0));
                    if !deps.contains(&e) {
                        deps.push(e);
                    }
                }
            }
        };
        match u {
            UKind::Lib | UKind::LibTest => {
                normal_edges(&mut deps, false);
                if let Some(&bi) = index.get(&(*k, UKind::BuildRun)) {
                    deps.push((bi, "build_script_build".to_string()));
                }
            }
            UKind::Bin(_) | UKind::Test(_) => {
                if let Some(&li) = index.get(&(*k, UKind::Lib)) {
                    deps.push((li, lib_name(k.0)));
                }
                normal_edges(&mut deps, false);
                if let Some(&bi) = index.get(&(*k, UKind::BuildRun)) {
                    deps.push((bi, "build_script_build".to_string()));
                }
            }
            UKind::BuildCompile => normal_edges(&mut deps, true),
            UKind::BuildRun => {
                if let Some(&ci) = index.get(&(*k, UKind::BuildCompile)) {
                    deps.push((ci, "build_script_build".to_string()));
                }
                // the run units of dependencies with `links` (their DEP_* variables)
                for (di, tk) in &node.edges {
                    if m.deps[*di].build || r.manifests[tk.0].links.is_none() {
                        continue;
                    }
                    if let Some(&ri) = index.get(&(*tk, UKind::BuildRun)) {
                        if !deps.iter().any(|d| d.0 == ri) {
                            deps.push((ri, "build_script_build".to_string()));
                        }
                    }
                }
            }
        }
        out.push(GraphUnit {
            pkg_id: m.pkg_id(),
            kind,
            name,
            // the target's source as cargo spells it: the OS separator (Windows `C:\..\src\lib.rs`),
            // so file!(), panic locations and diagnostics read as under rustc
            src_path: if cfg!(windows) { path.replace('/', "\\") } else { path },
            edition,
            host,
            mode: mode.to_string(),
            features: node.features.iter().cloned().collect(),
            deps,
        });
    }
    Ok(Units { units: out, roots })
}

#[cfg(test)]
mod root_tests {
    use super::*;

    fn tree(name: &str, files: &[(&str, &str)]) -> String {
        let root = std::env::temp_dir().join(format!("relax-resolve-{}-{}", name, std::process::id()));
        for (p, text) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
        root.display().to_string()
    }

    /// the names of the package's units
    fn units(sess: &std::sync::Arc<crate::Session>, dir: &str, pkg: &str) -> Vec<String> {
        unit_graph(sess, dir, pkg, &[]).unwrap_or_else(|e| panic!("{dir} {pkg}: {e}")).units.into_iter().map(|u| u.name).collect()
    }

    #[test]
    fn standalone_and_member_packages() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree(
            "ws",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"app\", \"lib\"]\nexclude = [\"bench\"]\n"),
                ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nlib = { path = \"../lib\" }\n"),
                ("app/src/main.rs", "fn main() {}\n"),
                ("lib/Cargo.toml", "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
                ("lib/src/lib.rs", ""),
                // a package with its own (empty) workspace, like bench_rt
                ("bench/Cargo.toml", "[package]\nname = \"bench\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[[bin]]\nname = \"bench\"\npath = \"src/main.rs\"\n[dependencies]\nlib = { path = \"../lib\" }\n[workspace]\n"),
                ("bench/src/main.rs", "fn main() {}\n"),
                // a package with no workspace at all, outside the workspace's members
                ("solo/Cargo.toml", "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
                ("solo/src/main.rs", "fn main() {}\n"),
                // a members package naming a crate only as a never-built path dependency (apps/commercial
                // and Sandbox): cargo makes that crate a member, so the workspace's inherited deps apply
                ("hub/Cargo.toml", "[package]\nname = \"hub\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[target.'cfg(any())'.dependencies]\ngame = { path = \"game\" }\n"),
                ("hub/src/lib.rs", ""),
                ("hub/game/Cargo.toml", "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nlib = { workspace = true }\n"),
                ("hub/game/src/main.rs", "fn main() {}\n"),
            ],
        );
        let _ = std::fs::write(format!("{root}/Cargo.toml"), "[workspace]\nmembers = [\"app\", \"lib\", \"hub\"]\nexclude = [\"bench\"]\n[workspace.dependencies]\nlib = { path = \"lib\" }\n");
        assert_eq!(find_root(&format!("{root}/hub/game")).unwrap(), normalize(&root));
        assert!(units(sess, &format!("{root}/hub/game"), "game").iter().any(|n| n == "lib"));
        // from the workspace root and from inside a member's folder
        assert!(units(sess, &root, "app").iter().any(|n| n == "app"));
        assert_eq!(find_root(&format!("{root}/app/src")).unwrap(), normalize(&root));
        assert!(units(sess, &format!("{root}/app/src"), "app").iter().any(|n| n == "app"));
        // a standalone package with its own [workspace] (the root package is a member)
        assert_eq!(find_root(&format!("{root}/bench")).unwrap(), normalize(&format!("{root}/bench")));
        let b = units(sess, &format!("{root}/bench"), "bench");
        assert!(b.iter().any(|n| n == "bench") && b.iter().any(|n| n == "lib"));
        // a package without a workspace table that no workspace above holds
        assert_eq!(find_root(&format!("{root}/solo")).unwrap(), normalize(&format!("{root}/solo")));
        assert!(units(sess, &format!("{root}/solo"), "solo").iter().any(|n| n == "solo"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
#[test]
fn run_env_reads_the_packages_table() {
    let m = "[package]\nname = \"makepad-amp\"\n\n[package.metadata.makepad.run-env]\n# why\nMAKEPAD_LOG = \"0\"\nbad key = \"x\"\n\n[dependencies]\nX = \"1\"\n";
    assert_eq!(run_env(m), vec![("MAKEPAD_LOG".to_string(), "0".to_string())]);
    assert!(run_env("[package]\nname = \"a\"\n").is_empty());
}
