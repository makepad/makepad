//! Inputs whose identity cannot be replaced by a live body transaction. Snapshot before
//! loading, so a checkout during load/parse is still observed when the watcher starts.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) type Stamp = Option<(std::time::SystemTime, u64)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Source(String),
    Provided,
    Build,
    Manifest,
    Std,
}

#[derive(Default, Clone)]
pub struct Inputs {
    pub(super) paths: BTreeMap<String, (Stamp, Kind)>,
}

impl Inputs {
    fn insert(&mut self, path: &Path, kind: Kind) {
        let path = path.to_string_lossy().into_owned();
        let stamp = super::stamp(&path);
        self.paths.insert(path, (stamp, kind));
    }

    fn protected(&mut self, path: &Path, root: &Path, kind: Kind) {
        self.insert(path, kind.clone());
        // Directory identities catch additions and atomic replacements of files that
        // did not exist at boot. Native events, not a periodic tree walk, drive them.
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|p| p.starts_with(root)) {
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
            let mut files = Vec::new();
            crate::srchash::files(Path::new(dir), "", &mut files);
            for (rel, path) in files {
                let kind = if matches!(rel.as_str(), "Cargo.toml" | "Cargo.lock") {
                    Kind::Manifest
                } else if provided {
                    Kind::Provided
                } else if path.extension().is_some_and(|x| x == "rs") {
                    Kind::Source(dir.clone())
                } else { continue };
                if provided { inputs.protected(&path, Path::new(dir), kind); }
                else { inputs.insert(&path, kind); }
            }
            // Include an absent lockfile too: creating it changes the resolved graph.
            inputs.insert(&Path::new(dir).join("Cargo.lock"), Kind::Manifest);
        }
        for script in &graph.build_scripts {
            inputs.insert(Path::new(&script.src_path), Kind::Build);
        }
        for name in ["Cargo.toml", "Cargo.lock"] {
            inputs.insert(&Path::new(workspace).join(name), Kind::Manifest);
        }
        let mut files = Vec::new();
        crate::srchash::files(Path::new(std_dir), "", &mut files);
        for (_, path) in files { inputs.protected(&path, Path::new(std_dir), Kind::Std); }
        inputs
    }

    /// More source files than this in one settled batch is a bulk change.
    pub(super) const BULK_SOURCES: usize = 16;
    /// Sources of more packages than this in one settled batch is a bulk change.
    pub(super) const BULK_PACKAGES: usize = 4;

    /// Native bindings and the compile unit's std are shared by every domain. A fresh app
    /// domain cannot replace them; keep the current app and report the host restart instead.
    pub(super) fn host_restart_required<'a>(&self, paths: impl Iterator<Item = &'a str>) -> bool {
        paths.into_iter().any(|path| matches!(self.paths.get(path).map(|x| &x.1), Some(Kind::Provided | Kind::Std)))
    }

    pub(super) fn restart_reason<'a>(&self, paths: impl Iterator<Item = &'a str>) -> Option<&'static str> {
        let mut packages = BTreeSet::new();
        let mut sources = 0usize;
        let mut reason = None;
        for path in paths {
            match self.paths.get(path).map(|x| &x.1) {
                Some(Kind::Provided) => return Some("the Splash VM or other provided sources changed: rebuild makepad"),
                Some(Kind::Std) => reason = Some("makepad run's std changed"),
                Some(Kind::Manifest) => { if reason.is_none() { reason = Some("the Cargo graph changed"); } }
                Some(Kind::Build) => { if reason.is_none() { reason = Some("a build script changed"); } }
                Some(Kind::Source(package)) => { packages.insert(package); sources += 1; }
                None => {}
            }
        }
        // A bulk change (a git checkout / ff / stash pop) rewrites many files across packages at
        // once and can leave the tree between revisions; an editor or an agent saving a multi-file
        // edit touches a few files of a few packages, which a patch handles. Keep running the old
        // program on a bulk change; this is a reload policy, not a capacity limit.
        let bulk = sources > Self::BULK_SOURCES || packages.len() > Self::BULK_PACKAGES;
        reason.or_else(|| bulk.then_some("many sources changed at once (e.g. a git checkout)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_boundary_and_non_patchable_inputs() {
        let mut inputs = Inputs::default();
        inputs.insert(Path::new("app/a.rs"), Kind::Source("app".into()));
        inputs.insert(Path::new("app/b.rs"), Kind::Source("app".into()));
        inputs.insert(Path::new("dep/c.rs"), Kind::Source("dep".into()));
        assert_eq!(inputs.restart_reason(["app/a.rs", "app/b.rs"].into_iter()), None);
        // a multi-package edit (app + its library) is patched
        assert_eq!(inputs.restart_reason(["app/a.rs", "dep/c.rs"].into_iter()), None);
        // a checkout-sized batch is not
        let many: Vec<String> = (0..=Inputs::BULK_SOURCES).map(|i| format!("bulk/{i}.rs")).collect();
        for p in &many { inputs.insert(Path::new(p), Kind::Source("bulk".into())); }
        assert!(inputs.restart_reason(many.iter().map(|s| s.as_str())).is_some());
        let pkgs: Vec<String> = (0..=Inputs::BULK_PACKAGES).map(|i| format!("p{i}/a.rs")).collect();
        for (i, p) in pkgs.iter().enumerate() { inputs.insert(Path::new(p), Kind::Source(format!("p{i}"))); }
        assert!(inputs.restart_reason(pkgs.iter().map(|s| s.as_str())).is_some());
        for kind in [Kind::Provided, Kind::Build, Kind::Manifest, Kind::Std] {
            inputs.insert(Path::new("changed"), kind);
            assert!(inputs.restart_reason(["changed"].into_iter()).is_some());
        }
    }
}
