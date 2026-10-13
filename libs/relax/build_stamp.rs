//! Build provenance, shared by the build scripts of the makepad loader and relax-compiler
//! (#[path]; each declares `json` and `srchash` at its root): MAKEPAD_RUN_PROVENANCE =
//! "fastrust <hash> relax <hash>", content hashes of the source files of the path packages the
//! binary compiles, from `cargo metadata` of the package (its library-dependency closure), split into
//! Relax's own (under libs/relax and tools/run) and the makepad crates it uses. Only file contents count:
//! never git state, mtimes or the checkout's location. A commit, checkout or `git add` neither
//! reruns the script nor changes the stamp; the stamp is in the binary, whose bytes are the
//! build id in every Relax cache key.
//! Reruns: every file the hashes read (a deleted one too) and the workspace manifest (it places
//! the workspace dependencies). Nothing else of the workspace or the repository.
use crate::json;
use crate::srchash::{self, SrcPkg};
use std::path::Path;
use std::process::Command;

/// srchash.rs reads through Relax's VFS (`crate::vfs`); a build script has only the disk.
pub mod vfs {
    pub fn read(p: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
        std::fs::read(p)
    }
    pub fn is_file(p: impl AsRef<std::path::Path>) -> bool {
        p.as_ref().is_file()
    }
    pub fn read_dir(p: impl AsRef<std::path::Path>) -> std::io::Result<Vec<(String, bool)>> {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(p)?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
            .collect();
        v.sort();
        Ok(v)
    }
}

/// The resolved package graph of the workspace holding the package at `dir` (its resolved
/// features and library dependencies); `root` is that package.
pub struct Graph {
    pub pkgs: Vec<SrcPkg>,
    pub root: usize,
}

pub fn graph(dir: &Path) -> Graph {
    let target = std::env::var("TARGET").unwrap_or_default();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = dir.join("Cargo.toml");
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--offline", "--filter-platform", &target, "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("cargo metadata");
    assert!(out.status.success(), "cargo metadata: {}", String::from_utf8_lossy(&out.stderr));
    let m = json::parse(&out.stdout).expect("cargo metadata json");
    let packages = m.get("packages").expect("packages").arr();
    let nodes = m.get("resolve").expect("resolve").get("nodes").expect("nodes").arr();
    let ids: Vec<&str> = nodes.iter().map(|n| n.get("id").unwrap().str()).collect();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let own = canon(&manifest);
    let mut pkgs = Vec::new();
    let mut root = None;
    for n in nodes {
        let id = n.get("id").unwrap().str();
        let p = packages.iter().find(|p| p.get("id").unwrap().str() == id).expect("package of node");
        let manifest = p.get("manifest_path").unwrap().str();
        if canon(Path::new(manifest)) == own {
            root = Some(pkgs.len());
        }
        let pdir = Path::new(manifest).parent().unwrap().to_string_lossy().into_owned();
        let features = n.get("features").map(|f| f.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default();
        // library dependencies (proc macros among them); build- and dev-dependencies are not
        // compiled into the crate
        let deps = n
            .get("deps")
            .map(|d| {
                d.arr()
                    .iter()
                    .filter(|d| d.get("dep_kinds").map_or(true, |k| k.arr().iter().any(|k| matches!(k.get("kind"), None | Some(json::Json::Null)))))
                    .filter_map(|d| ids.iter().position(|i| *i == d.get("pkg").unwrap().str()))
                    .collect()
            })
            .unwrap_or_default();
        let proc_macro = p.get("targets").map_or(false, |t| t.arr().iter().any(|t| t.get("kind").map_or(false, |k| k.arr().iter().any(|k| k.str() == "proc-macro"))));
        pkgs.push(SrcPkg { name: p.get("name").unwrap().str().to_string(), version: p.get("version").unwrap().str().to_string(), dir: pdir, features, deps, proc_macro });
    }
    if let Some(ws) = m.get("workspace_root") {
        println!("cargo:rerun-if-changed={}/Cargo.toml", ws.str());
    }
    Graph { pkgs, root: root.unwrap_or_else(|| panic!("{} is not in its cargo metadata", manifest.display())) }
}

impl Graph {
    /// The packages `root` reaches, itself included.
    pub fn closure(&self, root: usize) -> Vec<usize> {
        let mut seen = vec![false; self.pkgs.len()];
        let mut stack = vec![root];
        let mut out = Vec::new();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            out.push(i);
            stack.extend(self.pkgs[i].deps.iter().copied());
        }
        out
    }

    /// Prints MAKEPAD_RUN_PROVENANCE for the package's closure and watches every file it hashes:
    /// each path package's files as srchash.rs lists them (nested packages, `target` and hidden
    /// entries left out) but for its tests, benches and examples, which the binary does not
    /// compile. `extra` joins Relax's own part: inputs the binary depends on outside those
    /// files (the compiler's std digest).
    pub fn provenance(&self, relax_roots: &[&Path], extra: &str) -> String {
        let relax_roots: Vec<std::path::PathBuf> = relax_roots.iter().map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf())).collect();
        let mut relax = vec![{ let mut h = Fnv::new(); h.bytes(extra.as_bytes()); h.0 }];
        let mut host = Vec::new();
        for i in self.closure(self.root) {
            let p = &self.pkgs[i];
            if p.dir.contains("/.cargo/") {
                continue;
            }
            let mut files = Vec::new();
            srchash::files(Path::new(&p.dir), "", &mut files);
            files.retain(|(rel, _)| !["tests/", "benches/", "examples/"].iter().any(|d| rel.starts_with(d)));
            files.sort();
            let mut h = Fnv::new();
            h.bytes(p.name.as_bytes());
            for (rel, f) in &files {
                println!("cargo:rerun-if-changed={}", f.display());
                h.bytes(rel.as_bytes());
                h.bytes(&std::fs::read(f).expect("read package file"));
            }
            let dir = Path::new(&p.dir).canonicalize().unwrap_or_else(|_| p.dir.clone().into());
            if relax_roots.iter().any(|r| dir.starts_with(r)) { relax.push(h.0) } else { host.push(h.0) }
        }
        format!("makepad {} jit {}", stamp(host), stamp(relax))
    }
}

/// FNV-1a, each piece length-prefixed.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for x in (b.len() as u64).to_le_bytes().iter().chain(b) {
            self.0 ^= *x as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
}

fn stamp(mut parts: Vec<u64>) -> String {
    parts.sort_unstable();
    let mut h = Fnv::new();
    for x in parts {
        h.bytes(&x.to_le_bytes());
    }
    format!("{:012x}", h.0 >> 16)
}
