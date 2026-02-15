/// Cargo metadata parsing and package discovery.

use makepad_micro_serde::*;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::error::{CheckError, CheckResult};

/// Cargo metadata JSON structure.
#[derive(Debug, DeJson)]
pub struct CargoMetadata {
    pub packages: Vec<CargoPackage>,
    pub workspace_members: Vec<String>,
    pub workspace_default_members: Option<Vec<String>>,
}

/// Package information from cargo metadata.
#[derive(Debug, DeJson)]
pub struct CargoPackage {
    pub id: String,
    pub name: String,
    pub manifest_path: String,
    pub dependencies: Vec<CargoDependency>,
    pub targets: Vec<CargoTarget>,
}

/// Build target information.
#[derive(Debug, DeJson)]
pub struct CargoTarget {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: String,
}

/// Dependency information.
#[derive(Debug, DeJson)]
pub struct CargoDependency {
    pub name: String,
    pub req: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub uses_default_features: bool,
    pub features: Vec<String>,
    pub optional: bool,
    pub target: Option<String>,
}

impl CargoPackage {
    /// Find the library target for this package.
    pub fn lib_target(&self) -> Option<&CargoTarget> {
        self.targets.iter().find(|t| t.kind.iter().any(|k| k == "lib"))
    }

    /// Find a non-optional, non-target-specific dependency by name.
    pub fn find_dependency(&self, name: &str) -> Option<&CargoDependency> {
        self.dependencies.iter().find(|dep| {
            dep.name == name && !dep.optional && dep.target.is_none()
        })
    }
}

impl CargoDependency {
    /// Generate a TOML dependency entry string for this dependency.
    pub fn to_toml_entry(&self, alias: &str) -> String {
        let mut fields = Vec::<String>::new();
        fields.push(format!("package = \"{}\"", self.name));

        if let Some(path) = &self.path {
            fields.push(format!("path = \"{}\"", path));
        } else if let Some(source) = &self.source {
            if source.starts_with("git+") {
                let (url, branch, tag, rev) = parse_git_source(source);
                fields.push(format!("git = \"{}\"", url));
                if let Some(b) = branch {
                    fields.push(format!("branch = \"{}\"", b));
                }
                if let Some(t) = tag {
                    fields.push(format!("tag = \"{}\"", t));
                }
                if let Some(r) = rev {
                    fields.push(format!("rev = \"{}\"", r));
                }
            } else {
                fields.push(format!("version = \"{}\"", self.req));
            }
        } else {
            fields.push(format!("version = \"{}\"", self.req));
        }

        if !self.uses_default_features {
            fields.push("default-features = false".to_string());
        }

        if !self.features.is_empty() {
            let features: String = self.features
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<_>>()
                .join(", ");
            fields.push(format!("features = [{}]", features));
        }

        format!("{} = {{ {} }}\n", alias, fields.join(", "))
    }
}

/// Parse a git source URL into (url, branch, tag, rev).
fn parse_git_source(source: &str) -> (String, Option<String>, Option<String>, Option<String>) {
    let mut raw = source.strip_prefix("git+").unwrap_or(source);
    let mut fragment = None;

    // Extract fragment (commit sha after #)
    if let Some((before_hash, frag)) = raw.split_once('#') {
        raw = before_hash;
        if !frag.is_empty() {
            fragment = Some(frag.to_string());
        }
    }

    // Split URL and query string
    let (url, query) = if let Some((u, q)) = raw.split_once('?') {
        (u.to_string(), Some(q))
    } else {
        (raw.to_string(), None)
    };

    let mut branch = None;
    let mut tag = None;
    let mut rev = None;

    // Parse query parameters
    if let Some(query) = query {
        for kv in query.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                if v.is_empty() {
                    continue;
                }
                match k {
                    "branch" => branch = Some(v.to_string()),
                    "tag" => tag = Some(v.to_string()),
                    "rev" => rev = Some(v.to_string()),
                    _ => {}
                }
            }
        }
    }

    // Use fragment as rev if no explicit ref is specified
    if rev.is_none() && branch.is_none() && tag.is_none() {
        rev = fragment;
    }

    (url, branch, tag, rev)
}

/// Canonicalize a path, falling back to the original if it fails.
pub fn canonicalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Find the Cargo.toml manifest by walking up from the given directory.
pub fn find_manifest_path(cwd: &Path) -> Option<PathBuf> {
    let mut cur = Some(cwd);
    while let Some(dir) = cur {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Read cargo metadata for a manifest.
pub fn read_cargo_metadata(manifest_path: &Path) -> CheckResult<CargoMetadata> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .map_err(|e| CheckError::CargoMetadataError(format!("Failed to run cargo metadata: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CheckError::CargoMetadataError(format!(
            "cargo metadata failed: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    DeJson::deserialize_json_lenient(&stdout)
        .map_err(|e| CheckError::CargoMetadataError(format!("Failed to parse cargo metadata JSON: {:?}", e)))
}

/// Select the appropriate package from metadata based on manifest path and cwd.
pub fn select_package<'a>(
    metadata: &'a CargoMetadata,
    manifest_path: &Path,
    cwd: &Path,
) -> Option<&'a CargoPackage> {
    let manifest_canon = canonicalize_path(manifest_path);

    // First, try exact manifest match
    if let Some(pkg) = metadata.packages.iter().find(|p| {
        canonicalize_path(Path::new(&p.manifest_path)) == manifest_canon
    }) {
        return Some(pkg);
    }

    // Second, find packages whose directory contains cwd
    let cwd_canon = canonicalize_path(cwd);
    let mut candidates: Vec<&CargoPackage> = metadata.packages
        .iter()
        .filter(|p| {
            let package_dir = Path::new(&p.manifest_path)
                .parent()
                .map(canonicalize_path)
                .unwrap_or_else(|| PathBuf::from(&p.manifest_path));
            cwd_canon.starts_with(package_dir)
        })
        .collect();

    // Sort by path length (most specific first)
    candidates.sort_by_key(|p| std::cmp::Reverse(p.manifest_path.len()));
    if let Some(pkg) = candidates.first() {
        return Some(pkg);
    }

    // Fall back to workspace default members
    if let Some(default_members) = &metadata.workspace_default_members {
        for member in default_members {
            if let Some(pkg) = metadata.packages.iter().find(|p| &p.id == member) {
                return Some(pkg);
            }
        }
    } else if let Some(member) = metadata.workspace_members.first() {
        if let Some(pkg) = metadata.packages.iter().find(|p| &p.id == member) {
            return Some(pkg);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_source_with_branch() {
        let source = "git+https://github.com/makepad/makepad?branch=main#abc123";
        let (url, branch, tag, rev) = parse_git_source(source);
        assert_eq!(url, "https://github.com/makepad/makepad");
        assert_eq!(branch, Some("main".to_string()));
        assert_eq!(tag, None);
        assert_eq!(rev, None);
    }

    #[test]
    fn test_parse_git_source_with_rev() {
        let source = "git+https://github.com/makepad/makepad#abc123";
        let (url, branch, tag, rev) = parse_git_source(source);
        assert_eq!(url, "https://github.com/makepad/makepad");
        assert_eq!(branch, None);
        assert_eq!(tag, None);
        assert_eq!(rev, Some("abc123".to_string()));
    }
}
