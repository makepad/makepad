//! Git identities sampled by the live watcher, never by an application thread.
//! A linked worktree owns HEAD and index.lock; its refs live in commondir.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct Repositories {
    dirs: BTreeSet<PathBuf>,
    parents: HashMap<PathBuf, Option<PathBuf>>,
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    pub busy: bool,
    identities: Vec<(PathBuf, super::inputs::Stamp, Option<String>)>,
}

impl Repositories {
    pub fn include<'a>(&mut self, paths: impl Iterator<Item = &'a str>) {
        for path in paths {
            if crate::vfs::is_virtual(path) { continue; }
            let mut at = Path::new(path).parent();
            let mut visited = Vec::new();
            let mut found = None;
            while let Some(dir) = at {
                if let Some(cached) = self.parents.get(dir) {
                    found = cached.clone();
                    break;
                }
                visited.push(dir.to_path_buf());
                let dot = dir.join(".git");
                if dot.is_dir() {
                    found = Some(dot);
                    break;
                }
                if let Ok(text) = std::fs::read_to_string(&dot) {
                    if let Some(gitdir) = text.trim().strip_prefix("gitdir:") {
                        found = Some(dir.join(gitdir.trim()));
                        break;
                    }
                }
                at = dir.parent();
            }
            for dir in visited { self.parents.insert(dir, found.clone()); }
            if let Some(dir) = found { self.dirs.insert(dir); }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut out = Snapshot { busy: false, identities: Vec::new() };
        for dir in &self.dirs {
            let common = out.read(dir.join("commondir")).map(|p| dir.join(p.trim())).unwrap_or_else(|| dir.clone());
            // Sampling the index identity also notices an operation that began
            // and ended between polls without moving HEAD (checkout of paths).
            for base in [dir, &common] {
                out.busy |= base.join("index.lock").exists();
                out.note(base.join("index"));
            }
            let mut value = out.read(dir.join("HEAD"));
            out.busy |= value.is_none() || dir.join("HEAD.lock").exists();
            let mut refs = BTreeSet::new();
            while let Some(name) = value.as_deref().and_then(|s| s.trim().strip_prefix("ref:")).map(str::trim) {
                let name = name.to_string();
                if !refs.insert(name.clone()) { out.busy = true; break; }
                let path = common.join(&name);
                out.busy |= path.with_file_name(format!("{}.lock", path.file_name().unwrap_or_default().to_string_lossy())).exists();
                value = out.read(path);
                if value.is_none() {
                    // A ref can be packed; both its value and the packed file's
                    // identity participate in the quiet window.
                    value = out.read(common.join("packed-refs")).and_then(|text| text.lines().find_map(|line| {
                        let (hash, reference) = line.split_once(' ')?;
                        (reference == name).then(|| hash.to_string())
                    }));
                }
            }
        }
        out
    }
}

impl Snapshot {
    fn note(&mut self, path: PathBuf) {
        let stamp = super::stamp(&path.to_string_lossy());
        self.identities.push((path, stamp, None));
    }

    fn read(&mut self, path: PathBuf) -> Option<String> {
        let stamp = super::stamp(&path.to_string_lossy());
        let text = std::fs::read_to_string(&path).ok();
        self.identities.push((path, stamp, text.clone()));
        text
    }
}
