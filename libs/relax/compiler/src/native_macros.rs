//! Makepad proc macros are native-only and must match the linked package closure.
use crate::macro_reg::{Entry, Reg};
use relax_native_macros::Entry as Native;

static SOURCE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn report(profile: bool) {
    if profile { eprintln!("native-macro-validation: source_reads={} hashes=0", SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed)); }
}

static UPDATE: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
static GRAPH: std::sync::RwLock<Option<std::sync::Arc<Graph>>> = std::sync::RwLock::new(None);

pub(crate) struct ZoneTaken(Option<std::sync::Arc<Graph>>);

pub(crate) fn zone_take() -> ZoneTaken {
    // prepare (called when the front registry is installed) rebuilds this graph.
    let mut graph = GRAPH.write().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken(std::mem::take(&mut *graph));
    #[cfg(test)]
    assert!(graph.is_none());
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut graph = GRAPH.write().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *graph, taken.0));
}

#[derive(Clone, Debug)]
pub struct Mismatch {
    package: String,
    file: String,
    reason: String,
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the proc macros in this tree differ from those built into makepad: {} [{}: {}: {}]", crate::srchash::rebuild_hint(), self.package, self.file, self.reason)
    }
}
impl std::error::Error for Mismatch {}

pub fn required(name: &str) -> bool {
    relax_native_macros::PACKAGES.iter().any(|(n, _)| *n == name)
}

fn validate_package(reg: &Reg, unit: usize, overlays: &std::collections::HashMap<String, Vec<u8>>) -> Result<(), Mismatch> {
    let u = &reg.units[unit];
    let error = |file: String, reason: String| Mismatch { package: u.name.clone(), file, reason };
    let Some((_, files)) = relax_native_macros::PACKAGES.iter().find(|(name, _)| *name == u.name) else {
        return Err(error(u.src_path.clone(), "dependency is not built into makepad".into()));
    };
    if u.proc_macro != relax_native_macros::ENTRIES.iter().any(|n| n.krate == u.name) {
        return Err(error(u.src_path.clone(), "proc-macro crate kind differs".into()));
    }
    let features: &[&str] = match u.name.as_str() {
        "makepad_derive_widget" => &["default", "nightly"],
        "makepad_live_id" => &["default"],
        _ => &[],
    };
    if u.edition != 2021 || !u.features.iter().all(|f| features.contains(&f.as_str())) {
        return Err(error(u.src_path.clone(), format!("edition {} or features {:?} differ", u.edition, u.features)));
    }
    let root = std::path::Path::new(&u.src_path).parent().and_then(|p| p.parent())
        .ok_or_else(|| error(u.src_path.clone(), "invalid package root".into()))?;
    if root.join("src/lib.rs") != std::path::Path::new(&u.src_path) {
        return Err(error(u.src_path.clone(), "crate root is not src/lib.rs".into()));
    }
    let _why = crate::why::enabled().then(|| crate::why::enter(crate::why::Cause::new(&u.src_path, 1, "native-macro-validation", &u.name)));
    for (path, expected) in *files {
        let path = root.join(path);
        let actual = if let Some(bytes) = overlays.get(&crate::vfs::normalize_path(&path.to_string_lossy())) {
            std::sync::Arc::<[u8]>::from(bytes.as_slice())
        } else {
            SOURCE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::vfs::read_shared(&path).map_err(|e| error(path.display().to_string(), e.to_string()))?
        };
        if actual.as_ref() != *expected {
            return Err(error(path.display().to_string(), "source bytes differ".into()));
        }
    }
    // Additions matter as well as modified/deleted embedded inputs.
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for (path, is_dir) in crate::vfs::read_dir(&dir).map_err(|e| error(dir.display().to_string(), e.to_string()))? {
            let path = dir.join(path);
            if is_dir { dirs.push(path); }
            else if !files.iter().any(|(relative, _)| root.join(relative) == path) {
                return Err(error(path.display().to_string(), "source file was added".into()));
            }
        }
    }
    for path in overlays.keys().filter(|p| std::path::Path::new(p).starts_with(root.join("src")) || std::path::Path::new(p) == root.join("build.rs")) {
        if !files.iter().any(|(relative, _)| crate::vfs::normalize_path(&root.join(relative).to_string_lossy()) == *path) {
            return Err(error(path.clone(), "source file was added".into()));
        }
    }
    if crate::vfs::exists(root.join("build.rs")) && !files.iter().any(|(p, _)| *p == "build.rs") {
        return Err(error(root.join("build.rs").display().to_string(), "build script was added".into()));
    }
    let (_, expected) = relax_native_macros::DEPENDENCIES.iter().find(|(name, _)| *name == u.name).unwrap();
    if u.deps.len() != expected.len() {
        return Err(error(root.join("Cargo.toml").display().to_string(), "dependency count differs".into()));
    }
    for name in *expected {
        let (dep, _) = u.deps.iter().find(|(_, alias)| alias == name)
            .ok_or_else(|| error(root.join("Cargo.toml").display().to_string(), format!("missing dependency alias {name}")))?;
        if reg.units[*dep].name != *name {
            return Err(error(root.join("Cargo.toml").display().to_string(), format!("dependency {name} resolves to {}", reg.units[*dep].name)));
        }
        if !required(&reg.units[*dep].name) {
            return Err(error(root.join("Cargo.toml").display().to_string(), format!("dependency {name} is not built into makepad")));
        }
    }

    Ok(())
}

// Own-package verdicts are separate from propagated dependency verdicts: a helper edit
// invalidates its consumers without rereading any consumer's identity inputs.
#[derive(Clone)]
struct Graph {
    reg: Reg,
    own: Vec<Option<Result<(), Mismatch>>>,
    verdicts: Vec<Result<(), String>>,
    entries: Vec<std::collections::HashMap<String, &'static Native>>,
    overlays: std::collections::HashMap<String, Vec<u8>>,
    watch_paths: Vec<String>,
}
impl Graph {
    fn new(reg: Reg) -> Self {
        let mut graph = Self { own: vec![None; reg.units.len()], verdicts: vec![Ok(()); reg.units.len()],
            entries: (0..reg.units.len()).map(|_| Default::default()).collect(), overlays: Default::default(), watch_paths: Vec::new(), reg };
        for (unit, u) in graph.reg.units.iter().enumerate().filter(|(_, u)| required(&u.name)) {
            graph.own[unit] = Some(validate_package(&graph.reg, unit, &graph.overlays));
            graph.entries[unit].extend(relax_native_macros::ENTRIES.iter().filter(|n| n.krate == u.name).map(|n| (n.fname.to_string(), n)));
        }
        graph.propagate();
        graph.watch_paths = graph.input_paths();
        graph
    }
    fn propagate(&mut self) {
        fn visit(graph: &Graph, unit: usize, visiting: &mut [bool], done: &mut [Option<Result<(), String>>]) -> Result<(), String> {
            if let Some(verdict) = &done[unit] { return verdict.clone(); }
            let Some(own) = &graph.own[unit] else { return Ok(()) };
            let u = &graph.reg.units[unit];
            if visiting[unit] { return Err(Mismatch { package: u.name.clone(), file: u.src_path.clone(), reason: "dependency cycle".into() }.to_string()); }
            visiting[unit] = true;
            let verdict = own.as_ref().map_err(|e| e.to_string()).and_then(|_| {
                for (dep, _) in &u.deps { visit(graph, *dep, visiting, done)?; }
                for (_, entry) in graph.reg.derives.iter().chain(&graph.reg.macros).filter(|(_, e)| e.unit == unit) {
                    if !graph.entries[unit].contains_key(&entry.fname) { return Err(graph.missing(entry)); }
                }
                Ok(())
            });
            visiting[unit] = false;
            done[unit] = Some(verdict.clone());
            verdict
        }
        let mut done = vec![None; self.reg.units.len()];
        let mut visiting = vec![false; self.reg.units.len()];
        for unit in 0..self.reg.units.len() { let _ = visit(self, unit, &mut visiting, &mut done); }
        self.verdicts = done.into_iter().map(|v| v.unwrap_or(Ok(()))).collect();
    }
    fn missing(&self, entry: &Entry) -> String {
        let u = &self.reg.units[entry.unit];
        Mismatch { package: u.name.clone(), file: u.src_path.clone(), reason: format!("entry {} is not built into makepad", entry.fname) }.to_string()
    }
    fn verdict(&self) -> Result<(), String> { self.verdicts.iter().find(|v| v.is_err()).cloned().unwrap_or(Ok(())) }
    fn find(&self, entry: &Entry) -> Result<Option<&'static Native>, String> {
        self.verdicts[entry.unit].clone()?;
        if self.own[entry.unit].is_none() { return Ok(None); }
        self.entries[entry.unit].get(&entry.fname).copied().map(Some).ok_or_else(|| self.missing(entry))
    }
    fn affected(&self, unit: usize, path: &str) -> bool {
        if self.own[unit].is_none() { return false; }
        let Some(root) = std::path::Path::new(&self.reg.units[unit].src_path).parent().and_then(|p| p.parent()) else { return false };
        let root = std::path::PathBuf::from(crate::vfs::normalize_path(&root.to_string_lossy()));
        let path = std::path::Path::new(path);
        path == root || path.starts_with(root.join("src")) || path == root.join("Cargo.toml") || path == root.join("build.rs")
    }
    fn reload(&mut self, changes: &[(&str, Option<&[u8]>)]) -> bool {
        let mut dirty = vec![false; self.reg.units.len()];
        for (path, bytes) in changes {
            let path = crate::vfs::normalize_path(path);
            let mut native = false;
            for (unit, dirty) in dirty.iter_mut().enumerate() {
                if self.affected(unit, &path) { *dirty = true; native = true; }
            }
            if native {
                if let Some(bytes) = bytes { self.overlays.insert(path, bytes.to_vec()); }
                else { self.overlays.remove(&path); }
            }
        }
        for (unit, _) in dirty.iter().enumerate().filter(|(_, dirty)| **dirty) {
            self.own[unit] = Some(validate_package(&self.reg, unit, &self.overlays));
        }
        if dirty.iter().any(|d| *d) { self.propagate(); true } else { false }
    }
    fn input_paths(&self) -> Vec<String> {
        let mut paths = std::collections::BTreeSet::new();
        for (_, u) in self.reg.units.iter().enumerate().filter(|(i, _)| self.own[*i].is_some()) {
            let root = std::path::Path::new(&u.src_path).parent().unwrap().parent().unwrap();
            let (_, inputs) = relax_native_macros::PACKAGES.iter().find(|(n, _)| *n == u.name).unwrap();
            // Watch directories too: an added source or build script has no embedded file.
            for (relative, _) in *inputs {
                let path = root.join(relative);
                paths.insert(path.to_string_lossy().into_owned());
                for parent in path.ancestors().skip(1).take_while(|p| p.starts_with(root)) {
                    paths.insert(parent.to_string_lossy().into_owned());
                }
            }
        }
        paths.into_iter().collect()
    }
}

/// Expansion, outlining, declaration and reuse queries are table reads only.
pub fn find(entry: &Entry) -> Result<Option<&'static Native>, String> {
    let graph = GRAPH.read().unwrap();
    graph.as_ref().ok_or("proc macro graph is not registered")?.find(entry)
}
pub fn verdict() -> Result<(), String> {
    GRAPH.read().unwrap().as_ref().map_or(Ok(()), |g| g.verdict())
}
pub fn owns_path(path: &str) -> bool {
    let path = crate::vfs::normalize_path(path);
    GRAPH.read().unwrap().as_ref().is_some_and(|g| (0..g.reg.units.len()).any(|i| g.affected(i, &path)))
}
pub fn paths() -> Vec<String> {
    GRAPH.read().unwrap().as_ref().map_or_else(Vec::new, |g| g.watch_paths.clone())
}

/// Called once at graph registration, never again by prewarm or expansion.
pub fn prepare() -> Result<(), String> {
    let _update = UPDATE.lock().unwrap();
    SOURCE_READS.store(0, std::sync::atomic::Ordering::Relaxed);
    let reads = crate::vfs::ReadSnapshot::new(false);
    let _reads = reads.bind();
    let reg = crate::macro_reg::REG.with(|r| r.borrow().clone());
    let graph = reg.map(Graph::new).map(std::sync::Arc::new);
    let verdict = graph.as_ref().map_or(Ok(()), |g| g.verdict());
    if crate::macro_reg::trace() {
        eprintln!("pmhost: {} source-matched native entries", graph.as_ref().map_or(0, |g| g.entries.iter().enumerate().filter(|(i, _)| g.verdicts[*i].is_ok()).map(|(_, e)| e.len()).sum::<usize>()));
    }
    *GRAPH.write().unwrap() = graph;
    verdict
}

/// Changed paths come from the reload transaction; Some(bytes) is an unsaved buffer.
/// Publish failed verdicts too, so later calls cannot use stale certification.
pub fn reload(changes: &[(&str, Option<&[u8]>)], profile: bool) -> Result<(), String> {
    let _update = UPDATE.lock().unwrap();
    let old = GRAPH.read().unwrap().clone();
    let Some(old) = old else { return Ok(()) };
    if !changes.iter().any(|(p, _)| (0..old.reg.units.len()).any(|i| old.affected(i, &crate::vfs::normalize_path(p)))) {
        if profile { eprintln!("native-macro-reload: source_reads=0 hashes=0"); }
        return old.verdict();
    }
    let reads = crate::vfs::ReadSnapshot::new(false);
    let _reads = reads.bind();
    let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
    let mut next = (*old).clone();
    next.reload(changes);
    let verdict = next.verdict();
    *GRAPH.write().unwrap() = Some(std::sync::Arc::new(next));
    if profile { eprintln!("native-macro-reload: source_reads={} hashes=0", SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed) - before); }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;
    fn checked(reg: &Reg) -> Result<(), Mismatch> {
        let graph = Graph::new(reg.clone());
        for verdict in graph.own.iter().flatten() { verdict.clone()?; }
        Ok(())
    }
    #[test]
    fn native_requires_the_complete_source_and_dependency_identity() {
        use crate::macro_reg::UnitInfo;
        let root = std::env::temp_dir().join(format!("relax-native-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(root.clone());
        let mut reg = Reg::default();
        for name in ["makepad_micro_proc_macro", "makepad_micro_serde_derive_impl", "makepad_micro_serde_derive"] {
            let (_, files) = relax_native_macros::PACKAGES.iter().find(|(n, _)| *n == name).unwrap();
            for (path, bytes) in *files {
                let path = root.join(name).join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            let unit = reg.units.len();
            reg.units.push(UnitInfo { name: name.into(), src_path: root.join(name).join("src/lib.rs").to_string_lossy().into_owned(), edition: 2021,
                features: Vec::new(), deps: if unit == 0 { Vec::new() } else { vec![(unit - 1, reg.units[unit - 1].name.clone())] }, proc_macro: unit == 2 });
        }
        reg.units[2].deps.push((0, "makepad_micro_proc_macro".into()));
        assert!(checked(&reg).is_ok());
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        let mut graph = Graph::new(reg.clone());
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed) - before, 11);
        let native = relax_native_macros::ENTRIES.iter().find(|n| n.krate == reg.units[2].name).unwrap();
        let entry = Entry { unit: 2, fname: native.fname.into(), kind: crate::macro_reg::Kind::Derive };
        assert!(graph.find(&entry).unwrap().is_some());
        let missing = Entry { fname: "missing_native_entry".into(), ..entry.clone() };
        assert!(graph.find(&missing).err().unwrap().contains("entry missing_native_entry is not built into makepad"));
        let helper = root.join("makepad_micro_proc_macro/src/macro_lib.rs");
        let original = std::fs::read(&helper).unwrap();
        std::fs::write(&helper, b"// changed helper").unwrap();
        let mismatch = checked(&reg).unwrap_err();
        assert_eq!(mismatch.package, "makepad_micro_proc_macro");
        assert_eq!(mismatch.file, helper.to_string_lossy());
        assert_eq!(mismatch.reason, "source bytes differ");
        // Only a reload transaction changes certification; use-time lookup reads no files.
        assert!(graph.find(&entry).unwrap().is_some());
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        assert!(!graph.reload(&[("/unrelated/app.rs", None)]));
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed), before);
        assert!(graph.reload(&[(helper.to_str().unwrap(), None)]));
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        assert!(graph.find(&entry).err().unwrap().contains("source bytes differ"));
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed), before);
        assert!(mismatch.to_string().contains("the proc macros in this tree differ from those built into makepad: rebuild makepad by starting it through cargo: cargo run --release -p makepad-run --bin makepad --"));
        std::fs::write(&helper, &original).unwrap();
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        graph.reload(&[(helper.to_str().unwrap(), None), (helper.to_str().unwrap(), None)]);
        let count = relax_native_macros::PACKAGES.iter().find(|(n, _)| *n == "makepad_micro_proc_macro").unwrap().1.len();
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed) - before, count);
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        assert!(graph.find(&entry).unwrap().is_some());
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed), before);
        graph.reload(&[(helper.to_str().unwrap(), Some(b"// unsaved helper"))]);
        assert!(graph.find(&entry).err().unwrap().contains("source bytes differ"));
        graph.reload(&[(helper.to_str().unwrap(), None)]);
        assert!(graph.find(&entry).unwrap().is_some());
        reg.units[1].deps[0].1 = "replacement".into();
        assert!(checked(&reg).unwrap_err().reason.contains("dependency alias"));
        reg.units[1].deps[0].1 = "makepad_micro_proc_macro".into();
        reg.units[2].features.push("different".into());
        assert!(checked(&reg).unwrap_err().reason.contains("features"));
        reg.units[2].features.clear();
        reg.units[2].edition = 2018;
        assert!(checked(&reg).unwrap_err().reason.contains("edition"));
        reg.units[2].edition = 2021;
        let added = root.join("makepad_micro_proc_macro/src/added.rs");
        std::fs::write(&added, b"// newly added input").unwrap();
        assert_eq!(checked(&reg).unwrap_err().reason, "source file was added");
        std::fs::remove_file(&added).unwrap();
        assert!(checked(&reg).is_ok());
    }
}
