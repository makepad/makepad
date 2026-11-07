//! Relax's own crate resolution: the unit graph of one package as `cargo build --release -p <pkg>
//! --unit-graph` gives it for the host target, computed from the manifests (Cargo.toml of the
//! workspace and of every package reached, Cargo.lock for registry packages), without running
//! cargo. The output is cargo's unit graph (the fields of its JSON that unitgraph::load reads),
//! handed over in memory (`Units`; a build script's child process reads it as that JSON). It was
//! checked equal to cargo's, unit by unit, for every app on macOS, Linux and Windows before
//! cargo's path was deleted (2026-10-06).
//!
//! Rules followed (cargo, resolver 2; https://doc.rust-lang.org/cargo/reference/):
//! - packages: path dependencies (relative to the manifest), git dependencies (below),
//!   `[patch.crates-io]` and `[patch."<git url>"]` of the workspace root (path or git
//!   replacements), registry packages from a vendored directory when `.cargo/config.toml`
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
//!   `required-features` are off are skipped; `[[example]]` plus discovered examples
//!   (examples/*.rs, examples/*/main.rs) unless `autoexamples = false`, one of which `--example`
//!   runs as a bin with the package's dev-dependencies (as `cargo run --example` builds it); build scripts (`build = ..` or build.rs): a compile
//!   unit on the host side (deps = build-dependencies) and a run unit that the lib depends on
//!   (deps = the compile unit and the run units of dependencies with `links`).
//! - registry packages neither vendored nor in $CARGO_HOME: fetched from crates.io (registry.rs:
//!   sparse index, SHA-256, a cache under the Relax cache root, offline-first); with Cargo.lock its
//!   version, without, the newest the requirement accepts (one version per package);
//! - git dependencies (`git = <url>` with `branch`, `tag` or `rev`, else the default branch; also
//!   from `[workspace.dependencies]`, in dev-dependencies, in git packages' own manifests and as
//!   `[patch]` replacements): a completed Cargo checkout at the locked commit is read directly;
//!   otherwise Cargo's bare db is imported, or Relax fetches it into its ONE store file
//!   (git/fetch.rs, gitstore.rs: `<run home>/cache`, shared with `makepad run <git url>`) and
//!   mounted read-only on the VFS at `<run home>/vfs/git/<name>-<url hash>/<commit>` (never
//!   checked out). The commit: Cargo.lock's `git+<url>?<ref>#<commit>` source when it lists the
//!   source (the lockfile wins); else the pin Relax recorded in `<relax dir>/git.lock` (the user's
//!   Cargo.lock is never written); else the commit the remote's ref names now, recorded there.
//!   Offline (Cargo's env/config, --offline/--frozen, MAKEPAD_RUN_OFFLINE or no answer), a pinned
//!   commit must be local; an unpinned ref uses the store's newest fetch of it. The package is
//!   found in the repository as cargo does it: every Cargo.toml of the tree (not under hidden
//!   folders, a package's `target` folder or a nested repository), the root first, the first
//!   `[package]` of that name; `workspace = true` fields inherit from the repository's own
//!   workspace; a path dependency inside the repository is part of the same git source. Package
//!   ids are cargo's `git+<url>[?<ref>]#<name>@<version>`.
//! Unsupported (an error naming it): ssh and file git URLs, registry packages missing from the vendored
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
    /// a path package directory, or None for a registry or git package
    path: Option<String>,
    /// a git package's source
    git: Option<GitSrc>,
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
    /// `test = false`: `cargo test` leaves it out unless it is named
    test: bool,
    /// `doctest = false` (a lib): `cargo test` runs none of its doc tests
    doctest: bool,
}

impl Target {
    fn enabled(&self, features: &BTreeSet<String>, explicit: bool) -> Result<bool, String> {
        if self.required_features.iter().all(|f| features.contains(f)) {
            return Ok(true);
        }
        if explicit {
            return Err(format!("target `{}` requires the features: {}\nConsider enabling them by passing, e.g., `--features=\"{}\"`",
                self.name, self.required_features.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "), self.required_features.join(" ")));
        }
        Ok(false)
    }
}

/// A git dependency's source: its repository (as the manifest spells the URL) and reference.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GitSrc {
    url: String,
    reference: crate::git::fetch::GitRef,
}

impl GitSrc {
    /// cargo's source id without the commit, `git+<url>[?<ref query>]` (Cargo.lock's spelling
    /// before `#<commit>`; the key of relax's own pins).
    fn id(&self) -> String {
        match self.reference.query() {
            Some(q) => format!("git+{}?{}", self.url, q),
            None => format!("git+{}", self.url),
        }
    }
    /// Same repository and reference as `o` (cargo compares canonical URLs).
    fn same(&self, o: &GitSrc) -> bool {
        self.reference == o.reference && canonical_url(&self.url) == canonical_url(&o.url)
    }
}

/// cargo's CanonicalUrl: no trailing `/`, no `.git` suffix; github.com's paths in lower case.
fn canonical_url(url: &str) -> String {
    let mut u = url.trim_end_matches('/').to_string();
    if let Some(rest) = u.strip_prefix("https://github.com/").or_else(|| u.strip_prefix("http://github.com/")) {
        u = format!("https://github.com/{}", rest.to_ascii_lowercase());
    }
    if let Some(x) = u.strip_suffix(".git") {
        u = x.to_string();
    }
    u
}

/// `%XX` escapes and `+` decoded (a Cargo.lock query value is form-urlencoded).
fn url_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: &[u8]| std::str::from_utf8(x).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 3 <= b.len() && hex(&b[i + 1..i + 3]).is_some() => {
                out.push(hex(&b[i + 1..i + 3]).unwrap());
                i += 3;
                continue;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A git source of Cargo.lock, `git+<url>[?branch=|tag=|rev=<x>]#<commit>`: the source and commit.
fn parse_git_source(src: &str) -> Option<(GitSrc, crate::gitstore::Oid)> {
    use crate::git::fetch::GitRef;
    let rest = src.strip_prefix("git+")?;
    let (url_q, commit) = rest.rsplit_once('#')?;
    let commit = crate::gitstore::from_hex(&commit.to_ascii_lowercase())?;
    let (url, query) = match url_q.split_once('?') {
        Some((u, q)) => (u, Some(q)),
        None => (url_q, None),
    };
    let mut reference = GitRef::DefaultBranch;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = url_decode(v);
        match k {
            "branch" => reference = GitRef::Branch(v),
            "tag" => reference = GitRef::Tag(v),
            "rev" => reference = GitRef::Rev(v),
            _ => {}
        }
    }
    Some((GitSrc { url: url.to_string(), reference }, commit))
}

/// A dependency entry's git source: `git` with at most one of `branch`, `tag`, `rev`.
/// `at`: the file holding the entry (errors name it).
fn git_src(st: &TomlTable, at: &str, key: &str) -> Result<Option<GitSrc>, String> {
    use crate::git::fetch::GitRef;
    let Some(url) = s(st, "git") else { return Ok(None) };
    if st.contains_key("path") {
        return Err(format!("{}: dependency ({}) specification is ambiguous. Only one of `git` or `path` is allowed.", at, key));
    }
    let mut refs: Vec<GitRef> = Vec::new();
    if let Some(b) = s(st, "branch") {
        refs.push(GitRef::Branch(b));
    }
    if let Some(t) = s(st, "tag") {
        refs.push(GitRef::Tag(t));
    }
    if let Some(r) = s(st, "rev") {
        refs.push(GitRef::Rev(r));
    }
    if refs.len() > 1 {
        return Err(format!("{}: dependency ({}) specification is ambiguous. Only one of `branch`, `tag` or `rev` is allowed.", at, key));
    }
    if crate::git::fetch::check_url(&url).is_err() {
        return Err(format!("{}: `{}` is a git dependency on {}: makepad run fetches git repositories over https only", at, key, url));
    }
    Ok(Some(GitSrc { url, reference: refs.pop().unwrap_or(GitRef::DefaultBranch) }))
}

/// Where a package comes from (its package id's kind).
#[derive(Clone, Debug, PartialEq)]
enum Source {
    Path,
    Registry,
    Git(GitSrc),
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
    /// examples: `[[example]]` plus discovered examples/*.rs, examples/*/main.rs
    examples: Vec<Target>,
    features: BTreeMap<String, Vec<String>>,
    deps: Vec<Dep>,
    /// the keys of a registry package's dev-dependencies, which are not read (never built): a
    /// feature value `x/f` naming one enables nothing, as under cargo (features.rs
    /// activate_dep_feature finds no activated dependency `x`)
    dev_keys: Vec<String>,
    proc_macro: bool,
    source: Source,
}

impl Manifest {
    fn pkg_id(&self) -> String {
        match &self.source {
            Source::Registry => return format!("registry+https://github.com/rust-lang/crates.io-index#{}@{}", self.name, self.version),
            Source::Git(g) => return format!("{}#{}@{}", g.id(), self.name, self.version),
            Source::Path => {}
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
    fn implicit_features(&self) -> BTreeSet<String> {
        let hidden: BTreeSet<&str> = self.features.values().flatten().filter_map(|v| v.strip_prefix("dep:")).collect();
        self.deps.iter().filter(|d| d.optional && !hidden.contains(d.key.as_str())).map(|d| d.key.clone()).collect()
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

/// A `[patch]` replacement: a path package or a git source.
#[derive(Clone, Debug)]
enum Patch {
    Path(String),
    Git(GitSrc),
}

/// The workspace: root manifest dir, `[workspace.dependencies]`, `[workspace.package]`,
/// `[patch.crates-io]` / `[patch."<git url>"]`, Cargo.lock's registry versions and git commits.
struct Workspace {
    root: String,
    ws_deps: TomlTable,
    ws_package: TomlTable,
    /// (`crates-io` or the patched repository's canonical URL, package name) -> replacement
    patches: HashMap<(String, String), Patch>,
    lock: HashMap<String, Vec<String>>,
    /// Cargo.lock's git sources with their commits
    lock_git: Vec<(GitSrc, crate::gitstore::Oid)>,
    members: Vec<String>,
    excludes: Vec<String>,
    /// the root manifest has a `[package]`: cargo makes the root package a member
    root_package: bool,
}

/// Where git dependencies live: the stores and their mounts (`home`: `<home>/cache`, `<home>/vfs/git`)
/// and relax's own pins for sources Cargo.lock does not list (`lock`; None: `<relax dir>/git.lock`,
/// computed when first needed).
pub struct GitPlaces {
    pub home: std::path::PathBuf,
    pub lock: Option<std::path::PathBuf>,
}

impl GitPlaces {
    /// A run's: the run home (gitstore::relax_home) and the workspace's relax dir.
    pub fn run() -> GitPlaces {
        GitPlaces { home: crate::gitstore::relax_home(), lock: None }
    }
}

/// The git sources a resolution reached.
struct GitDeps {
    places: GitPlaces,
    /// relax's pins (source id -> commit), read when first needed
    pins: Option<BTreeMap<String, String>>,
    /// mounted sources: (source, mount root, commit, the folders holding a Cargo.toml in cargo's
    /// discovery order)
    mounts: Vec<(GitSrc, String, crate::gitstore::Oid, Vec<String>)>,
    /// (source, package) -> the package's folder
    found: HashMap<(GitSrc, String), String>,
}

pub struct Resolver {
    ws: std::rc::Rc<Workspace>,
    git: std::cell::RefCell<GitDeps>,
    offline: bool,
    /// the workspaces that path packages outside `ws` (or excluded from it) inherit from: cargo
    /// resolves `workspace = true` against the package's OWN workspace root (find_root), not the
    /// root of the graph being built; by root dir
    owners: HashMap<String, std::rc::Rc<Workspace>>,
    /// crates-io's replacement (a vendored directory or a git repository) and the config file
    /// that names it
    vendor: Option<(Vendor, String)>,
    /// registry packages fetched for a program with neither a vendored directory nor a locked
    /// version: name -> (version, sources, the requirements that chose it)
    fetched: std::cell::RefCell<HashMap<String, (String, String, Vec<String>)>>,
    manifests: Vec<Manifest>,
    by_dir: HashMap<String, usize>,
    cfg: crate::cfg::CfgSet,
    /// `--allow-c`: registry packages that compile C are accepted (cargo builds them, native.rs)
    allow_c: bool,
    /// Metadata describes packages without compiling them: keep their declared dev dependencies
    /// and unify host/target features into the one node cargo metadata exposes per package.
    metadata: bool,
}

fn read_toml(path: &str) -> Result<TomlTable, String> {
    let text = crate::vfs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    parse_toml(&text).map(|d| d.root).map_err(|e| e.in_file(path))
}

impl Workspace {
    fn load(root: &str) -> Result<Workspace, String> {
        let root = normalize(root);
        let t = read_toml(&format!("{}/Cargo.toml", root))?;
        let w = table(&t, "workspace").cloned().unwrap_or_default();
        let mut patches = HashMap::new();
        if let Some(p) = table(&t, "patch") {
            for (src, entries) in p {
                // crates-io or a git repository's URL (another registry's name: not resolved here)
                let skey = if src == "crates-io" {
                    src.clone()
                } else if src.contains("://") {
                    canonical_url(src)
                } else {
                    continue;
                };
                let Some(entries) = entries.as_table() else { continue };
                for (k, v) in entries {
                    let Some(v) = v.as_table() else { continue };
                    let target = match (s(v, "path"), git_src(v, &format!("{}/Cargo.toml", root), k)?) {
                        (_, Some(g)) => Patch::Git(g),
                        (Some(path), None) => Patch::Path(join(&root, &path)),
                        (None, None) => continue,
                    };
                    patches.insert((skey.clone(), s(v, "package").unwrap_or_else(|| k.clone())), target);
                }
            }
        }
        let mut lock: HashMap<String, Vec<String>> = HashMap::new();
        let mut lock_git = Vec::new();
        if let Ok(l) = read_toml(&format!("{}/Cargo.lock", root)) {
            if let Some(pkgs) = l.get("package").and_then(|p| p.as_array_of_tables()) {
                for p in pkgs {
                    let src = s(p, "source");
                    if src.as_deref().is_some_and(|x| x.starts_with("registry+")) {
                        if let (Some(n), Some(v)) = (s(p, "name"), s(p, "version")) {
                            lock.entry(n).or_default().push(v);
                        }
                    } else if let Some(g) = src.as_deref().and_then(parse_git_source) {
                        if !lock_git.iter().any(|x: &(GitSrc, crate::gitstore::Oid)| x.0.same(&g.0)) {
                            lock_git.push(g);
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
            lock_git,
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
        Resolver::with(sess, dir, GitPlaces::run())
    }

    /// `new` with git dependencies kept in `git` (tests).
    pub fn with(sess: &std::sync::Arc<crate::Session>, dir: &str, git: GitPlaces) -> Result<Resolver, String> {
        let offline = crate::git::cargo::offline(std::path::Path::new(dir))?;
        let git = std::cell::RefCell::new(GitDeps { places: git, pins: None, mounts: Vec::new(), found: HashMap::new() });
        Ok(Resolver { ws: std::rc::Rc::new(Workspace::load(&find_root(dir)?)?), git, offline, owners: HashMap::new(), vendor: vendor_dir(dir)?, fetched: Default::default(), manifests: Vec::new(), by_dir: HashMap::new(), cfg: crate::cfg::CfgSet::host(sess, &[]), // a check resolves C-compiling packages too: their scripts are not run without --allow-c
            // (unitgraph::run_build_scripts) and a check links nothing (`const_fuel` is check-only)
            allow_c: sess.options.allow_c || sess.options.const_fuel, metadata: false })
    }

    fn registry_dir(&self, name: &str, req: &str) -> Result<String, String> {
        match &self.vendor {
            Some((Vendor::Dir(vdir), config)) => return self.vendored_dir(vdir, config, name),
            // the repository's package of that name (cargo's discovery, as for a git dependency)
            Some((Vendor::Git(src), config)) => return self.git_dir(src, name).map_err(|e| format!("{} (crates-io's replacement, {})", e, config)),
            None => {}
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
            match crate::registry::exact(name, v, self.offline).and_then(|r| crate::registry::sources(name, &r, self.offline)) {
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
        let r = crate::registry::pick(name, req, &[], self.offline)?;
        let d = crate::registry::sources(name, &r, self.offline)?;
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

    /// The workspace whose `[workspace.package]` / `[workspace.dependencies]` the package at `dir`
    /// inherits from: the graph's own unless the package lies outside its root or is excluded
    /// (a fixture workspace's path dependency on an in-tree makepad crate). Registry packages are
    /// published normalized (nothing left to inherit); a git package inherits from the workspace
    /// of its own repository (found as cargo finds it, not above the repository's root).
    fn inherits_from(&mut self, dir: &str, source: &Source) -> Result<std::rc::Rc<Workspace>, String> {
        if let Source::Git(_) = source {
            let ceiling = self.git.borrow().mounts.iter().find(|m| dir == m.1 || dir.starts_with(&format!("{}/", m.1))).map(|m| m.1.clone());
            let root = find_root_in(dir, ceiling.as_deref())?;
            if let Some(w) = self.owners.get(&root) {
                return Ok(w.clone());
            }
            let w = std::rc::Rc::new(Workspace::load(&root)?);
            self.owners.insert(root, w.clone());
            return Ok(w);
        }
        if *source == Source::Registry || (dir == self.ws.root || dir.starts_with(&format!("{}/", self.ws.root))) && !self.ws.excluded(dir) {
            return Ok(self.ws.clone());
        }
        // a workspace already found that holds `dir` (cargo's ws_roots shortcut)
        if let Some(w) = self.owners.values().find(|w| dir.starts_with(&format!("{}/", w.root)) && !w.excluded(dir)) { return Ok(w.clone()); }
        let root = find_root(dir)?;
        if root == self.ws.root { return Ok(self.ws.clone()); }
        if let Some(w) = self.owners.get(&root) { return Ok(w.clone()); }
        let w = std::rc::Rc::new(Workspace::load(&root)?);
        self.owners.insert(root, w.clone());
        Ok(w)
    }

    /// The manifest of the package at `dir` (loaded once).
    fn load(&mut self, dir: &str, source: Source) -> Result<usize, String> {
        let dir = normalize(dir);
        if let Some(&i) = self.by_dir.get(&dir) {
            return Ok(i);
        }
        let t = read_toml(&format!("{}/Cargo.toml", dir))?;
        let pkg = table(&t, "package").ok_or_else(|| format!("{}/Cargo.toml: no [package]", dir))?;
        let ws = self.inherits_from(&dir, &source)?;
        let registry = source != Source::Path;
        let inherit = |k: &str| -> Option<String> {
            match pkg.get(k) {
                Some(Toml::Str(v, _)) => Some(v.clone()),
                Some(Toml::Table(x)) if b(x, "workspace") == Some(true) => s(&ws.ws_package, k),
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
                test: lt.and_then(|l| b(l, "test")).unwrap_or(true),
                doctest: lt.and_then(|l| b(l, "doctest")).unwrap_or(true),
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
            bins.push(Target { name: bname, path, edition: s(bt, "edition").unwrap_or_else(|| edition.clone()), kind: vec!["bin".to_string()], required_features: strs(bt, "required-features"), harness: b(bt, "harness").unwrap_or(true), test: b(bt, "test").unwrap_or(true), doctest: false });
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
            found.sort();
            for (n, p) in found {
                // an explicit target with that name or path replaces the discovered one
                if !bins.iter().any(|b| b.name == n || b.path == p) {
                    bins.push(Target { name: n, path: p, edition: edition.clone(), kind: vec!["bin".to_string()], required_features: Vec::new(), harness: true, test: true, doctest: false });
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
            tests.push(Target { name: tname, path, edition: s(tt, "edition").unwrap_or_else(|| edition.clone()), kind: vec!["test".to_string()], required_features: strs(tt, "required-features"), harness: b(tt, "harness").unwrap_or(true), test: b(tt, "test").unwrap_or(true), doctest: false });
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
                    tests.push(Target { name: n, path: p, edition: edition.clone(), kind: vec!["test".to_string()], required_features: Vec::new(), harness: true, test: true, doctest: false });
                }
            }
        }
        // [[example]] + discovered examples (cargo's target discovery: examples/*.rs and
        // examples/*/main.rs unless `autoexamples = false`, as for tests). A `crate-type` other
        // than `bin` is a library example: no program to run.
        let mut examples: Vec<Target> = Vec::new();
        let explicit_examples = t.get("example").and_then(|b| b.as_array_of_tables()).map(|v| v.to_vec()).unwrap_or_default();
        for et in &explicit_examples {
            let Some(ename) = s(et, "name") else { continue };
            let path = match s(et, "path") {
                Some(p) => join(&dir, &p),
                None => {
                    let cands = [format!("{}/examples/{}.rs", dir, ename), format!("{}/examples/{}/main.rs", dir, ename)];
                    match cands.iter().find(|c| crate::vfs::is_file(c)) {
                        Some(p) => p.clone(),
                        None => continue,
                    }
                }
            };
            let types = strs(et, "crate-type");
            let kind = if types.is_empty() || types.iter().any(|t| t == "bin") { vec!["example".to_string()] } else { types };
            examples.push(Target { name: ename, path, edition: s(et, "edition").unwrap_or_else(|| edition.clone()), kind, required_features: strs(et, "required-features"), harness: b(et, "harness").unwrap_or(true), test: b(et, "test").unwrap_or(false), doctest: false });
        }
        let autoexamples = match b(pkg, "autoexamples") {
            Some(v) => v,
            None => edition != "2015" || explicit_examples.is_empty(),
        };
        if autoexamples {
            let mut found: Vec<(String, String)> = Vec::new();
            if let Ok(rd) = crate::vfs::read_dir(format!("{}/examples", dir)) {
                for (n, is_dir) in rd {
                    if is_dir {
                        let p = format!("{}/examples/{}/main.rs", dir, n);
                        if crate::vfs::is_file(&p) {
                            found.push((n, p));
                        }
                    } else if let Some(stem) = n.strip_suffix(".rs") {
                        found.push((stem.to_string(), format!("{}/examples/{}", dir, n)));
                    }
                }
            }
            found.sort();
            for (n, p) in found {
                if !examples.iter().any(|x| x.name == n || x.path == p) {
                    examples.push(Target { name: n, path: p, edition: edition.clone(), kind: vec!["example".to_string()], required_features: Vec::new(), harness: true, test: false, doctest: false });
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
                    let w = match ws.ws_deps.get(key) {
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
                    base_dir = ws.root.clone();
                }
                let package = s(&st, "package").unwrap_or_else(|| key.clone());
                let mut git = git_src(&st, &format!("{}/Cargo.toml", dir), key)?;
                let mut path = s(&st, "path").map(|p| join(&base_dir, &p));
                // the root workspace's [patch] of the dependency's source (crates-io or its repository)
                if path.is_none() && (git.is_some() || !st.contains_key("registry")) {
                    let src = git.as_ref().map_or_else(|| "crates-io".to_string(), |g| canonical_url(&g.url));
                    match self.ws.patches.get(&(src, package.clone())) {
                        Some(Patch::Path(p)) => {
                            path = Some(p.clone());
                            git = None;
                        }
                        Some(Patch::Git(g)) => git = Some(g.clone()),
                        None => {}
                    }
                }
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
                    git,
                    req: s(&st, "version").unwrap_or_else(|| "*".to_string()),
                });
            }
            Ok(())
        };
        add(&mut deps, table(&t, "dependencies"), false, false, None)?;
        add(&mut deps, table(&t, "build-dependencies").or_else(|| table(&t, "build_dependencies")), true, false, None)?;
        // A registry package's dev-dependencies are never built (only the root package's tests
        // are): keep just their keys, except when metadata needs the declared dependencies.
        let mut dev_keys: Vec<String> = Vec::new();
        let dev_tbl = table(&t, "dev-dependencies").or_else(|| table(&t, "dev_dependencies"));
        if !registry || self.metadata {
            add(&mut deps, dev_tbl, false, true, None)?;
        } else if let Some(d) = dev_tbl {
            dev_keys.extend(d.keys().cloned());
        }
        if let Some(tt) = table(&t, "target") {
            for (plat, v) in tt {
                if let Some(v) = v.as_table() {
                    add(&mut deps, table(v, "dependencies"), false, false, Some(plat.clone()))?;
                    add(&mut deps, table(v, "build-dependencies").or_else(|| table(v, "build_dependencies")), true, false, Some(plat.clone()))?;
                    let dev_tbl = table(v, "dev-dependencies").or_else(|| table(v, "dev_dependencies"));
                    if !registry || self.metadata {
                        add(&mut deps, dev_tbl, false, true, Some(plat.clone()))?;
                    } else if let Some(d) = dev_tbl {
                        dev_keys.extend(d.keys().cloned());
                    }
                }
            }
        }
        let i = self.manifests.len();
        self.manifests.push(Manifest { dir: dir.clone(), name, version, edition, links, build, lib, bins, tests, examples, features, deps, dev_keys, proc_macro, source });
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

    /// The package directory a dependency entry names, and the package's source. A path inside a
    /// mounted repository is part of that git source (cargo: a git package's path dependencies
    /// in its repository share its source id).
    fn dep_dir(&self, d: &Dep) -> Result<(String, Source), String> {
        if let Some(p) = &d.path {
            let g = self.git.borrow();
            return Ok(match g.mounts.iter().find(|m| *p == m.1 || p.starts_with(&format!("{}/", m.1))) {
                Some(m) => (p.clone(), Source::Git(m.0.clone())),
                None => (p.clone(), Source::Path),
            });
        }
        if let Some(src) = &d.git {
            return self.git_dir(src, &d.package).map(|r| (r, Source::Git(src.clone())));
        }
        self.registry_dir(&d.package, &d.req).map(|r| (r, Source::Registry))
    }

    /// The folder of package `package` in the git source `src` (fetched and mounted once).
    fn git_dir(&self, src: &GitSrc, package: &str) -> Result<String, String> {
        if let Some(d) = self.git.borrow().found.get(&(src.clone(), package.to_string())) {
            return Ok(d.clone());
        }
        let (root, commit, manifests) = self.git_mount(src)?;
        let mut found: Option<String> = None;
        for rel in &manifests {
            let dir = if rel.is_empty() { root.clone() } else { format!("{}/{}", root, rel) };
            let path = format!("{}/Cargo.toml", dir);
            // parsed only when the text names the package
            let Ok(text) = crate::vfs::read_to_string(&path) else { continue };
            if !text.contains(package) {
                continue;
            }
            let Ok(t) = parse_toml(&text).map(|d| d.root) else { continue };
            let Some(pkg) = table(&t, "package") else { continue };
            if s(pkg, "name").as_deref() == Some(package) {
                found = Some(dir);
                break;
            }
        }
        let at = format!("{} at {}", src.id(), &crate::gitstore::hex(&commit)[..12]);
        let dir = found.ok_or_else(|| format!("no package named `{}` in the git repository {}", package, at))?;
        self.git.borrow_mut().found.insert((src.clone(), package.to_string()), dir.clone());
        Ok(dir)
    }

    /// The git source `src` mounted: its root, commit and manifest folders. The commit is
    /// Cargo.lock's, else relax's pin, else the remote's (then pinned).
    fn git_mount(&self, src: &GitSrc) -> Result<(String, crate::gitstore::Oid, Vec<String>), String> {
        if let Some(m) = self.git.borrow().mounts.iter().find(|m| m.0 == *src) {
            return Ok((m.1.clone(), m.2, m.3.clone()));
        }
        let locked = self.ws.lock_git.iter().find(|(g, _)| g.same(src)).map(|(_, c)| *c);
        let pinned = if locked.is_some() { None } else { self.git_pin(src)? };
        let revision = locked.or(pinned).or_else(|| match &src.reference {
            crate::git::fetch::GitRef::Rev(rev) => crate::gitstore::from_hex(&rev.to_ascii_lowercase()),
            _ => None,
        });
        if let Some(commit) = revision {
            if let Some(checkout) = crate::git::cargo::checkout(&crate::git::cargo::home(), &src.url, &commit) {
                // rustc compiler/rustc_interface/src/passes.rs::parse reads Input::File directly.
                // Cargo owns fetching; a resolved source path needs no second fetch or VFS copy.
                let root = normalize(&checkout.display().to_string());
                let manifests = checkout_manifests(&root)?;
                self.git.borrow_mut().mounts.push((src.clone(), root.clone(), commit, manifests.clone()));
                return Ok((root, commit, manifests));
            }
        }
        let home = self.git.borrow().places.home.clone();
        let cache = home.join("cache");
        let commit = crate::git::fetch::fetch_source(&cache, &src.url, &src.reference, revision, self.offline)?;
        if locked.is_none() && pinned.is_none() {
            self.set_git_pin(src, &commit)?;
        }
        let store_path = crate::gitstore::store_path(&cache, &src.url);
        let stem = store_path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = stem.strip_prefix("git-").unwrap_or(&stem).to_string();
        let root = normalize(&home.join("vfs").join("git").join(stem).join(&crate::gitstore::hex(&commit)[..12]).display().to_string());
        let manifests = {
            // the mounted store (index, file tables) is process state: kept heap, never the compiler zone
            let _k = crate::zone_alloc::kept();
            let store = crate::gitstore::Store::open(&store_path).map_err(|e| format!("{}: {e}", store_path.display()))?.ok_or_else(|| format!("{}: no store", store_path.display()))?;
            let table = store.index.table(&commit).ok_or_else(|| format!("{}: commit {} is not kept", store_path.display(), crate::gitstore::hex(&commit)))?;
            let manifests = manifest_dirs(table);
            crate::vfs::mount(&root, std::sync::Arc::new(store), &commit)?;
            manifests
        };
        self.git.borrow_mut().mounts.push((src.clone(), root.clone(), commit, manifests.clone()));
        Ok((root, commit, manifests))
    }

    fn git_lock_path(&self) -> std::path::PathBuf {
        let mut g = self.git.borrow_mut();
        g.places.lock.get_or_insert_with(|| crate::dirs::relax_dir().join("git.lock")).clone()
    }

    /// relax's pin of `src` (`<relax dir>/git.lock`: `<source id>\t<commit>` lines).
    fn git_pin(&self, src: &GitSrc) -> Result<Option<crate::gitstore::Oid>, String> {
        if self.git.borrow().pins.is_none() {
            let path = self.git_lock_path();
            let mut pins = BTreeMap::new();
            if let Ok(text) = std::fs::read_to_string(&path) {
                for line in text.lines() {
                    if let Some((id, c)) = line.split_once('\t') {
                        pins.insert(id.to_string(), c.trim().to_string());
                    }
                }
            }
            self.git.borrow_mut().pins = Some(pins);
        }
        let g = self.git.borrow();
        Ok(g.pins.as_ref().and_then(|p| p.get(&src.id())).and_then(|c| crate::gitstore::from_hex(c)))
    }

    /// Records `commit` as relax's pin of `src` (the file rewritten whole, renamed into place).
    fn set_git_pin(&self, src: &GitSrc, commit: &crate::gitstore::Oid) -> Result<(), String> {
        let path = self.git_lock_path();
        let text = {
            let mut g = self.git.borrow_mut();
            let pins = g.pins.get_or_insert_with(BTreeMap::new);
            pins.insert(src.id(), crate::gitstore::hex(commit));
            let mut text = String::from("# makepad run's pins of git dependencies Cargo.lock does not list: <source id>\t<commit>\n");
            for (id, c) in pins.iter() {
                text.push_str(&format!("{}\t{}\n", id, c));
            }
            text
        };
        let err = |e: std::io::Error| format!("{}: {}", path.display(), e);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(err)?;
        }
        let tmp = path.with_extension(format!("lock.{}", std::process::id()));
        std::fs::write(&tmp, text).map_err(err)?;
        std::fs::rename(&tmp, &path).map_err(err)
    }

    /// The workspace member named `package` (members, and path dependencies under the root).
    /// A manifest that does not load is passed over while another may be the package; when none
    /// is, its error is the answer (a duplicate key in the package's own manifest read as "not a
    /// member"), the ones whose text names the package first.
    fn find_member(&mut self, package: &str) -> Result<usize, String> {
        let mut queue: Vec<String> = self.ws.member_dirs();
        // manifests that did not load: (names the package, error), in the order met
        let mut broken: Vec<(bool, String)> = Vec::new();
        // fast path: a member whose manifest names the package (parsed only when the text matches)
        let needle = format!("\"{}\"", package);
        for d in &queue {
            let path = format!("{}/Cargo.toml", d);
            if crate::vfs::read_to_string(&path).is_ok_and(|t| t.contains(&needle)) {
                match self.load(d, Source::Path) {
                    Ok(i) if self.manifests[i].name == package => return Ok(i),
                    Ok(_) => {}
                    Err(e) => broken.push((true, e)),
                }
            }
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(d) = queue.pop() {
            let d = normalize(&d);
            if !seen.insert(d.clone()) || !crate::vfs::is_file(format!("{}/Cargo.toml", d)) {
                continue;
            }
            let i = match self.load(&d, Source::Path) {
                Ok(i) => i,
                Err(e) => {
                    if !broken.iter().any(|b| b.1 == e) {
                        broken.push((false, e));
                    }
                    continue;
                }
            };
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
        if !broken.is_empty() {
            broken.sort_by_key(|b| !b.0);
            let errs: Vec<&str> = broken.iter().map(|b| b.1.as_str()).collect();
            return Err(format!(
                "{}\n(package `{}` was not found in the workspace at {}: {} see above)",
                errs.join("\n"),
                package,
                self.ws.root,
                if errs.len() == 1 {
                    "a member's manifest does not load,".to_string()
                } else {
                    format!("{} members' manifests do not load,", errs.len())
                }
            ));
        }
        Err(format!(
            "package `{}` is not a member of the workspace at {}: no member or path dependency there has that name (check the name; a private repository's package needs that repository checked out under apps/, e.g. apps/commercial)",
            package, self.ws.root
        ))
    }
}

/// The enabled test programs of the workspace member `package`: its lib target, bin targets,
/// and integration test targets by name with their source paths
/// (cargo test's order: the lib's unit tests, the bins' unit tests, then the test targets by name).
/// The package's lib path, bins and integration tests: (name, path, harness) each. Missing
/// required features skip implicit targets and reject explicitly named targets before any run.
pub fn test_targets(sess: &std::sync::Arc<crate::Session>, dir: &str, package: &str, flags: &[String], bin_names: &[String], test_names: &[String]) -> Result<(Option<LibTarget>, Vec<(String, String, bool)>, Vec<(String, String, bool)>), String> {
    let o = parse_flags(flags)?;
    let mut r = Resolver::new(sess, dir)?;
    let i = r.find_member(package)?;
    let m = &r.manifests[i];
    let features = enabled_features(&m.features, &m.implicit_features(), &o).map_err(|e| format!("package `{package}`: {e}"))?;
    let enabled = |targets: &[Target], names: &[String]| -> Result<Vec<(String, String, bool)>, String> {
        let mut out = Vec::new();
        for t in targets {
            let named = names.contains(&t.name);
            if (t.test || named) && t.enabled(&features, named)? {
                out.push((t.name.clone(), t.path.clone(), t.harness));
            }
        }
        out.sort();
        Ok(out)
    };
    let bins = enabled(&m.bins, bin_names)?;
    let tests = enabled(&m.tests, test_names)?;
    Ok((m.lib.as_ref().map(|l| LibTarget { path: l.path.clone(), test: l.test, doctest: l.doctest }), bins, tests))
}

/// The package's lib as `cargo test` sees it.
pub struct LibTarget {
    pub path: String,
    /// `[lib] test = false`: its unit tests run only when asked for (`--lib`)
    pub test: bool,
    /// `[lib] doctest = false`: it has no doc tests to run
    pub doctest: bool,
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

/// The folders of a repository's tree (a commit's path table) whose Cargo.toml cargo's package
/// discovery in a git checkout reads, in its order: the root first, then depth-first; no folder
/// under a hidden one (`.x`), a `target` folder beside a Cargo.toml, or a nested repository (a
/// folder with its own `.git`).
fn manifest_dirs(table: &[crate::gitstore::Entry]) -> Vec<String> {
    let files: std::collections::HashSet<&str> = table.iter().map(|e| e.path.as_str()).collect();
    let has = |dir: &str, name: &str| files.contains(if dir.is_empty() { name.to_string() } else { format!("{}/{}", dir, name) }.as_str());
    let mut out: Vec<String> = Vec::new();
    'entries: for e in table {
        let dir = match e.path.strip_suffix("Cargo.toml") {
            Some("") => "",
            Some(d) if d.ends_with('/') => &d[..d.len() - 1],
            _ => continue,
        };
        let mut parent = "";
        if !dir.is_empty() {
            for (i, part) in dir.split('/').enumerate() {
                let me = &dir[..parent.len() + if i == 0 { 0 } else { 1 } + part.len()];
                if part.starts_with('.') || (part == "target" && has(parent, "Cargo.toml")) || has(me, ".git") {
                    continue 'entries;
                }
                parent = me;
            }
        }
        out.push(dir.to_string());
    }
    // depth-first from the root: a folder's own manifest before its subfolders', siblings by name
    out.sort_by(|a, b| {
        let ka: Vec<&str> = if a.is_empty() { Vec::new() } else { a.split('/').collect() };
        let kb: Vec<&str> = if b.is_empty() { Vec::new() } else { b.split('/').collect() };
        ka.cmp(&kb)
    });
    out
}

fn checkout_manifests(root: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut todo = vec![String::new()];
    while let Some(rel) = todo.pop() {
        let dir = if rel.is_empty() { root.to_string() } else { format!("{root}/{rel}") };
        if !rel.is_empty() && crate::vfs::exists(format!("{dir}/.git")) { continue; }
        let manifest = crate::vfs::is_file(format!("{dir}/Cargo.toml"));
        if manifest { out.push(rel.clone()); }
        let entries = crate::vfs::read_dir(&dir).map_err(|e| format!("{dir}: {e}"))?;
        for (name, is_dir) in entries.into_iter().rev() {
            if is_dir && !name.starts_with('.') && !(manifest && name == "target") {
                todo.push(if rel.is_empty() { name } else { format!("{rel}/{name}") });
            }
        }
    }
    Ok(out)
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
/// A `git` source (with `branch`, `tag` or `rev`) serves the packages of that repository, fetched
/// and mounted as git dependencies are (their package ids stay crates-io's, as cargo keeps the
/// replaced source's ids). Another replacement kind (a registry) is an error.
fn vendor_dir(dir: &str) -> Result<Option<(Vendor, String)>, String> {
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
    let config = format!("{}/.cargo", d);
    if let Some(g) = git_src(src, &config, &format!("source.{}", with))? {
        return Ok(Some((Vendor::Git(g), config)));
    }
    match s(src, "directory") {
        Some(p) => Ok(Some((Vendor::Dir(join(d, &p)), config))),
        None => Err(format!("{}/.cargo: [source.{}] is neither a `directory` nor a `git` source: makepad run reads vendored crates and git repositories only", d, with)),
    }
}

/// crates-io's replacement source.
enum Vendor {
    Dir(String),
    Git(GitSrc),
}

/// cargo's target directory for a start folder (what `cargo metadata` reports as
/// `target_directory`): CARGO_TARGET_DIR, else CARGO_BUILD_TARGET_DIR (relative to the current
/// folder), else `[build] target-dir` of the first cargo config file that sets it
/// (cargo_config_files: the nearest at or above `dir`, then $CARGO_HOME's; relative to the folder
/// holding that `.cargo`), else `<workspace root>/target` (find_root).
pub fn target_dir(dir: &str) -> Result<String, String> {
    for k in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"] {
        if let Some(v) = std::env::var(k).ok().filter(|v| !v.is_empty()) {
            let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
            return Ok(join(&cwd, &v));
        }
    }
    for (d, path) in cargo_config_files(dir) {
        if let Some(t) = table(&read_toml(&path)?, "build").and_then(|b| s(b, "target-dir")) {
            return Ok(join(&d, &t));
        }
    }
    Ok(format!("{}/target", find_root(dir)?))
}

pub fn find_root(dir: &str) -> Result<String, String> {
    find_root_in(dir, None)
}

/// The cargo config files for a start folder, in cargo's order (GlobalContext::walk_tree): the
/// `.cargo/config` (else `.cargo/config.toml`) of `dir` and of every folder above it, then
/// $CARGO_HOME's when none of those is it. Each with the folder its relative paths resolve
/// against: the one holding that `.cargo` (the folder above CARGO_HOME for its config).
fn cargo_config_files(dir: &str) -> Vec<(String, String)> {
    let in_dir = |d: &str| ["config", "config.toml"].iter().map(|f| format!("{}/{}", d, f)).find(|p| crate::vfs::is_file(p));
    let mut files: Vec<(String, String)> = Vec::new();
    let mut cur = Some(normalize(dir));
    while let Some(d) = cur {
        if let Some(path) = in_dir(&format!("{}/.cargo", d)) { files.push((d.clone(), path)); }
        cur = parent(&d);
    }
    let home = std::env::var("CARGO_HOME").ok().filter(|h| !h.is_empty()).map(|h| normalize(&h)).or_else(|| {
        std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok().map(|h| format!("{}/.cargo", normalize(&h)))
    });
    if let Some(path) = home.as_deref().and_then(in_dir) {
        if !files.iter().any(|(_, p)| *p == path) { files.push((parent(home.as_deref().unwrap_or_default()).unwrap_or_default(), path)); }
    }
    files
}

/// cargo's `[env]` configuration as it applies for a start folder (GlobalContext::env_config):
/// what cargo hands its rustc, build scripts and `cargo run` programs beside the variables it sets
/// itself. The `[env]` tables of the config files (cargo_config_files) merge as cargo merges
/// config: the nearest file's value wins, a `{ value, force, relative }` entry merges field by
/// field, and a string in one file against a table in another is an error. A `relative` value is
/// joined to the folder of the file its `value` comes from (Path::join: unnormalized, an absolute
/// value stands). An entry applies when it is `force`d or the process environment lacks the
/// variable. CARGO_HOME, RUSTUP_HOME and RUSTUP_TOOLCHAIN may not be set there.
pub fn cargo_env(dir: &str) -> Result<Vec<(String, String)>, String> {
    // a plain string, or the table's (value and its file's folder, force, relative)
    enum Entry { Plain(String), Options(Option<(String, String)>, Option<bool>, Option<bool>) }
    let mut env: BTreeMap<String, (Entry, String)> = BTreeMap::new();
    for (base, path) in cargo_config_files(dir) {
        let t = read_toml(&path)?;
        let Some(tab) = t.get("env") else { continue };
        let Toml::Table(tab) = tab else { return Err(format!("{}: `env` is not a table", path)) };
        for (k, v) in tab {
            let flag = |e: &TomlTable, f: &str| match e.get(f) {
                None => Ok(None),
                Some(Toml::Bool(x, _)) => Ok(Some(*x)),
                Some(_) => Err(format!("{}: [env] `{}`: `{}` is not a boolean", path, k, f)),
            };
            let new = match v {
                Toml::Str(value, _) => Entry::Plain(value.clone()),
                Toml::Table(e) => {
                    let value = match e.get("value") {
                        None => None,
                        Some(Toml::Str(value, _)) => Some((value.clone(), base.clone())),
                        Some(_) => return Err(format!("{}: [env] `{}`: `value` is not a string", path, k)),
                    };
                    Entry::Options(value, flag(e, "force")?, flag(e, "relative")?)
                }
                _ => return Err(format!("{}: [env] `{}` is neither a string nor a table", path, k)),
            };
            match env.get_mut(k) {
                None => { env.insert(k.clone(), (new, path.clone())); }
                Some((Entry::Plain(_), _)) if matches!(new, Entry::Plain(_)) => {}
                Some((Entry::Options(value, force, relative), _)) if matches!(new, Entry::Options(..)) => {
                    let Entry::Options(v2, f2, r2) = new else { unreachable!() };
                    if value.is_none() { *value = v2; }
                    if force.is_none() { *force = f2; }
                    if relative.is_none() { *relative = r2; }
                }
                Some((_, first)) => return Err(format!("failed to merge [env] `{}` between {} and {}: a string against a table", k, first, path)),
            }
        }
    }
    let mut out = Vec::new();
    for (k, (e, path)) in env {
        if ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"].contains(&k.as_str()) {
            return Err(format!("{}: setting the `{}` environment variable is not supported in the `[env]` configuration table", path, k));
        }
        let (value, force) = match e {
            Entry::Plain(value) => (value, false),
            Entry::Options(None, _, _) => return Err(format!("{}: [env] `{}` is a table without a `value`", path, k)),
            Entry::Options(Some((value, base)), force, relative) => {
                let value = if relative == Some(true) { std::path::Path::new(&base).join(&value).to_string_lossy().into_owned() } else { value };
                (value, force == Some(true))
            }
        };
        if force || std::env::var_os(&k).is_none() { out.push((k, value)); }
    }
    Ok(out)
}

/// What `file!()` names a source file relative to, as cargo decides it (`path_args`): a file of a
/// path package under the workspace root of the run is named relative to that root; a file of a
/// registry, git or vendored package, or of a path package outside the root, as given. The
/// folders under the root that hold no path package are where the resolver reads the others
/// from: Relax's registry store (inside the target folder), the cargo home and a vendored folder.
pub struct FileRoot {
    root: String,
    not_path: Vec<String>,
}

impl FileRoot {
    /// The workspace of a run started in `dir`.
    pub fn of(dir: &str) -> Option<FileRoot> {
        let root = find_root(dir).ok()?;
        let mut not_path = vec![normalize(&crate::dirs::relax_dir().join("registry").display().to_string()), normalize(&crate::git::cargo::home().display().to_string())];
        if let Ok(Some((Vendor::Dir(v), _))) = vendor_dir(dir) {
            not_path.push(normalize(&v));
        }
        Some(FileRoot { root, not_path })
    }

    /// `path` relative to the root when it is a path package's file under it.
    pub fn relative(&self, path: &str) -> Option<String> {
        if self.not_path.iter().any(|n| crate::vfs::relative_path(path, n).is_some()) {
            return None;
        }
        crate::vfs::relative_path(path, &self.root).filter(|r| !r.is_empty())
    }
}

/// `find_root` looking no higher than `ceiling` (a mounted repository's root).
fn find_root_in(dir: &str, ceiling: Option<&str>) -> Result<String, String> {
    let up = |d: &str| if ceiling == Some(d) { None } else { parent(d) };
    let start = normalize(dir);
    let mut cur = Some(start.clone());
    let mut current: Option<String> = None;
    while let Some(d) = cur {
        if crate::vfs::is_file(format!("{}/Cargo.toml", d)) {
            current = Some(d);
            break;
        }
        cur = up(&d);
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
        cur = up(&d);
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

impl Req {
    fn parse(value: &str) -> Self {
        if let Some(d) = value.strip_prefix("dep:") {
            Self::Dep(d.to_string())
        } else if let Some((d, f)) = value.split_once("?/") {
            Self::DepFeature(d.to_string(), f.to_string(), true)
        } else if let Some((d, f)) = value.split_once('/') {
            Self::DepFeature(d.to_string(), f.to_string(), false)
        } else {
            Self::Feature(value.to_string())
        }
    }
}

fn feature_requests(features: &BTreeMap<String, Vec<String>>, implicit: &BTreeSet<String>, o: &Options) -> Vec<Req> {
    let mut work = Vec::new();
    if !o.no_default_features {
        work.push(Req::Feature("default".to_string()));
    }
    if o.all_features {
        work.extend(features.keys().chain(implicit).cloned().map(Req::Feature));
    }
    work.extend(o.features.iter().map(|f| Req::parse(f)));
    work
}

/// Package-local feature closure for target selection, without resolving or fetching dependencies.
/// Dependency features are not local features; a strong `dep/feature` can enable an implicit
/// optional-dependency feature, while `dep:dep` and `dep?/feature` cannot.
fn enabled_features(features: &BTreeMap<String, Vec<String>>, implicit: &BTreeSet<String>, o: &Options) -> Result<BTreeSet<String>, String> {
    let mut enabled = BTreeSet::new();
    let mut work = feature_requests(features, implicit, o);
    while let Some(req) = work.pop() {
        match req {
            Req::Feature(f) => {
                if enabled.contains(&f) { continue; }
                if let Some(values) = features.get(&f) {
                    enabled.insert(f);
                    work.extend(values.iter().map(|v| Req::parse(v)));
                } else if implicit.contains(&f) {
                    enabled.insert(f);
                } else if f != "default" {
                    return Err(format!("has no feature `{f}`"));
                }
            }
            Req::DepFeature(d, _, false) if implicit.contains(&d) => work.push(Req::Feature(d)),
            _ => {}
        }
    }
    Ok(enabled)
}

pub struct Options {
    pub offline: bool,
    pub no_default_features: bool,
    pub all_features: bool,
    pub features: Vec<String>,
    pub bin: Option<String>,
    /// `--example <name>`: that example is the program (a bin built with the dev-dependencies)
    pub example: Option<String>,
    /// `--lib`, `--bins`, `--bin <name>`, `--tests`, `--test <name>` (cargo test's target
    /// selection): the graph of the package's test harness. The lib's unit tests are a program of
    /// their own (the lib is cfg(test) there and plain beside the bins' unit tests and the
    /// integration tests), so one graph holds either the lib test or bin and integration tests.
    pub test: Option<TestSel>,
    /// `makepad check`'s target flags, which are cargo check's (parse_check_flags); None: none
    /// given (the lib and every bin) or a run's or test's graph.
    pub check: Option<CheckSel>,
}

/// `makepad check`'s target selection, read as `cargo check` reads its target flags (cargo
/// ops/cargo_compile/compile_filter.rs): `--lib` the lib, `--bins` / `--bin <name>` the bins,
/// `--tests` every target with `test = true` built with `--test` (the lib's unit tests, the bins'
/// unit tests and the integration tests), `--test <name>` that integration test, in one graph.
/// `makepad test` (parse_flags) picks test programs instead: there `--lib` is the lib's unit tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckSel {
    pub lib: bool,
    pub all_bins: bool,
    pub bins: Vec<String>,
    pub all_tests: bool,
    pub tests: Vec<String>,
}

impl CheckSel {
    /// A target of it is built with `--test` (the graph resolves the dev-dependencies).
    fn tests_anything(&self) -> bool {
        self.all_tests || !self.tests.is_empty()
    }
}

/// `makepad check`'s flags: cargo check's target selection (CheckSel), the rest as parse_flags.
pub fn parse_check_flags(flags: &[String]) -> Result<Options, String> {
    let mut sel = CheckSel::default();
    let mut any = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i].as_str();
        let value = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            flags.get(*i).cloned().ok_or_else(|| format!("{} needs a value", f))
        };
        match f {
            "--lib" => sel.lib = true,
            "--bins" => sel.all_bins = true,
            "--bin" => sel.bins.push(value(&mut i)?),
            "--tests" => sel.all_tests = true,
            "--test" => sel.tests.push(value(&mut i)?),
            // (a flag's value is no target flag: `--features lib`)
            "--features" | "-F" => {
                rest.push(f.to_string());
                rest.push(value(&mut i)?);
                i += 1;
                continue;
            }
            _ => {
                if let Some(v) = f.strip_prefix("--bin=") {
                    sel.bins.push(v.to_string());
                } else if let Some(v) = f.strip_prefix("--test=") {
                    sel.tests.push(v.to_string());
                } else {
                    rest.push(f.to_string());
                    i += 1;
                    continue;
                }
            }
        }
        any = true;
        i += 1;
    }
    let mut o = parse_flags(&rest)?;
    if any {
        o.check = Some(sel);
    }
    Ok(o)
}

#[derive(Clone, Debug, PartialEq)]
pub enum TestSel {
    /// the lib target built with `--test`
    Lib,
    /// these bin targets built with `--test` and these integration test targets, beside the plain
    /// lib: one graph, as `cargo test` builds them in one go with one feature resolution (`makepad
    /// test` runs each target out of it in a process of its own, run.rs `--subject`). None: no
    /// target of that kind; empty: all of them.
    Targets { bins: Option<Vec<String>>, tests: Option<Vec<String>> },
}

pub fn parse_flags(flags: &[String]) -> Result<Options, String> {
    let mut o = Options { offline: false, no_default_features: false, all_features: false, features: Vec::new(), bin: None, example: None, test: None, check: None };
    let mut lib = false;
    let mut bins = false;
    let mut bin_names: Vec<String> = Vec::new();
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
                o.features.extend(v.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).map(str::to_string));
            }
            "--bins" => bins = true,
            "--bin" => bin_names.push(value(&mut i)?),
            "--example" => o.example = Some(value(&mut i)?),
            "--lib" => lib = true,
            "--tests" => {
                tests.get_or_insert_with(Vec::new);
            }
            "--test" => {
                let v = value(&mut i)?;
                tests.get_or_insert_with(Vec::new).push(v);
            }
            "--release" => {}
            "--offline" | "--frozen" => o.offline = true,
            _ => {
                if let Some(v) = f.strip_prefix("--features=").or_else(|| f.strip_prefix("-F").map(|v| v.trim_start_matches('='))) {
                    o.features.extend(v.split(|c: char| c == ',' || c.is_whitespace()).filter(|x| !x.is_empty()).map(str::to_string));
                } else if let Some(v) = f.strip_prefix("--bin=") {
                    bin_names.push(v.to_string());
                } else if let Some(v) = f.strip_prefix("--example=") {
                    o.example = Some(v.to_string());
                } else if let Some(v) = f.strip_prefix("--test=") {
                    tests.get_or_insert_with(Vec::new).push(v.to_string());
                } else {
                    return Err(format!("unit graph: flag `{}` is not supported by makepad run's resolver", f));
                }
            }
        }
        i += 1;
    }
    // (cargo run takes one target: a bin or an example)
    if o.example.is_some() && (bins || !bin_names.is_empty() || lib || tests.is_some()) {
        return Err("unit graph: `--example` runs one example; it takes no other target flag (--bin, --bins, --lib, --tests, --test)".to_string());
    }
    let bins = if bins {
        Some(bin_names)
    } else {
        o.bin = bin_names.into_iter().last();
        None
    };
    o.test = match (lib, bins, tests) {
        (true, None, None) => Some(TestSel::Lib),
        (false, None, None) => None,
        (false, bins, tests) => Some(TestSel::Targets { bins, tests }),
        _ => return Err("unit graph: the lib's unit tests (--lib) are a test program of their own; select them alone".to_string()),
    };
    Ok(o)
}

struct Feat<'a> {
    r: &'a mut Resolver,
    /// Test roots, or all workspace members for metadata.
    dev_of: BTreeSet<(usize, bool)>,
    /// Metadata without --filter-platform follows dependencies for every platform.
    all_platforms: bool,
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
            if !d.optional && (!d.dev || self.dev_of.contains(&k)) {
                self.follow(k, di, d)?;
            }
        }
        Ok(())
    }

    /// Follows dependency entry `di` of node `k`: the target node, its requested features.
    fn follow(&mut self, k: (usize, bool), di: usize, d: &Dep) -> Result<(), String> {
        if !self.all_platforms && !self.r.platform_ok(&d.platform) {
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
        if d.build && self.r.manifests[k.0].source != Source::Path && !self.r.allow_c {
            if let Some(c) = crate::registry::c_build_dep(std::slice::from_ref(&d.package)) {
                let m = &self.r.manifests[k.0];
                return Err(format!("registry package `{}` {} compiles C or C++ (build-dependency `{}`): it {}", m.name, m.version, c, relax_session::options::C_GATE));
            }
        }
        let (dir, source) = self.r.dep_dir(d)?;
        let t = self.r.load(&dir, source)?;
        if self.r.manifests[t].name != d.package {
            return Err(format!("{}/Cargo.toml: dependency `{}` names package `{}`, but {} holds `{}`", self.r.manifests[k.0].dir, d.key, d.package, dir, self.r.manifests[t].name));
        }
        // a git package must meet the entry's version requirement (cargo's resolution checks it)
        if let (Some(g), true) = (&d.git, d.req != "*") {
            let m = &self.r.manifests[t];
            if crate::registry::Version::parse(&m.version).is_some_and(|v| !crate::registry::matches(&d.req, &v)) {
                return Err(format!("{}/Cargo.toml: git package `{}` {} ({}) does not satisfy the requirement `{}`", self.r.manifests[k.0].dir, m.name, m.version, g.id(), d.req));
            }
        }
        let host = !self.r.metadata && (k.1 || d.build || self.r.manifests[t].proc_macro);
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
                            self.work.push((k, Req::parse(&v)));
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
                    if !has && self.r.manifests[k.0].dev_keys.iter().any(|x| *x == d) {
                        // `x/f` on a dev-dependency of a package whose tests are not built: no
                        // dependency `x` is activated, so nothing is enabled (vsss-rs 6.0.1's
                        // `rand/alloc` with rand a dev-dependency)
                        continue;
                    }
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
    /// a host-side unit (build-dependency / proc-macro side)
    pub host: bool,
    /// build | run-custom-build
    pub mode: String,
    /// built with rustc's `--test`: libtest's generated main replaces the crate's own
    /// (a lib's or bin's unit tests, an integration test without `harness = false`)
    pub harness: bool,
    /// Relax's own sysroot crate of its name (run.rs with_sysroot_proc_macro); no package's unit is
    pub sysroot: bool,
    pub features: Vec<String>,
    /// (unit index, extern crate name)
    pub deps: Vec<(usize, String)>,
}

/// A unit graph: its units and the indexes of its roots.
#[derive(Clone, Debug, Default)]
pub struct Units {
    pub units: Vec<GraphUnit>,
    pub roots: Vec<usize>,
    /// binary target names of the root package when building integration tests (`CARGO_BIN_EXE_<name>`)
    pub bins: Vec<String>,
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
        let mut units: Vec<GraphUnit> = units
            .iter()
            .map(|u| {
                let t = u.get("target");
                GraphUnit {
                    pkg_id: s(u.get("pkg_id")),
                    kind: list(t.and_then(|t| t.get("kind"))),
                    name: s(t.and_then(|t| t.get("name"))),
                    src_path: s(t.and_then(|t| t.get("src_path"))),
                    edition: s(t.and_then(|t| t.get("edition"))),
                    host: matches!(u.get("relax_host"), Some(crate::json::Json::Bool(true))),
                    mode: s(u.get("mode")),
                    harness: u.get("mode").is_some_and(|m| m.str() == "test"),
                    sysroot: false,
                    features: list(u.get("features")),
                    deps: u.get("dependencies").map(|d| d.arr().iter().filter_map(|dep| Some((dep.get("index")?.num() as usize, s(dep.get("extern_crate_name"))))).collect()).unwrap_or_default(),
                }
            })
            .collect();
        // Cargo may optimize host dependencies and may leave target dependencies
        // unoptimized. Their compilation side follows edges from proc macros and
        // build scripts, independently of either profile or target triple.
        let mut pending: Vec<usize> = units.iter().enumerate().filter_map(|(i, u)|
            (u.host || u.kind.iter().any(|k| matches!(k.as_str(), "proc-macro" | "custom-build"))).then_some(i)).collect();
        let mut seen = vec![false; units.len()];
        while let Some(i) = pending.pop() {
            if i >= units.len() || std::mem::replace(&mut seen[i], true) { continue; }
            units[i].host = true;
            pending.extend(units[i].deps.iter().map(|d| d.0));
        }
        let roots = j.get("roots").map(|r| r.arr().iter().map(|x| x.num() as usize).collect()).unwrap_or_default();
        Ok(Units { units, roots, bins: Vec::new() })
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

/// The cargo metadata subset used by build scripts, from the same manifests and VFS as the
/// unit graph. No script is run and Cargo.lock is never written. `dir` is the caller's virtual cwd.
pub fn cargo_metadata(sess: &std::sync::Arc<crate::Session>, dir: &str, flags: &[String]) -> Result<String, String> {
    use crate::diag::quote as q;
    let (mut manifest, mut platform) = (None, None);
    let (mut offline, mut no_deps) = (false, false);
    let mut i = 0;
    while i < flags.len() {
        let arg = flags[i].as_str();
        let (flag, inline) = arg.split_once('=').map_or((arg, None), |(k, v)| (k, Some(v)));
        let value = |i: &mut usize| -> Result<String, String> {
            if let Some(v) = inline { return Ok(v.to_string()); }
            *i += 1;
            let v = flags.get(*i).ok_or_else(|| format!("metadata: {flag} needs a value"))?;
            if v.starts_with('-') { return Err(format!("metadata: {flag} needs a value, found {v:?}")); }
            Ok(v.clone())
        };
        match flag {
            "--format-version" => {
                let v = value(&mut i)?;
                if v != "1" { return Err(format!("metadata: unsupported --format-version {v:?}")); }
            }
            "--manifest-path" => manifest = Some(join(dir, &value(&mut i)?)),
            "--filter-platform" => platform = Some(value(&mut i)?),
            "--offline" | "--frozen" if inline.is_none() => offline = true,
            "--locked" if inline.is_none() => {}
            "--no-deps" if inline.is_none() => no_deps = true,
            _ => return Err(format!("metadata: unsupported argument {arg:?}")),
        }
        i += 1;
    }
    let manifest = match manifest {
        Some(p) if p.ends_with("/Cargo.toml") => p,
        Some(p) => return Err(format!("metadata: --manifest-path must name Cargo.toml: {p}")),
        None => {
            let mut cur = Some(normalize(dir));
            loop {
                let d = cur.ok_or_else(|| format!("no Cargo.toml in {dir} or any folder above it"))?;
                let p = format!("{d}/Cargo.toml");
                if crate::vfs::is_file(&p) { break p; }
                cur = parent(&d);
            }
        }
    };
    let current = read_toml(&manifest)?;
    let start = parent(&manifest).unwrap();
    let mut r = Resolver::new(sess, &start)?;
    r.offline |= offline;
    r.metadata = true;
    // C-compiling packages are valid metadata; no compilation is requested here.
    r.allow_c = true;
    if let Some(t) = &platform {
        use crate::options::CheckTarget as C;
        r.cfg = match C::from_triple(t).ok_or_else(|| format!("metadata: unsupported --filter-platform {t:?}"))? {
            C::MacosArm64 => crate::cfg::CfgSet::macos_arm64(sess, &[]),
            C::LinuxX64 => crate::cfg::CfgSet::linux_x64(sess, &[]),
            C::WindowsX64 => crate::cfg::CfgSet::windows_x64(sess, &[]),
            C::AndroidArm64 => crate::cfg::CfgSet::android_arm64(sess, &[]),
            C::IosArm64 => crate::cfg::CfgSet::ios_arm64(sess, &[]),
            C::Web(w) => crate::cfg::CfgSet::web(sess, w, &[]),
            C::Host => unreachable!("from_triple names a target"),
        };
    }
    let root = if current.contains_key("package") { Some(r.load(&start, Source::Path)?) } else { None };
    // As in Workspace::has_member, membership follows every in-workspace path dependency,
    // including optional, dev, build and inactive platform entries. --no-deps still lists them.
    let mut todo = r.ws.member_dirs();
    let mut members = BTreeSet::new();
    if let Some(root) = root { todo.push(r.manifests[root].dir.clone()); }
    while let Some(dir) = todo.pop() {
        let p = r.load(&dir, Source::Path)?;
        if !members.insert(p) { continue; }
        for d in &r.manifests[p].deps {
            if let Some(path) = &d.path {
                if path.starts_with(&format!("{}/", r.ws.root)) && !r.ws.excluded(path) {
                    todo.push(path.clone());
                }
            }
        }
    }
    let nodes = if no_deps {
        HashMap::new()
    } else {
        let mut f = Feat { r: &mut r, dev_of: members.iter().map(|p| (*p, false)).collect(), all_platforms: platform.is_none(), nodes: HashMap::new(), work: Vec::new(), order: Vec::new() };
        for &p in &members {
            let k = (p, false);
            f.activate(k)?;
            f.work.push((k, Req::Feature("default".to_string())));
        }
        f.run()?;
        f.nodes
    };
    let ids: Vec<String> = r.manifests.iter().map(|m| {
        if m.source != Source::Path { return m.pkg_id(); }
        // Always include the name, including when the last directory component matches it.
        let path = if m.dir.starts_with('/') { m.dir.clone() } else { format!("/{}", m.dir) };
        let mut url = String::new();
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~:@!$&'()*+,;=".contains(&b) { url.push(b as char); }
            else { url.push_str(&format!("%{b:02X}")); }
        }
        format!("path+file://{}#{}@{}", url, m.name, m.version)
    }).collect();
    let mut packages: Vec<usize> = if no_deps { members.iter().copied().collect() } else { nodes.keys().map(|k| k.0).collect() };
    packages.sort_by(|a, b| ids[*a].cmp(&ids[*b]));
    let mut pkgs = Vec::new();
    let mut resolved = Vec::new();
    for &p in &packages {
        let m = &r.manifests[p];
        let dependencies: Vec<String> = m.deps.iter().map(|d| format!("{{\"name\":{},\"kind\":{},\"optional\":{}}}", q(&d.package), metadata_dep_kind(d), d.optional)).collect();
        pkgs.push(format!("{{\"id\":{},\"name\":{},\"version\":{},\"manifest_path\":{},\"targets\":[{}],\"dependencies\":[{}]}}",
            q(&ids[p]), q(&m.name), q(&m.version), q(&format!("{}/Cargo.toml", m.dir)), metadata_targets(m)?.join(","), dependencies.join(",")));
        let Some(node) = nodes.get(&(p, false)) else { continue };
        let mut deps: BTreeMap<(String, usize), BTreeSet<(&str, Option<&str>)>> = BTreeMap::new();
        for (di, (to, _)) in &node.edges {
            let d = &m.deps[*di];
            // A dependency names a library (including proc macros), never a binary target.
            let Some(lib) = &r.manifests[*to].lib else { continue };
            let name = if d.key != d.package { &d.key } else { &lib.name }.replace('-', "_");
            deps.entry((name, *to)).or_default().insert((metadata_dep_kind(d), d.platform.as_deref()));
        }
        let deps: Vec<String> = deps.into_iter().map(|((name, to), kinds)| {
            let kinds: Vec<String> = kinds.into_iter().map(|(kind, target)| format!("{{\"kind\":{},\"target\":{}}}", kind, target.map(q).unwrap_or_else(|| "null".into()))).collect();
            format!("{{\"name\":{},\"pkg\":{},\"dep_kinds\":[{}]}}", q(&name), q(&ids[to]), kinds.join(","))
        }).collect();
        let features: Vec<String> = node.features.iter().map(|f| q(f)).collect();
        resolved.push(format!("{{\"id\":{},\"deps\":[{}],\"features\":[{}]}}", q(&ids[p]), deps.join(","), features.join(",")));
    }
    let resolve = if no_deps { "null".to_string() } else {
        format!("{{\"nodes\":[{}],\"root\":{}}}", resolved.join(","), root.map(|p| q(&ids[p])).unwrap_or_else(|| "null".into()))
    };
    let mut members: Vec<String> = members.iter().map(|p| q(&ids[*p])).collect();
    members.sort();
    // A relative target-dir environment value is relative to the script's virtual cwd, not
    // the real cwd inherited by this native loader process.
    let target = match ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())) {
        Some(t) => join(dir, &t),
        None => target_dir(&start)?,
    };
    Ok(format!("{{\"packages\":[{}],\"resolve\":{},\"workspace_root\":{},\"workspace_members\":[{}],\"target_directory\":{},\"version\":1}}",
        pkgs.join(","), resolve, q(&r.ws.root), members.join(","), q(&target)))
}

fn metadata_dep_kind(d: &Dep) -> &'static str {
    if d.build { "\"build\"" } else if d.dev { "\"dev\"" } else { "null" }
}

fn metadata_targets(m: &Manifest) -> Result<Vec<String>, String> {
    use crate::diag::quote as q;
    let target = |kind: &[String], types: &[String], name: &str, path: &str| format!(
        "{{\"kind\":[{}],\"crate_types\":[{}],\"name\":{},\"src_path\":{}}}",
        kind.iter().map(|s| q(s)).collect::<Vec<_>>().join(","), types.iter().map(|s| q(s)).collect::<Vec<_>>().join(","), q(name), q(path));
    let bin = vec!["bin".to_string()];
    let mut targets = Vec::new();
    if let Some(lib) = &m.lib { targets.push(target(&lib.kind, &lib.kind, &lib.name, &lib.path)); }
    for t in m.bins.iter().chain(&m.tests) { targets.push(target(&t.kind, &bin, &t.name, &t.path)); }
    // Examples and benches have no units in a normal run, but are still manifest targets.
    let t = read_toml(&format!("{}/Cargo.toml", m.dir))?;
    let pkg = table(&t, "package").unwrap();
    for (kind, folder, auto) in [("example", "examples", "autoexamples"), ("bench", "benches", "autobenches")] {
        let explicit = t.get(kind).and_then(|x| x.as_array_of_tables()).unwrap_or_default();
        let mut found = Vec::new();
        for e in explicit {
            let Some(name) = s(e, "name") else { continue };
            let path = match s(e, "path") {
                Some(path) => join(&m.dir, &path),
                None => {
                    let paths = [format!("{}/{}/{}.rs", m.dir, folder, name), format!("{}/{}/{}/main.rs", m.dir, folder, name)];
                    let Some(path) = paths.iter().find(|p| crate::vfs::is_file(p)) else { continue };
                    path.clone()
                }
            };
            let types = strs(e, "crate-type");
            found.push((name, path, if types.is_empty() { bin.clone() } else { types }));
        }
        if b(pkg, auto).unwrap_or(m.edition != "2015" || explicit.is_empty()) {
            if let Ok(entries) = crate::vfs::read_dir(format!("{}/{}", m.dir, folder)) {
                for (name, is_dir) in entries {
                    let (name, path) = if is_dir {
                        let path = format!("{}/{}/{}/main.rs", m.dir, folder, name);
                        if !crate::vfs::is_file(&path) { continue; }
                        (name, path)
                    } else if let Some(stem) = name.strip_suffix(".rs") {
                        (stem.to_string(), format!("{}/{}/{}", m.dir, folder, name))
                    } else { continue };
                    if !found.iter().any(|(n, p, _)| *n == name || *p == path) { found.push((name, path, bin.clone())); }
                }
            }
        }
        found.sort();
        for (name, path, types) in found { targets.push(target(&[kind.to_string()], &types, &name, &path)); }
    }
    if let Some(path) = &m.build {
        let stem = path.rsplit('/').next().unwrap_or("build.rs").trim_end_matches(".rs");
        targets.push(target(&["custom-build".to_string()], &bin, &format!("build-script-{stem}"), path));
    }
    Ok(targets)
}

/// The unit graph of `package` in the workspace at `workspace` (see the module comment).
pub fn unit_graph(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, flags: &[String]) -> Result<Units, String> {
    unit_graph_with(sess, workspace, package, flags, GitPlaces::run())
}

/// `unit_graph` with git dependencies kept in `git` (tests: a private home and pin file).
pub fn unit_graph_with(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, flags: &[String], git: GitPlaces) -> Result<Units, String> {
    graph_of(sess, workspace, package, &parse_flags(flags)?, git)
}

/// `makepad check`'s unit graph: its target flags are cargo check's (parse_check_flags).
pub fn check_unit_graph(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, flags: &[String]) -> Result<Units, String> {
    graph_of(sess, workspace, package, &parse_check_flags(flags)?, GitPlaces::run())
}

fn graph_of(sess: &std::sync::Arc<crate::Session>, workspace: &str, package: &str, o: &Options, git: GitPlaces) -> Result<Units, String> {
    let mut r = Resolver::with(sess, workspace, git)?;
    r.offline |= o.offline;
    let root = r.find_member(package)?;
    let rk = (root, false);
    // (the dev-dependencies are resolved for a graph with a target built with `--test`)
    let dev = o.test.is_some() || o.example.is_some() || o.check.as_ref().is_some_and(CheckSel::tests_anything);
    let mut f = Feat { r: &mut r, dev_of: dev.then_some(rk).into_iter().collect(), all_platforms: false, nodes: HashMap::new(), work: Vec::new(), order: Vec::new() };
    f.activate(rk)?;
    let m = &f.r.manifests[root];
    f.work.extend(feature_requests(&m.features, &m.implicit_features(), o).into_iter().map(|req| (rk, req)));
    f.run()?;
    let (nodes, order) = (std::mem::take(&mut f.nodes), std::mem::take(&mut f.order));
    emit(&r, &nodes, &order, root, o)
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum UKind {
    Lib,
    /// the root's lib target built with `--test` (its unit tests)
    LibTest,
    Bin(usize),
    /// the root's bin target built with `--test` (its unit tests)
    BinTest(usize),
    /// the root's integration test target
    Test(usize),
    /// the root's example (`--example`): a bin with the dev-dependencies
    Example(usize),
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
    let check = o.check.as_ref();
    for &k in order {
        let m = &r.manifests[k.0];
        if let Some(lib) = &m.lib {
            let check_lib_test = check.is_some_and(|c| c.all_tests && lib.test);
            if k == (root, false) && (o.test == Some(TestSel::Lib) || check_lib_test) {
                roots.push(add(k, UKind::LibTest, &mut units));
                // Dependencies link the plain library, not its test harness: other packages', and
                // a check's bins and integration tests (the lib itself when it is checked too).
                let check_plain = check.is_some_and(|c| c.lib || c.all_bins || !c.bins.is_empty() || c.tests_anything());
                if check_plain || nodes.iter().any(|(other, node)| *other != k && node.edges.iter().any(|(di, target)| {
                    *target == k && !r.manifests[other.0].deps[*di].build
                })) {
                    let i = add(k, UKind::Lib, &mut units);
                    if check.is_some_and(|c| c.lib) {
                        roots.push(i);
                    }
                }
            } else {
                let i = add(k, UKind::Lib, &mut units);
                if k == (root, false) && ((o.test.is_none() && check.is_none()) || check.is_some_and(|c| c.lib)) {
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
        Some(TestSel::Targets { bins, tests }) => {
            if let Some(names) = bins {
                if rm.bins.is_empty() {
                    return Err(format!("no bin targets found in package `{}`", rm.name));
                }
                for n in names {
                    if !rm.bins.iter().any(|t| &t.name == n) {
                        return Err(format!("no bin target named `{}` in package `{}`", n, rm.name));
                    }
                }
                for (bi, bt) in rm.bins.iter().enumerate() {
                    if !names.is_empty() && !names.contains(&bt.name) {
                        continue;
                    }
                    if !bt.enabled(&rn.features, !names.is_empty())? {
                        continue;
                    }
                    // `harness = false` bins (no libtest main): `--bins` builds the harness ones;
                    // `makepad test` names the targets it runs (test_pkg.rs), these included
                    if !bt.harness && names.is_empty() {
                        continue;
                    }
                    roots.push(add((root, false), UKind::BinTest(bi), &mut units));
                }
            }
            if let Some(names) = tests {
                for n in names {
                    if !rm.tests.iter().any(|t| &t.name == n) {
                        return Err(format!("no test target named `{}` in package `{}`", n, rm.name));
                    }
                }
                for (ti, tt) in rm.tests.iter().enumerate() {
                    if !names.is_empty() && !names.contains(&tt.name) {
                        continue;
                    }
                    if !tt.enabled(&rn.features, !names.is_empty())? {
                        continue;
                    }
                    // `harness = false` (cargo: `--cfg test`, no `--test`, run as a plain program):
                    // `--tests` builds the harness ones; `makepad test` names the targets it runs
                    if !tt.harness && names.is_empty() {
                        continue;
                    }
                    roots.push(add((root, false), UKind::Test(ti), &mut units));
                }
            }
        }
        _ => {}
    }
    let mut bins = Vec::new();
    if let Some(c) = check {
        if c.lib && rm.lib.is_none() {
            return Err(format!("no library targets found in package `{}`", rm.name));
        }
        if (c.all_bins || !c.bins.is_empty()) && rm.bins.is_empty() {
            return Err(format!("no bin target found in package `{}`", rm.name));
        }
        for n in &c.bins {
            if !rm.bins.iter().any(|t| &t.name == n) {
                return Err(format!("no bin target named `{}` in package `{}`", n, rm.name));
            }
        }
        for n in &c.tests {
            if !rm.tests.iter().any(|t| &t.name == n) {
                return Err(format!("no test target named `{}` in package `{}`", n, rm.name));
            }
        }
        for (bi, bt) in rm.bins.iter().enumerate() {
            let named = c.bins.contains(&bt.name);
            if (c.all_bins || named) && bt.enabled(&rn.features, named)? {
                roots.push(add((root, false), UKind::Bin(bi), &mut units));
            }
            if c.all_tests && bt.test && bt.enabled(&rn.features, false)? {
                roots.push(add((root, false), UKind::BinTest(bi), &mut units));
            }
        }
        for (ti, tt) in rm.tests.iter().enumerate() {
            let named = c.tests.contains(&tt.name);
            if ((c.all_tests && tt.test) || named) && tt.enabled(&rn.features, named)? {
                roots.push(add((root, false), UKind::Test(ti), &mut units));
            }
        }
        // the integration tests' CARGO_BIN_EXE_<name>: every bin cargo would build for them
        if c.tests_anything() {
            for bt in &rm.bins {
                if bt.enabled(&rn.features, false)? {
                    bins.push(bt.name.clone());
                }
            }
        }
    }
    if let Some(e) = &o.example {
        let rm = &r.manifests[root];
        let Some(ei) = rm.examples.iter().position(|t| &t.name == e) else {
            let names: Vec<&str> = rm.examples.iter().map(|t| t.name.as_str()).collect();
            return Err(if names.is_empty() { format!("no example targets found in package `{}`", rm.name) }
                else { format!("no example target named `{}` in package `{}` (available: {})", e, rm.name, names.join(", ")) });
        };
        let et = &rm.examples[ei];
        if et.kind.iter().all(|k| k != "example") {
            return Err(format!("example target `{}` is a library and cannot be executed", e));
        }
        et.enabled(&rn.features, true)?;
        roots.push(add((root, false), UKind::Example(ei), &mut units));
    }
    for (bi, bt) in r.manifests[root].bins.iter().enumerate() {
        if check.is_some() || o.example.is_some() {
            break;
        }
        if !bt.enabled(&rn.features, o.test.is_none() && o.bin.as_ref() == Some(&bt.name))? {
            continue;
        }
        if o.test.is_some() {
            if matches!(o.test, Some(TestSel::Targets { tests: Some(_), .. })) {
                bins.push(bt.name.clone());
            }
            continue;
        }
        if o.bin.as_ref().is_some_and(|b| b != &bt.name) {
            continue;
        }
        roots.push(add((root, false), UKind::Bin(bi), &mut units));
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
            UKind::Bin(bi) | UKind::BinTest(bi) => {
                let b = &m.bins[*bi];
                (b.kind.clone(), b.name.clone(), b.path.clone(), b.edition.clone(), if matches!(u, UKind::BinTest(_)) { "test" } else { "build" })
            }
            UKind::Example(ei) => {
                let e = &m.examples[*ei];
                (e.kind.clone(), e.name.clone(), e.path.clone(), e.edition.clone(), "build")
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
        // dev-dependencies: only the test targets' and an example's (the plain lib of a test
        // graph has none)
        let dev = matches!(u, UKind::LibTest | UKind::BinTest(_) | UKind::Test(_) | UKind::Example(_));
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
            UKind::Bin(_) | UKind::BinTest(_) | UKind::Test(_) | UKind::Example(_) => {
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
            harness: match u {
                UKind::LibTest | UKind::BinTest(_) => true,
                UKind::Test(ti) => m.tests[*ti].harness,
                _ => false,
            },
            sysroot: false,
            features: node.features.iter().cloned().collect(),
            deps,
        });
    }
    if std::env::var("MAKEPAD_RUN_TRACE").is_ok_and(|v| v.split(',').any(|x| x == "units")) {
        for (i, u) in out.iter().enumerate() {
            let (k, _) = &units[i];
            let m = &r.manifests[k.0];
            let deps: Vec<&str> = u.deps.iter().map(|(d, _)| out[*d].name.as_str()).collect();
            eprintln!("units: {} {} {} unit={} mode={} features=[{}] deps=[{}]", m.name, m.version,
                if u.host { "host" } else { "target" }, u.name, u.mode, u.features.join(","), deps.join(","));
        }
    }
    Ok(Units { units: out, roots, bins })
}

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn required_features_follow_package_feature_flags() {
        let manifest = parse_toml(r#"
[dependencies]
optional = { version = "1", optional = true }
hidden = { version = "1", optional = true }
[features]
default = ["a"]
a = ["b"]
b = ["a"]
manual = []
forwarded = ["dep:hidden", "optional?/foreign"]
strong = ["optional/foreign"]
[[test]]
name = "gated"
required-features = ["b", "manual"]
"#).unwrap().root;
        let definitions = table(&manifest, "features").unwrap();
        let features = definitions.keys().map(|k| (k.clone(), strs(definitions, k))).collect();
        let implicit = BTreeSet::from(["optional".to_string()]);
        let target = &manifest.get("test").unwrap().as_array_of_tables().unwrap()[0];
        let target = Target {
            name: s(target, "name").unwrap(), path: String::new(), edition: "2021".into(),
            kind: vec!["test".into()], required_features: strs(target, "required-features"), harness: true, test: true, doctest: false,
        };
        let enabled = |flags: &[&str]| {
            let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
            enabled_features(&features, &implicit, &parse_flags(&flags).unwrap()).unwrap()
        };
        assert_eq!(enabled(&[]), BTreeSet::from(["default".into(), "a".into(), "b".into()]));
        assert!(enabled(&["--no-default-features"]).is_empty());
        assert!(!target.enabled(&enabled(&[]), false).unwrap());
        assert_eq!(target.enabled(&enabled(&[]), true).unwrap_err(),
            "target `gated` requires the features: `b`, `manual`\nConsider enabling them by passing, e.g., `--features=\"b manual\"`");
        for flags in [
            vec!["--features", "manual"],
            vec!["-F", "manual"],
            vec!["--features=manual"],
            vec!["-Fmanual"],
            vec!["--no-default-features", "--features", "a,manual"],
            vec!["--no-default-features", "-F", "a\tmanual"],
            vec!["--no-default-features", "-F", "a", "--features", "manual"],
            vec!["--no-default-features", "--all-features"],
        ] {
            let set = enabled(&flags);
            assert!(target.enabled(&set, false).unwrap(), "{flags:?}");
            assert!(target.enabled(&set, true).unwrap(), "{flags:?}");
        }
        assert!(!target.enabled(&enabled(&["--no-default-features", "-F", "manual"]), false).unwrap());
        assert_eq!(enabled(&["--no-default-features", "-F", "forwarded"]), BTreeSet::from(["forwarded".into()]));
        assert_eq!(enabled(&["--no-default-features", "-F", "strong"]), BTreeSet::from(["strong".into(), "optional".into()]));
        assert_eq!(enabled(&["--no-default-features", "-F", "optional/foreign"]), BTreeSet::from(["optional".into()]));
        let all = enabled(&["--all-features"]);
        assert!(all.contains("default") && all.contains("optional"));
        assert!(!all.contains("hidden") && !all.contains("foreign"));
        let mut bin = target.clone();
        bin.kind = vec!["bin".into()];
        bin.harness = false;
        assert!(!bin.enabled(&enabled(&[]), false).unwrap());
        assert!(bin.enabled(&all, true).unwrap());
        bin.required_features.clear();
        assert!(bin.enabled(&BTreeSet::new(), false).unwrap());
        assert!(bin.enabled(&BTreeSet::new(), true).unwrap());
    }

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
    fn resolver_2_host_features() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree("host-features", &[
            ("Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\nexclude = [\"macros\"]\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na-macro = { path = \"../macros/a\" }\nb-macro = { path = \"../macros/b\" }\nshared = { path = \"../macros/shared\", optional = true, default-features = false, features = [\"target\"] }\n[build-dependencies]\nshared = { path = \"../macros/shared\", default-features = false, features = [\"build\"] }\n[dev-dependencies]\nshared = { path = \"../macros/shared\", features = [\"dev\"] }\n[features]\ntarget-side = [\"dep:shared\"]\n"),
            ("app/src/main.rs", "fn main() {}\n"),
            ("app/build.rs", "fn main() {}\n"),
            ("macros/Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"a\", \"b\", \"shared\", \"optional\", \"unused\"]\n[workspace.dependencies]\nshared = { path = \"shared\", default-features = false, features = [\"parsing\"] }\n"),
            ("macros/a/Cargo.toml", "[package]\nname = \"a-macro\"\nversion = \"0.1.0\"\n[lib]\nproc-macro = true\n[dependencies]\nshared = { workspace = true, features = [\"full\"] }\n"),
            ("macros/a/src/lib.rs", ""),
            ("macros/b/Cargo.toml", "[package]\nname = \"b-macro\"\nversion = \"0.1.0\"\n[lib]\nproc-macro = true\n[dependencies]\nshared = { workspace = true, features = [\"visit\"] }\n[features]\ndefault = [\"shared/extra-traits\", \"shared?/fold\"]\n"),
            ("macros/b/src/lib.rs", ""),
            ("macros/shared/Cargo.toml", "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n[dependencies]\noptional = { path = \"../optional\", optional = true, default-features = false }\nunused = { path = \"../unused\", optional = true }\n[features]\ndefault = [\"std\"]\nstd = []\nparsing = []\nfull = [\"optional?/walk\", \"unused?/walk\"]\nvisit = [\"dep:optional\"]\nextra-traits = []\nfold = []\nbuild = []\ndev = []\ntarget = []\n"),
            ("macros/shared/src/lib.rs", ""),
            ("macros/optional/Cargo.toml", "[package]\nname = \"optional\"\nversion = \"0.1.0\"\n[features]\nwalk = []\n"),
            ("macros/optional/src/lib.rs", ""),
            ("macros/unused/Cargo.toml", "[package]\nname = \"unused\"\nversion = \"0.1.0\"\n[features]\nwalk = []\n"),
            ("macros/unused/src/lib.rs", ""),
        ]);
        // Cargo reference features.md#feature-resolver-version-2: union on each side, no dev
        // features for a build. rustc compiler/rustc_attr_parsing/src/attributes/cfg.rs::eval_config_entry
        // tests the resulting feature cfg in this compilation's session.
        for target in [false, true] {
            let flags = if target { vec!["--features".to_string(), "target-side".to_string()] } else { Vec::new() };
            let g = unit_graph(sess, &root, "app", &flags).unwrap();
            let shared: Vec<_> = g.units.iter().filter(|u| u.name == "shared").collect();
            assert_eq!(shared.len(), if target { 2 } else { 1 });
            let host = shared.iter().find(|u| u.host).unwrap();
            assert_eq!(host.features, ["build", "extra-traits", "fold", "full", "parsing", "visit"]);
            let optional = &g.units[host.deps.iter().find(|(_, n)| n == "optional").unwrap().0];
            assert!(optional.host);
            assert_eq!(optional.features, ["walk"]);
            assert!(!g.units.iter().any(|u| u.name == "unused"));
            assert_eq!(g.units.iter().filter(|u| u.kind.iter().any(|k| k == "proc-macro")).count(), 2);
            for pm in g.units.iter().filter(|u| u.kind.iter().any(|k| k == "proc-macro")) {
                assert!(pm.host);
                let dep = &g.units[pm.deps.iter().find(|(_, n)| n == "shared").unwrap().0];
                assert!(dep.host);
                assert_eq!(dep.features, host.features);
            }
            if target {
                let app = shared.iter().find(|u| !u.host).unwrap();
                assert_eq!(app.features, ["target"]);
                assert!(app.deps.is_empty());
            }
        }
        // A git member inherits from its repository's workspace too, with member features added.
        let mut r = Resolver::new(sess, &root).unwrap();
        let source = Source::Git(GitSrc { url: "https://example.invalid/macros".to_string(), reference: crate::git::fetch::GitRef::DefaultBranch });
        let i = r.load(&format!("{root}/macros/b"), source).unwrap();
        let d = &r.manifests[i].deps[0];
        assert_eq!(d.features, ["parsing", "visit"]);
        assert!(!d.default_features);
        assert_eq!(d.path.as_deref(), Some(normalize(&format!("{root}/macros/shared")).as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `makepad check`'s target flags are cargo check's (cargo compile_filter.rs): `--tests` is
    /// every target with `test = true` built with `--test` -- the lib's and the bins' unit tests
    /// and the integration tests, a `test = false` bin left out -- beside the plain lib they link,
    /// with the dev-dependencies; `--lib` and `--bins` are the plain targets, without them. A
    /// `makepad check --tests` had checked the integration tests only, and so reported none of
    /// the bins' unit tests' lints (`unused_mut` in a binary's unit tests, cargo check's 11-05).
    #[test]
    fn check_target_flags_are_cargo_checks() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree("check-targets", &[
            ("Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[[bin]]\nname = \"tool\"\ntest = false\n[dev-dependencies]\nhelper = { path = \"../helper\" }\n"),
            ("app/src/lib.rs", ""),
            ("app/src/main.rs", "fn main() {}\n"),
            ("app/src/bin/tool.rs", "fn main() {}\n"),
            ("app/tests/it.rs", ""),
            ("app/tests/other.rs", ""),
            ("helper/Cargo.toml", "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n"),
            ("helper/src/lib.rs", ""),
        ]);
        // each root: target name and mode; then whether the dev-dependency is in the graph
        let check = |flags: &[&str]| -> (Vec<String>, bool) {
            let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
            let g = check_unit_graph(sess, &root, "app", &flags).unwrap_or_else(|e| panic!("{flags:?}: {e}"));
            let mut r: Vec<String> = g.roots.iter().map(|&i| format!("{} {}", g.units[i].name, g.units[i].mode)).collect();
            r.sort();
            (r, g.units.iter().any(|u| u.name == "helper"))
        };
        assert_eq!(check(&["--tests"]), (vec!["app test".to_string(), "app test".to_string(), "it test".to_string(), "other test".to_string()], true));
        assert_eq!(check(&["--test", "it"]), (vec!["it test".to_string()], true));
        assert_eq!(check(&["--lib"]), (vec!["app build".to_string()], false));
        assert_eq!(check(&["--bins"]), (vec!["app build".to_string(), "tool build".to_string()], false));
        assert_eq!(check(&["--bin", "tool", "--tests"]), (vec!["app test".to_string(), "app test".to_string(), "it test".to_string(), "other test".to_string(), "tool build".to_string()], true));
        assert_eq!(check(&["--lib", "--tests"]), (vec!["app build".to_string(), "app test".to_string(), "app test".to_string(), "it test".to_string(), "other test".to_string()], true));
        // no target flag: the lib and every bin, as before
        assert_eq!(check(&[]), (vec!["app build".to_string(), "app build".to_string(), "tool build".to_string()], false));
        // the lib's and the bin's unit tests and the integration tests link the plain lib
        let g = check_unit_graph(sess, &root, "app", &["--tests".to_string()]).unwrap();
        let plain = g.units.iter().position(|u| u.name == "app" && u.mode == "build" && u.kind.iter().any(|k| k == "lib")).expect("the plain lib");
        for &i in &g.roots {
            let u = &g.units[i];
            if u.kind.iter().any(|k| k == "lib") { continue; }
            assert!(u.deps.iter().any(|(d, _)| *d == plain), "{} links the plain lib", u.name);
        }
        // makepad test's flags stay test programs: `--lib` is the lib's unit tests
        let g = unit_graph(sess, &root, "app", &["--lib".to_string()]).unwrap();
        assert_eq!(g.roots.iter().map(|&i| g.units[i].mode.as_str()).collect::<Vec<_>>(), ["test"]);
        let e = check_unit_graph(sess, &root, "app", &["--test".to_string(), "nope".to_string()]).err().unwrap();
        assert!(e.contains("no test target named `nope`"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--example <name>` (cargo run --example): the example is the root, a bin that links the
    /// package's lib and its dev-dependencies (the plain lib has none); explicit `[[example]]`
    /// targets and discovered examples/*.rs, examples/*/main.rs; a library example, an unknown
    /// name and another target flag beside it are errors.
    #[test]
    fn an_example_runs_as_a_bin_with_the_dev_dependencies() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree("examples", &[
            ("Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[[example]]\nname = \"named\"\npath = \"demo/named.rs\"\n[[example]]\nname = \"plugin\"\ncrate-type = [\"cdylib\"]\n[dev-dependencies]\nhelper = { path = \"../helper\" }\n"),
            ("app/src/lib.rs", ""),
            ("app/src/main.rs", "fn main() {}\n"),
            ("app/examples/smoke.rs", "fn main() {}\n"),
            ("app/examples/multi/main.rs", "fn main() {}\n"),
            ("app/examples/plugin.rs", ""),
            ("app/demo/named.rs", "fn main() {}\n"),
            ("helper/Cargo.toml", "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n"),
            ("helper/src/lib.rs", ""),
        ]);
        let graph = |flags: &[&str]| unit_graph(sess, &root, "app", &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>());
        for (name, path) in [("smoke", "examples/smoke.rs"), ("multi", "examples/multi/main.rs"), ("named", "demo/named.rs")] {
            for flags in [vec!["--example", name], vec![]] {
                let flags: Vec<String> = if flags.is_empty() { vec![format!("--example={name}")] } else { flags.iter().map(|f| f.to_string()).collect() };
                let g = unit_graph(sess, &root, "app", &flags).unwrap_or_else(|e| panic!("{flags:?}: {e}"));
                let ex: Vec<usize> = g.roots.iter().copied().filter(|&i| g.units[i].kind.iter().any(|k| k == "example")).collect();
                assert_eq!(ex.len(), 1, "{flags:?}: one example root");
                let u = &g.units[ex[0]];
                assert_eq!((u.name.as_str(), u.mode.as_str()), (name, "build"));
                assert!(u.src_path.replace('\\', "/").ends_with(path), "{}", u.src_path);
                // no bin of the package is a root
                assert!(!g.roots.iter().any(|&i| g.units[i].kind.iter().any(|k| k == "bin")), "{flags:?}");
                let lib = g.units.iter().position(|x| x.name == "app" && x.kind.iter().any(|k| k == "lib")).expect("the lib");
                let helper = g.units.iter().position(|x| x.name == "helper").expect("the dev-dependency");
                assert!(u.deps.iter().any(|(d, _)| *d == lib) && u.deps.iter().any(|(d, _)| *d == helper), "{name} links the lib and the dev-dependency");
                assert!(!g.units[lib].deps.iter().any(|(d, _)| *d == helper), "the plain lib has no dev-dependency");
            }
        }
        // a plain run: the bin, without the dev-dependencies
        let g = graph(&[]).unwrap();
        assert!(!g.units.iter().any(|u| u.name == "helper" || u.kind.iter().any(|k| k == "example")));
        let e = |flags: &[&str]| graph(flags).err().unwrap_or_else(|| panic!("{flags:?}: no error"));
        assert!(e(&["--example", "plugin"]).contains("is a library and cannot be executed"));
        let unknown = e(&["--example", "nope"]);
        assert!(unknown.contains("no example target named `nope`") && unknown.contains("smoke"), "{unknown}");
        assert!(e(&["--example", "smoke", "--bin", "app"]).contains("takes no other target flag"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A registry package's feature value `x/f` naming one of its dev-dependencies (vsss-rs 6.0.1:
    /// `alloc = ["rand/alloc"]` with rand a dev-dependency): cargo accepts it and enables nothing
    /// (cargo core/resolver/features.rs activate_dep_feature: no activated dependency `x`).
    #[test]
    fn registry_feature_on_a_dev_dependency() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree("devfeat", &[
            ("Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"app\"]\n"),
            (".cargo/config.toml", "[source.crates-io]\nreplace-with = \"v\"\n[source.v]\ndirectory = \"vendor\"\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nshares = \"6\"\n"),
            ("app/src/lib.rs", ""),
            ("vendor/shares/Cargo.toml", "[package]\nname = \"shares\"\nversion = \"6.0.1\"\nedition = \"2021\"\n[features]\ndefault = [\"alloc\"]\nalloc = [\"rand/alloc\"]\nstd = [\"alloc\", \"rand/std\"]\n[dev-dependencies]\nrand = \"0.8\"\n[target.'cfg(unix)'.dev-dependencies]\nrand_unix = \"0.1\"\n"),
            ("vendor/shares/.cargo-checksum.json", "{\"files\":{},\"package\":null}"),
            ("vendor/shares/src/lib.rs", ""),
        ]);
        let g = unit_graph(sess, &root, "app", &[]).unwrap_or_else(|e| panic!("{e}"));
        let shares = g.units.iter().find(|u| u.name == "shares").unwrap();
        assert_eq!(shares.features, ["alloc", "default"]);
        assert!(!g.units.iter().any(|u| u.name == "rand"));
        // a value naming neither a dependency nor a dev-dependency stays an error
        let _ = std::fs::write(format!("{root}/vendor/shares/Cargo.toml"), "[package]\nname = \"shares\"\nversion = \"6.0.1\"\nedition = \"2021\"\n[features]\ndefault = [\"nope/alloc\"]\n");
        let e = unit_graph(sess, &root, "app", &[]).err().unwrap();
        assert!(e.contains("names no dependency"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A member manifest that does not parse is the answer when no member is the package: its
    /// file, line, column and message, not "not a member of the workspace". Another broken member
    /// does not hide a package that loads.
    #[test]
    fn a_member_manifest_that_does_not_parse_is_reported() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = tree("broken-member", &[
            ("Cargo.toml", "[workspace]\nmembers = [\"slice\", \"other\"]\n"),
            ("slice/Cargo.toml", "[package]\nname = \"fab-slice\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\nsdf = { path = \"../other\" }\nsdf = { path = \"../other\" }\n"),
            ("slice/src/lib.rs", ""),
            ("other/Cargo.toml", "[package]\nname = \"other\"\nversion = \"0.1.0\"\n"),
            ("other/src/lib.rs", ""),
        ]);
        let mut r = Resolver::new(sess, &root).unwrap();
        let e = r.find_member("fab-slice").err().unwrap();
        let path = format!("{}/slice/Cargo.toml", normalize(&root));
        assert!(e.starts_with(&format!("{path}:7:1: duplicate key `sdf`\n")), "{e}");
        assert!(!e.contains("not a member"), "{e}");
        assert!(r.find_member("other").is_ok());
        let _ = std::fs::remove_dir_all(&root);
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
fn git_sources_as_cargo_spells_them() {
    use crate::git::fetch::GitRef;
    assert_eq!(canonical_url("https://github.com/Foo/Bar.git/"), "https://github.com/foo/bar");
    assert_eq!(canonical_url("https://example.com/Foo/bar.git"), "https://example.com/Foo/bar");
    let c = "0123456789abcdef0123456789abcdef01234567";
    let (g, commit) = parse_git_source(&format!("git+https://github.com/a/b?branch=feature%2Fx#{c}")).unwrap();
    assert_eq!((g.reference.clone(), crate::gitstore::hex(&commit)), (GitRef::Branch("feature/x".into()), c.to_string()));
    assert!(g.same(&GitSrc { url: "https://github.com/A/b.git".into(), reference: GitRef::Branch("feature/x".into()) }));
    assert_eq!(parse_git_source(&format!("git+https://h/r#{c}")).unwrap().0.reference, GitRef::DefaultBranch);
    assert_eq!(parse_git_source(&format!("git+https://h/r?rev=abc#{c}")).unwrap().0.id(), "git+https://h/r?rev=abc");
    assert!(parse_git_source("registry+https://github.com/rust-lang/crates.io-index").is_none());
    let t = |text: &str| parse_toml(text).unwrap().root;
    assert!(git_src(&t("git = \"https://h/r\"\nbranch = \"a\"\ntag = \"b\"\n"), "x", "d").is_err());
    assert!(git_src(&t("git = \"https://h/r\"\npath = \"p\"\n"), "x", "d").is_err());
    assert!(git_src(&t("git = \"git@github.com:a/b\"\n"), "x", "d").unwrap_err().contains("https only"));
    assert_eq!(git_src(&t("git = \"https://h/r\"\ntag = \"v1\"\n"), "x", "d").unwrap().unwrap().reference, GitRef::Tag("v1".into()));
    // cargo's package discovery: root first, depth-first by name; hidden folders, target folders
    // beside a manifest and nested repositories skipped
    let e = |p: &str| crate::gitstore::Entry { path: p.into(), mode: 0o100644, oid: [0; 20] };
    let table: Vec<_> = ["Cargo.toml", "a-b/Cargo.toml", "a/Cargo.toml", "a/target/x/Cargo.toml", "a/z/Cargo.toml", ".github/Cargo.toml", "n/.git", "n/m/Cargo.toml", "target/Cargo.toml", "xCargo.toml"].iter().map(|p| e(p)).collect();
    assert_eq!(manifest_dirs(&table), vec!["", "a", "a/z", "a-b"]);
}

#[cfg(test)]
#[test]
fn file_macro_paths_as_cargo_names_them() {
    // a workspace root spelt with dotted `workspace.` keys (as makepad's), a member, a package
    // in Relax's registry store under the target folder, and a package outside the root
    let base = std::env::temp_dir().join(format!("relax_file_root_{}", std::process::id()));
    let ws = base.join("ws");
    std::fs::create_dir_all(ws.join("m/src")).unwrap();
    std::fs::write(ws.join("Cargo.toml"), "workspace.members = [\"m\"]\nworkspace.resolver = \"2\"\n\n[workspace.dependencies]\n").unwrap();
    std::fs::write(ws.join("m/Cargo.toml"), "[package]\nname = \"m\"\nversion = \"0.1.0\"\n").unwrap();
    let ws = normalize(&ws.display().to_string());
    let registry = format!("{}/target/makepad-run/registry", ws);
    let r = FileRoot { root: find_root(&format!("{}/m", ws)).unwrap(), not_path: vec![registry.clone()] };
    assert_eq!(r.relative(&format!("{}/m/src/main.rs", ws)).as_deref(), Some("m/src/main.rs"));
    assert_eq!(r.relative(&format!("{}/src/foo-1.0.0-0123456789abcdef/src/lib.rs", registry)), None);
    assert_eq!(r.relative(&format!("{}/other/src/lib.rs", normalize(&base.display().to_string()))), None);
    assert_eq!(FileRoot::of(&format!("{}/m", ws)).map(|r| r.root), Some(ws));
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(test)]
#[test]
fn unit_graph_host_side_follows_proc_macro_and_build_script_edges() {
    // A profile that optimizes build dependencies and leaves a target dependency unoptimized:
    // the side of a unit comes from the proc-macro and build-script edges, not from opt_level.
    let unit = |kind: &str, opt: &str, deps: &[usize]| {
        let deps = deps.iter().map(|d| format!("{{\"index\":{d},\"extern_crate_name\":\"d{d}\"}}")).collect::<Vec<_>>().join(",");
        format!("{{\"pkg_id\":\"p\",\"target\":{{\"kind\":[\"{kind}\"],\"name\":\"n\",\"src_path\":\"s.rs\",\"edition\":\"2021\"}},\"profile\":{{\"opt_level\":\"{opt}\"}},\"mode\":\"build\",\"features\":[],\"dependencies\":[{deps}]}}")
    };
    let units = [
        unit("bin", "0", &[1, 2]),
        unit("lib", "0", &[]),
        unit("proc-macro", "3", &[3]),
        unit("lib", "3", &[]),
        unit("custom-build", "3", &[5]),
        unit("lib", "0", &[]),
    ];
    let json = format!("{{\"units\":[{}],\"roots\":[0]}}", units.join(","));
    let graph = Units::from_json(json.as_bytes()).unwrap();
    assert_eq!(graph.units.iter().map(|u| u.host).collect::<Vec<_>>(), [false, false, true, true, true, true]);
}

#[cfg(test)]
#[test]
fn run_env_reads_the_packages_table() {
    let m = "[package]\nname = \"makepad-amp\"\n\n[package.metadata.makepad.run-env]\n# why\nMAKEPAD_LOG = \"0\"\nbad key = \"x\"\n\n[dependencies]\nX = \"1\"\n";
    assert_eq!(run_env(m), vec![("MAKEPAD_LOG".to_string(), "0".to_string())]);
    assert!(run_env("[package]\nname = \"a\"\n").is_empty());
}

#[cfg(test)]
#[test]
fn test_false_targets_run_only_when_named() {
    let root = std::env::temp_dir().join(format!("relax-test-false-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("Cargo.toml"), r#"[package]
name = "quiet"
version = "0.1.0"
edition = "2021"
[lib]
path = "src/lib.rs"
test = false
doctest = false
[[bin]]
name = "quiet_bin"
path = "src/quiet_bin.rs"
test = false
[[test]]
name = "quiet_test"
path = "tests/quiet_test.rs"
test = false
"#).unwrap();
    for f in ["src/lib.rs", "src/quiet_bin.rs", "tests/quiet_test.rs", "tests/loud.rs"] { std::fs::write(root.join(f), "").unwrap(); }
    let sess = std::sync::Arc::new(crate::Session::default());
    let dir = root.to_str().unwrap();
    let names = |t: &[(String, String, bool)]| t.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
    let (lib, bins, tests) = test_targets(&sess, dir, "quiet", &[], &[], &[]).unwrap();
    let lib = lib.unwrap();
    assert!(!lib.test && !lib.doctest);
    assert!(bins.is_empty(), "{bins:?}");
    assert_eq!(names(&tests), ["loud"]);
    let (_, bins, tests) = test_targets(&sess, dir, "quiet", &[], &["quiet_bin".to_string()], &["quiet_test".to_string()]).unwrap();
    assert_eq!(names(&bins), ["quiet_bin"]);
    assert_eq!(names(&tests), ["loud", "quiet_test"]);
    let _ = std::fs::remove_dir_all(&root);
}
