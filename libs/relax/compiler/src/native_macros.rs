//! Source-matched native proc macros (relax_native_macros: the base set and the sets the host
//! registered): a macro runs natively only when its package closure matches the linked one.
use crate::macro_reg::{Entry, Reg};
use relax_native_macros::{Entry as Native, Package};

/// Every set's packages by crate name (relax_native_macros::sets; the first read fixes the sets).
fn packages() -> &'static std::collections::HashMap<&'static str, &'static Package> {
    static PACKAGES: std::sync::OnceLock<std::collections::HashMap<&'static str, &'static Package>> = std::sync::OnceLock::new();
    PACKAGES.get_or_init(|| relax_native_macros::sets().iter().flat_map(|s| s.packages).map(|p| (p.name, p)).collect())
}

/// Every set's entries.
fn entries() -> impl Iterator<Item = &'static Native> {
    relax_native_macros::sets().iter().flat_map(|s| s.entries)
}

static SOURCE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub fn report(profile: bool) {
    if profile { eprintln!("native-macro-validation: source_reads={} hashes=0", SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed)); }
}

static UPDATE: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
static GRAPH: std::sync::RwLock<Option<std::sync::Arc<Graph>>> = std::sync::RwLock::new(None);


#[derive(Clone, Debug)]
pub struct Mismatch {
    package: String,
    file: String,
    reason: String,
}
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the proc macros in this tree differ from those built into {tool}: {} [{}: {}: {}]", crate::srchash::rebuild_hint(crate::tool_name()), self.package, self.file, self.reason, tool = crate::tool_name())
    }
}
impl std::error::Error for Mismatch {}

pub fn required(name: &str) -> bool {
    packages().contains_key(name)
}

fn validate_package(reg: &Reg, unit: usize, overlays: &std::collections::HashMap<String, Vec<u8>>) -> Result<(), Mismatch> {
    let u = &reg.units[unit];
    let error = |file: String, reason: String| Mismatch { package: u.name.clone(), file, reason };
    let Some(package) = packages().get(u.name.as_str()) else {
        return Err(error(u.src_path.clone(), format!("dependency is not built into {tool}", tool = crate::tool_name())));
    };
    let files = package.files;
    if u.proc_macro != entries().any(|n| n.krate == u.name) {
        return Err(error(u.src_path.clone(), "proc-macro crate kind differs".into()));
    }
    if u.edition != 2021 || !u.features.iter().all(|f| package.features.contains(&f.as_str())) {
        return Err(error(u.src_path.clone(), format!("edition {} or features {:?} differ", u.edition, u.features)));
    }
    let root = std::path::Path::new(&u.src_path).parent().and_then(|p| p.parent())
        .ok_or_else(|| error(u.src_path.clone(), "invalid package root".into()))?;
    if root.join("src/lib.rs") != std::path::Path::new(&u.src_path) {
        return Err(error(u.src_path.clone(), "crate root is not src/lib.rs".into()));
    }
    let _why = crate::why::enabled().then(|| crate::why::enter(crate::why::Cause::new(&u.src_path, 1, "native-macro-validation", &u.name)));
    for (path, expected) in files {
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
    // A graph that also links relax-native-macros as a library (relax-compiler's) holds
    // micro_proc_macro once, with the target side's `relax-tokens` (unitgraph: host and target
    // copies are one crate). The proc-macro side of that graph is the accepted one plus that
    // feature's optional dependency, which no macro entry reaches.
    let optional = |alias: &str| u.name == "makepad_micro_proc_macro" && alias == "relax_lex" && u.features.iter().any(|f| f == "relax-tokens");
    // the sysroot's proc_macro (run.rs with_sysroot_proc_macro) is no Cargo dependency
    let sysroot = |dep: usize, alias: &str| alias == "proc_macro" && reg.units[dep].name == "proc_macro";
    let deps: Vec<&(usize, String)> = u.deps.iter().filter(|(dep, alias)| !optional(alias) && !sysroot(*dep, alias)).collect();
    check_deps(reg, &deps, package.deps).map_err(|reason| error(root.join("Cargo.toml").display().to_string(), reason))?;
    Ok(())
}

/// A package's graph dependencies against its manifest's (relax_native_macros `Package::deps`,
/// whose bytes the graph's manifest equals): each one is a dependency the manifest declares,
/// resolves to the crate of its name and is built in; an unconditional one is in every graph, an
/// optional or target-specific one in the graphs whose features and target select it.
fn check_deps(reg: &Reg, deps: &[&(usize, String)], manifest: &[(&str, bool)]) -> Result<(), String> {
    for (dep, alias) in deps.iter().copied() {
        if !manifest.iter().any(|(name, _)| name == alias) {
            return Err(format!("dependency alias {alias} is not in the manifest built into {tool}", tool = crate::tool_name()));
        }
        if reg.units[*dep].name != *alias {
            return Err(format!("dependency {alias} resolves to {}", reg.units[*dep].name));
        }
        if !required(alias) {
            return Err(format!("dependency {alias} is not built into {tool}", tool = crate::tool_name()));
        }
    }
    for (name, _) in manifest.iter().filter(|(_, conditional)| !conditional) {
        if !deps.iter().any(|(_, alias)| alias == name) {
            return Err(format!("missing dependency alias {name}"));
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
            graph.entries[unit].extend(entries().filter(|n| n.krate == u.name).map(|n| (n.fname.to_string(), n)));
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
        Mismatch { package: u.name.clone(), file: u.src_path.clone(), reason: format!("entry {} is not built into {tool}", entry.fname, tool = crate::tool_name()) }.to_string()
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
            // Watch directories too: an added source or build script has no embedded file.
            for (relative, _) in packages()[u.name.as_str()].files {
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
            for (path, bytes) in packages()[name].files {
                let path = root.join(name).join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
            let unit = reg.units.len();
            reg.units.push(UnitInfo { name: name.into(), src_path: root.join(name).join("src/lib.rs").to_string_lossy().into_owned(), edition: 2021,
                features: Vec::new(), deps: if unit == 0 { Vec::new() } else { vec![(unit - 1, reg.units[unit - 1].name.clone())] }, proc_macro: unit == 2, harness: false, registry: false, cfgs: Vec::new(), env: Vec::new() });
        }
        reg.units[2].deps.push((0, "makepad_micro_proc_macro".into()));
        assert!(checked(&reg).is_ok());
        // The dependencies are the manifests' (gen.rs): an optional one is conditional, a plain one not.
        assert_eq!(packages()["makepad_micro_proc_macro"].deps, &[("relax_lex", true)], "an optional manifest dependency");
        assert_eq!(packages()["makepad_micro_serde_derive"].deps, &[("makepad_micro_proc_macro", false), ("makepad_micro_serde_derive_impl", false)], "plain manifest dependencies");
        // A graph whose features or target leave out a conditional dependency passes; one without an
        // unconditional dependency, or with one the manifest lacks, does not.
        let manifest = [("makepad_micro_proc_macro", false), ("makepad_micro_serde_derive_impl", true)];
        let deps = |units: &[usize]| units.iter().map(|&unit| (unit, reg.units[unit].name.clone())).collect::<Vec<_>>();
        let check = |graph: &[(usize, String)]| check_deps(&reg, &graph.iter().collect::<Vec<_>>(), &manifest);
        assert_eq!(check(&deps(&[0, 1])), Ok(()), "the conditional dependency selected");
        assert_eq!(check(&deps(&[0])), Ok(()), "the conditional dependency left out");
        assert_eq!(check(&deps(&[1])).unwrap_err(), "missing dependency alias makepad_micro_proc_macro", "an unconditional dependency left out");
        assert!(check(&deps(&[0, 2])).unwrap_err().contains("dependency alias makepad_micro_serde_derive is not in the manifest"), "a dependency the manifest lacks");
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        let mut graph = Graph::new(reg.clone());
        assert_eq!(SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed) - before, 11);
        let native = entries().find(|n| n.krate == reg.units[2].name).unwrap();
        let entry = Entry { unit: 2, fname: native.fname.into(), kind: crate::macro_reg::Kind::Derive };
        assert!(graph.find(&entry).unwrap().is_some());
        let missing = Entry { fname: "missing_native_entry".into(), ..entry.clone() };
        assert!(graph.find(&missing).err().unwrap().contains("entry missing_native_entry is not built into relax"));
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
        assert!(mismatch.to_string().contains("the proc macros in this tree differ from those built into relax: rebuild relax by starting it through cargo: cargo run --release -p makepad-run --bin makepad --"));
        std::fs::write(&helper, &original).unwrap();
        let before = SOURCE_READS.load(std::sync::atomic::Ordering::Relaxed);
        graph.reload(&[(helper.to_str().unwrap(), None), (helper.to_str().unwrap(), None)]);
        let count = packages()["makepad_micro_proc_macro"].files.len();
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

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&SOURCE_READS, std::sync::atomic::AtomicUsize::new(0));
    reset(&GRAPH, std::sync::RwLock::new(None));
}
