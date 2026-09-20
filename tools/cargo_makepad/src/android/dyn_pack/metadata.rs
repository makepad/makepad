//! `cargo metadata`, read into the few shapes the packer walks: packages by
//! id (name, version, manifest path, registry or path source, whether a
//! proc-macro), the resolve graph with each edge's dependency kinds, and the
//! features cargo resolved per node.

use makepad_micro_serde::{DeJson, JsonValue};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},

};

pub struct Package {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    /// `None` for a path package; the registry source otherwise.
    pub source: Option<String>,
    pub proc_macro: bool,
    /// The first target's name, as cargo names the unit (`-` kept).
    pub first_target: String,
}

pub struct Dep {
    pub pkg: String,
    /// One entry per `dep_kinds` element: `None` = normal, `Some("dev")`,
    /// `Some("build")`.
    pub kinds: Vec<Option<String>>,
}

pub struct Metadata {
    pub packages: HashMap<String, Package>,
    pub deps: HashMap<String, Vec<Dep>>,
    pub features: HashMap<String, Vec<String>>,
}

/// `cargo metadata --format-version 1 <extra>` in `cwd`, under the controlled
/// environment plus `env`.
pub fn cargo_metadata(cwd: &Path, extra: &[&str], env: &[(String, String)]) -> Result<Metadata, String> {
    let mut cmd = super::controlled_command("cargo", env);
    cmd.args(["metadata", "--format-version", "1"]).args(extra).current_dir(cwd);
    let out = cmd.output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata {} in {} failed:\n{}",
            extra.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8(out.stdout).map_err(|_| "cargo metadata: not utf-8".to_string())?;
    let json = JsonValue::deserialize_json(&text).map_err(|e| format!("cargo metadata json: {e}"))?;
    parse(&json)
}

fn parse(json: &JsonValue) -> Result<Metadata, String> {
    let mut packages = HashMap::new();
    for p in json.get("packages").and_then(|v| v.as_array()).ok_or("metadata: packages")? {
        let id = str_of(p, "id")?;
        let targets = p.get("targets").and_then(|v| v.as_array()).ok_or("metadata: targets")?;
        let proc_macro = targets.iter().any(|t| {
            t.get("kind")
                .and_then(|k| k.as_array())
                .map(|kinds| kinds.iter().any(|k| k.as_str() == Some("proc-macro")))
                .unwrap_or(false)
        });
        let first_target = targets
            .first()
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        packages.insert(
            id,
            Package {
                name: str_of(p, "name")?,
                version: str_of(p, "version")?,
                manifest_path: PathBuf::from(str_of(p, "manifest_path")?),
                source: p.get("source").and_then(|s| s.as_str()).map(str::to_string),
                proc_macro,
                first_target,
            },
        );
    }
    let mut deps = HashMap::new();
    let mut features = HashMap::new();
    let nodes = json
        .get("resolve")
        .and_then(|r| r.get("nodes"))
        .and_then(|n| n.as_array())
        .ok_or("metadata: resolve.nodes")?;
    for node in nodes {
        let id = str_of(node, "id")?;
        let mut edges = Vec::new();
        for d in node.get("deps").and_then(|v| v.as_array()).ok_or("metadata: node deps")? {
            let kinds = d
                .get("dep_kinds")
                .and_then(|v| v.as_array())
                .map(|ks| ks.iter().map(|k| k.get("kind").and_then(|k| k.as_str()).map(str::to_string)).collect())
                .unwrap_or_default();
            edges.push(Dep { pkg: str_of(d, "pkg")?, kinds });
        }
        let feats = node
            .get("features")
            .and_then(|v| v.as_array())
            .map(|fs| fs.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        deps.insert(id.clone(), edges);
        features.insert(id, feats);
    }
    Ok(Metadata { packages, deps, features })
}

fn str_of(v: &JsonValue, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("metadata: missing {key}"))
}

impl Metadata {
    /// The id of the package named `name` (a workspace has one of each).
    pub fn id_of(&self, name: &str) -> Result<&str, String> {
        self.packages
            .iter()
            .find(|(_, p)| p.name == name)
            .map(|(id, _)| id.as_str())
            .ok_or_else(|| format!("package {name} is not in the workspace"))
    }

    /// Package ids reachable from `roots` over non-dev edges.
    pub fn closure(&self, roots: &[&str]) -> Result<BTreeSet<String>, String> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = roots.iter().map(|r| self.id_of(r).map(str::to_string)).collect::<Result<_, _>>()?;
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for d in self.deps.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
                // Python's `all(kind == "dev" for kind in dep_kinds)`: an empty
                // list is skipped too.
                if d.kinds.iter().all(|k| k.as_deref() == Some("dev")) {
                    continue;
                }
                stack.push(d.pkg.clone());
            }
        }
        Ok(seen)
    }

    /// Package ids reachable from `id` over NORMAL edges only (the crates a
    /// proc-macro's own build pulls in).
    pub fn normal_closure(&self, roots: &[String]) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = roots.to_vec();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            for d in self.deps.get(&id).map(Vec::as_slice).unwrap_or(&[]) {
                if d.kinds.iter().any(|k| k.is_none()) {
                    stack.push(d.pkg.clone());
                }
            }
        }
        seen
    }

    pub fn package(&self, id: &str) -> Result<&Package, String> {
        self.packages.get(id).ok_or_else(|| format!("metadata: unknown package id {id}"))
    }

    /// The package directory (the manifest's parent).
    pub fn dir(&self, id: &str) -> Result<&Path, String> {
        self.package(id)?
            .manifest_path
            .parent()
            .ok_or_else(|| format!("metadata: manifest without a directory: {id}"))
    }
}
