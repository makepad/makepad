//! Inputs whose identity cannot be replaced by a live body transaction. Snapshot before
//! loading, so a checkout during load/parse is still observed when the watcher starts.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) type Stamp = Option<(std::time::SystemTime, u64)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Source,
    SourceDirectory(String),
    Provided,
    Build,
    Manifest,
    Std,
    /// a source of the running makepad binary itself (cargo's dep-info of the loader)
    Runner,
}

/// How the app's Cargo graph was resolved at boot, and what it resolved to.
#[derive(Clone)]
struct ResolvedGraph {
    workspace: String,
    package: String,
    flags: Vec<String>,
    shape: u64,
}

#[derive(Default, Clone)]
pub struct Inputs {
    pub(super) paths: BTreeMap<String, (Stamp, Kind)>,
    graph: Option<ResolvedGraph>,
    /// the folders of the graph's packages, downloaded ones aside
    packages: BTreeSet<String>,
    /// the checkouts the inputs are in, each with the commit its HEAD named when they were
    /// stamped (the workspace's own first: Repositories::heads), and when (unix ms): the
    /// sources the run reads are these (None outside git, or during a git operation)
    pub(super) heads: Option<(Vec<(String, String)>, u64)>,
}

/// Whether `path` lies in another OS's source tree (`src/backend/linux/..`, `src/os/windows/..`):
/// such a file is no input of this app. Build scripts declare whole crates as their inputs, so
/// cargo's dep-info lists these too; only the directory names tell them apart.
fn other_os(path: &Path) -> bool {
    const OURS: &[&str] = if cfg!(target_vendor = "apple") { &["apple", "macos", "ios"] }
        else if cfg!(target_os = "windows") { &["windows"] }
        else if cfg!(target_os = "android") { &["android", "linux"] }
        else { &["linux"] };
    path.iter().filter_map(|c| c.to_str()).skip_while(|c| *c != "src")
        .any(|c| matches!(c, "apple" | "macos" | "ios" | "linux" | "windows" | "android") && !OURS.contains(&c))
}

/// What a resolved graph is: its units (packages with their versions, targets, features and
/// dependency edges). A lockfile or manifest change that leaves this alone is not this app's.
fn graph_shape(units: &crate::unitgraph::Units) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{units:?}").hash(&mut h);
    h.finish()
}

impl Inputs {
    fn insert(&mut self, path: &Path, kind: Kind) {
        if other_os(path) { return; }
        let path = path.to_string_lossy().into_owned();
        let stamp = super::stamp(&path);
        self.paths.insert(path, (stamp, kind));
    }

    fn protected(&mut self, path: &Path, root: &Path, kind: Kind) {
        self.insert(path, kind.clone());
        // Watch directories below a package root: unrelated root-file replacements must not
        // restart makepad. Std retains its whole-tree watch. Outside includes get no parents.
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|p| p.starts_with(root) && (*p != root || kind == Kind::Std)) {
            self.insert(dir, if kind == Kind::Manifest { Kind::Provided } else { kind.clone() });
            parent = dir.parent();
        }
    }

    pub fn collect(graph: &crate::unitgraph::Graph, std_dir: &str, workspace: &str) -> Self {
        let mut inputs = Self::default();
        let mut dirs = BTreeSet::new();
        // A provided crate's source identity includes its compiled dependencies too.
        let mut provided = BTreeSet::new();
        let mut pending: Vec<_> = graph.pkg.iter().enumerate().filter(|(_, (_, name, _))| crate::provided::registered(name)).map(|(i, _)| i).collect();
        let mut seen = BTreeSet::new();
        while let Some(i) = pending.pop() {
            if !seen.insert(i) { continue; }
            if let Some((dir, _, _)) = graph.pkg.get(i) { provided.insert(dir.clone()); }
            if let Some(krate) = graph.crates.get(i) { pending.extend(krate.deps.iter().map(|d| d.0)); }
        }
        for (dir, _, _) in graph.pkg.iter().chain(graph.raw.iter().map(|u| &u.pkg)) {
            if dir.is_empty() || dir.contains("/.cargo/registry/") || dir.contains("/.cargo/git/") || !dirs.insert(dir) { continue; }
            let provided = provided.contains(dir);
            let files = if provided { crate::srchash::compiled_files(Path::new(dir)) } else {
                let mut files = Vec::new();
                crate::srchash::files(Path::new(dir), "", &mut files);
                files
            };
            for (rel, path) in files {
                let kind = if rel == "Cargo.toml" || (!provided && rel == "Cargo.lock") {
                    Kind::Manifest
                } else if provided {
                    Kind::Provided
                } else if path.extension().is_some_and(|x| x == "rs") {
                    Kind::Source
                } else { continue };
                if provided { inputs.protected(&path, Path::new(dir), kind); }
                else { inputs.insert(&path, kind); }
            }
            if provided {
                let build = Path::new(dir).join("build.rs");
                if crate::vfs::is_file(&build) { inputs.insert(&build, Kind::Provided); }
            } else {
                // Include an absent lockfile too: creating it changes the resolved graph.
                inputs.insert(&Path::new(dir).join("Cargo.lock"), Kind::Manifest);
                inputs.discover_source_dir(Path::new(dir), dir, &mut Vec::new());
            }
        }
        inputs.packages = dirs.into_iter().cloned().collect();
        for script in &graph.build_scripts {
            inputs.insert(Path::new(&script.src_path), Kind::Build);
        }
        for name in ["Cargo.toml", "Cargo.lock"] {
            inputs.insert(&Path::new(workspace).join(name), Kind::Manifest);
        }
        let mut files = Vec::new();
        crate::srchash::files(Path::new(std_dir), "", &mut files);
        for (_, path) in files { inputs.protected(&path, Path::new(std_dir), Kind::Std); }
        let mut repos = super::git::Repositories::default();
        repos.include(inputs.paths.keys().map(String::as_str));
        inputs.heads = repos.heads(workspace).map(|h| (h, super::unix_ms(std::time::SystemTime::now()) as u64));
        inputs
    }

    /// Directory stamps catch new modules even before a `mod` declaration is
    /// saved. Walk children only when discovering a directory; later scans read
    /// just the directories whose identities changed, including empty ones.
    fn discover_source_dir(&mut self, dir: &Path, package: &str, added: &mut Vec<String>) {
        if other_os(dir) { return; }
        let path = dir.to_string_lossy().into_owned();
        if !self.paths.contains_key(&path) {
            self.insert(dir, Kind::SourceDirectory(package.into()));
            added.push(path);
        }
        let Ok(entries) = crate::vfs::read_dir(dir) else { return };
        for (name, is_dir) in entries {
            if name.starts_with('.') || name == "target" { continue; }
            let child = dir.join(name);
            if other_os(&child) { continue; }
            let path = child.to_string_lossy().into_owned();
            if self.paths.contains_key(&path) { continue; }
            if is_dir {
                if !crate::vfs::is_file(child.join("Cargo.toml")) {
                    self.discover_source_dir(&child, package, added);
                }
            } else if child.extension().is_some_and(|e| e == "rs") {
                self.insert(&child, Kind::Source);
                added.push(path);
            }
        }
    }

    pub(super) fn discover_sources<'a>(&mut self, changed: impl Iterator<Item = &'a str>) -> Vec<String> {
        let dirs: Vec<_> = changed.filter_map(|path| match self.paths.get(path).map(|p| &p.1) {
            Some(Kind::SourceDirectory(package)) if self.paths[path].0 != super::stamp(path) => Some((path.to_string(), package.clone())),
            _ => None,
        }).collect();
        let mut added = Vec::new();
        for (dir, package) in dirs {
            self.paths.get_mut(&dir).unwrap().0 = super::stamp(&dir);
            self.discover_source_dir(Path::new(&dir), &package, &mut added);
        }
        added
    }

    /// The graph `units` the app was resolved to (`graph_moved` resolves it again).
    pub fn set_graph(&mut self, workspace: &str, package: &str, flags: &[String], units: &crate::unitgraph::Units) {
        self.graph = Some(ResolvedGraph { workspace: workspace.into(), package: package.into(), flags: flags.to_vec(), shape: graph_shape(units) });
    }

    /// Whether `path` is a manifest or lockfile of the app's graph.
    pub(super) fn is_manifest(&self, path: &str) -> bool {
        matches!(self.paths.get(path).map(|x| &x.1), Some(Kind::Manifest))
    }

    /// Whether the app's resolved graph differs from the one it runs (resolved again from the
    /// manifests and the lockfile as they are now; one that does not resolve differs).
    pub(super) fn graph_moved(&self, sess: &std::sync::Arc<crate::Session>) -> bool {
        let Some(g) = &self.graph else { return true };
        crate::unitgraph::unit_graph(sess, &g.workspace, &g.package, &g.flags).map_or(true, |units| graph_shape(&units) != g.shape)
    }

    /// The loader binary's own sources ([`runner_paths`]). Outside the target directory only. A
    /// file newer than the binary has no stamp: the binary is already older than it, so the first
    /// scan sees it as changed. Read once per process.
    pub(super) fn add_runner(&mut self) {
        static RUNNER: std::sync::OnceLock<Vec<(String, Stamp)>> = std::sync::OnceLock::new();
        let workspace = self.graph.as_ref().map(|g| g.workspace.clone()).unwrap_or_else(crate::run::workspace_dir);
        let runner = RUNNER.get_or_init(|| {
            let _k = crate::zone_alloc::kept();
            let Some(exe) = super::host::loader() else { return Vec::new() };
            let built = std::fs::metadata(&exe).and_then(|m| m.modified()).ok();
            let target = super::host::target_dir(&exe);
            let picked = super::host::picked(&exe);
            let mut out = Vec::new();
            for path in runner_paths(&exe, &workspace) {
                if target.as_ref().is_some_and(|t| Path::new(&path).starts_with(t)) { continue; }
                // the devhub linked into the binary (its CLI) is no part of a running app: no
                // "Update ready" on every app for a devhub landing
                if path.contains("/libs/devhub/") { continue; }
                // a file newer than this binary: an edit it was built before, so its first look
                // says changed. Not for a binary picked by the content of its inputs: there the
                // time only says a checkout rewrote the file (CI flips a tree between pins, and
                // every leg offered a restart for sources its JIT was built from)
                let stamp = super::stamp(&path).filter(|s| picked || built.is_none_or(|b| s.0 <= b));
                out.push((path, stamp));
            }
            out
        });
        for (path, stamp) in runner {
            if other_os(Path::new(path)) { continue; }
            self.paths.entry(path.clone()).or_insert((*stamp, Kind::Runner));
        }
    }

    /// The inputs whose change needs a new makepad process (not a patch): provided crates,
    /// std, makepad's own sources, the Cargo graph and build scripts.
    pub(super) fn process_paths(&self, keep: &dyn Fn(&str) -> bool) -> Vec<String> {
        self.paths.iter().filter(|(p, (_, k))| !matches!(k, Kind::Source | Kind::SourceDirectory(_)) && keep(p)).map(|(p, _)| p.clone()).collect()
    }

    /// The folder workspace-relative paths start at (the graph's workspace, else the run's).
    pub(super) fn workspace(&self) -> String {
        crate::vfs::normalize_path(&self.graph.as_ref().map(|g| g.workspace.clone()).unwrap_or_else(crate::run::workspace_dir))
    }

    /// Files that can produce a reload event: unloaded sources are read only when loaded.
    /// Keep absent inputs too (a deleted source or a lockfile that has not been created yet).
    /// `keep` picks paths before any is looked at (a live session asks about its few saves).
    pub(super) fn source_paths(&self, loaded: impl Iterator<Item = String>, keep: &dyn Fn(&str) -> bool) -> Vec<String> {
        let workspace = self.workspace();
        let roots: Vec<_> = Path::new(&workspace).ancestors().map(|p| p.to_string_lossy().into_owned()).collect();
        let paths = self.process_paths(keep).into_iter().chain(loaded).chain(crate::native_macros::paths().into_iter().filter(|p| keep(p)));
        paths.filter(|p| !crate::vfs::is_dir(p)).map(|p| {
            // Dependencies outside the workspace use ../; another drive has no relative path.
            roots.iter().enumerate().find_map(|(up, root)| {
                crate::vfs::relative_path(&p, root).map(|rel| format!("{}{rel}", "../".repeat(up)))
            }).unwrap_or_else(|| crate::vfs::normalize_path(&p))
        }).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// Whether a workspace-relative path lies in no package of the app's graph, nor in std: no
    /// crate's own folder holds it (a lane's scratch file in its worktree).
    pub(super) fn outside_packages(&self, rel: &str, std_dir: &str) -> bool {
        let path = format!("{}/{rel}", self.workspace());
        !self.packages.iter().map(String::as_str).chain([std_dir]).any(|dir| crate::vfs::relative_path(&path, dir).is_some())
    }

    /// Native bindings and the compile unit's std are shared by every domain. A fresh app
    /// domain cannot replace them; keep the current app and report the host restart instead.
    /// An input of makepad's own process (not a program source a patch can take).
    pub(super) fn process_input(&self, path: &str) -> bool {
        matches!(self.paths.get(path).map(|x| &x.1), Some(Kind::Provided | Kind::Std | Kind::Runner | Kind::Manifest | Kind::Build))
    }

    pub(super) fn host_restart_required<'a>(&self, paths: impl Iterator<Item = &'a str>) -> bool {
        paths.into_iter().any(|path| matches!(self.paths.get(path).map(|x| &x.1), Some(Kind::Provided | Kind::Std | Kind::Runner)))
    }

    pub(super) fn restart_reason<'a>(&self, paths: impl Iterator<Item = &'a str>) -> Option<&'static str> {
        let mut reason = None;
        for path in paths {
            match self.paths.get(path).map(|x| &x.1) {
                Some(Kind::Provided) => return Some("the Splash VM or other provided sources changed"),
                Some(Kind::Std) => reason = Some("makepad run's std changed"),
                Some(Kind::Runner) => { if reason.is_none() { reason = Some("makepad run's own sources changed"); } }
                Some(Kind::Manifest) => { if reason.is_none() { reason = Some("the Cargo graph changed"); } }
                Some(Kind::Build) => { if reason.is_none() { reason = Some("a build script changed"); } }
                Some(Kind::Source | Kind::SourceDirectory(_)) | None => {}
            }
        }
        // The watcher settles the whole tree, including git operations. Batch
        // size alone is never a reason to refuse a source transaction.
        reason
    }
}

/// The files the loader `exe` runs: for a cargo build, its dep-info `<exe>.d` (every file the
/// binary was built from); for a JIT devhub picked by its inputs' content ([`super::host::picked`]),
/// its `<exe>.inputs` (repo-relative, lease/loaders.rs) in the workspace it was picked for. The
/// `makepad.d` beside a picked binary is another build's, of another checkout: a lane's session
/// watched the box's checkout, and its lane's own JIT sources were no input of it (W3752,
/// 11-06). None for a binary with neither.
fn runner_paths(exe: &Path, workspace: &str) -> Vec<String> {
    if super::host::picked(exe) {
        let Some(name) = exe.file_name() else { return Vec::new() };
        let Ok(text) = std::fs::read_to_string(exe.with_file_name(format!("{}.inputs", name.to_string_lossy()))) else { return Vec::new() };
        return text.lines().filter(|l| !l.is_empty()).map(|rel| crate::vfs::normalize_path(&format!("{workspace}/{rel}"))).collect();
    }
    std::fs::read_to_string(exe.with_extension("d")).map(|text| dep_info_paths(&text)).unwrap_or_default()
}

/// The prerequisites of a cargo dep-info file (`target: a b\ c ...`; `\ ` is a space in a
/// path), one rule per line.
fn dep_info_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find(": ") else { continue };
        let mut cur = String::new();
        let mut chars = line[at + 2..].chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => { cur.push(' '); chars.next(); }
                ' ' => { if !cur.is_empty() { out.push(std::mem::take(&mut cur)); } }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() { out.push(cur); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_boundary_and_non_patchable_inputs() {
        let mut inputs = Inputs::default();
        inputs.insert(Path::new("app/a.rs"), Kind::Source);
        inputs.insert(Path::new("app/b.rs"), Kind::Source);
        inputs.insert(Path::new("dep/c.rs"), Kind::Source);
        assert_eq!(inputs.restart_reason(["app/a.rs", "app/b.rs"].into_iter()), None);
        // a multi-package edit (app + its library) is patched
        assert_eq!(inputs.restart_reason(["app/a.rs", "dep/c.rs"].into_iter()), None);
        // a settled checkout-sized batch patches together too
        let many: Vec<String> = (0..32).map(|i| format!("bulk/{i}.rs")).collect();
        for p in &many { inputs.insert(Path::new(p), Kind::Source); }
        assert_eq!(inputs.restart_reason(many.iter().map(|s| s.as_str())), None);
        let pkgs: Vec<String> = (0..8).map(|i| format!("p{i}/a.rs")).collect();
        for p in &pkgs { inputs.insert(Path::new(p), Kind::Source); }
        assert_eq!(inputs.restart_reason(pkgs.iter().map(|s| s.as_str())), None);
        for kind in [Kind::Provided, Kind::Build, Kind::Manifest, Kind::Std, Kind::Runner] {
            inputs.insert(Path::new("changed"), kind);
            assert!(inputs.restart_reason(["changed"].into_iter()).is_some());
        }
    }

    #[test]
    fn a_picked_jit_runs_the_sources_of_the_tree_it_was_picked_for() {
        let dir = std::env::temp_dir().join(format!("relax-runner-paths-{}", std::process::id()));
        let host = dir.join("target/host");
        std::fs::create_dir_all(&host).unwrap();
        // the target's own build is of the box's checkout; the picked binary is the lane tree's
        std::fs::write(host.join("makepad.d"), format!("{}/makepad: /box/libs/relax/base/src/vfs.rs\n", host.display())).unwrap();
        std::fs::write(host.join("makepad.jit-0123456789abcdef01234567.inputs"), "libs/relax/base/src/vfs.rs\ntools/run/src/main.rs\n").unwrap();
        let picked = host.join("makepad.jit-0123456789abcdef01234567");
        assert!(super::super::host::picked(&picked));
        assert_eq!(runner_paths(&picked, "/lane"), ["/lane/libs/relax/base/src/vfs.rs", "/lane/tools/run/src/main.rs"]);
        let built = host.join("makepad");
        assert!(!super::super::host::picked(&built));
        assert_eq!(runner_paths(&built, "/lane"), ["/box/libs/relax/base/src/vfs.rs"], "cargo's own build runs what its dep-info names");
        // a store's binary keeps no inputs file: it names none
        assert!(runner_paths(&dir.join("makepad.jit-89abcdef0123456789abcdef"), "/lane").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dep_info() {
        let d = "/t/release/makepad: /w/a.rs /w/my\\ dir/b.rs /w/Cargo.toml\n\n/w/a.rs:\n";
        assert_eq!(dep_info_paths(d), ["/w/a.rs", "/w/my dir/b.rs", "/w/Cargo.toml"]);
    }
}
