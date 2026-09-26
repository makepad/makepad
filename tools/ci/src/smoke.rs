use crate::{process::Result, watch::Config};
use makepad_toml_parser::{parse_toml, TomlDocument};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub package: String,
    pub binary: String,
    pub cwd: PathBuf,
    pub manifest: PathBuf,
    pub name: String,
}
#[derive(Clone, Debug)]
pub struct Script {
    pub path: PathBuf,
    pub name: String,
    pub target: Option<Target>,
}
fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ty.is_dir() {
            if !matches!(name.as_ref(), "target" | "local" | ".git") && !name.starts_with("target-")
            {
                walk(root, &entry.path(), files)?;
            }
        } else if name == "Cargo.toml" || name == "ci.splash" {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}
fn skips_package(package: &str, dir: &Path, skip: &[String]) -> bool {
    let short = dir.file_name().unwrap_or_default().to_string_lossy();
    skip.iter().any(|s| {
        s == package
            || s == short.as_ref()
            || s == package.strip_prefix("makepad-").unwrap_or(package)
            || s == &dir.to_string_lossy()
    })
}
fn targets_from_doc(
    doc: &TomlDocument,
    dir: &Path,
    has_main: bool,
    skip: &[String],
) -> Result<Vec<Target>> {
    let Some(package) = doc.get_path(&["package", "name"]).and_then(|v| v.as_str()) else {
        return Ok(Vec::new());
    };
    let short = dir.file_name().unwrap_or_default().to_string_lossy();
    if skips_package(package, dir, skip) {
        return Ok(Vec::new());
    }
    let mut bins = BTreeSet::new();
    if let Some(tables) = doc.root.get("bin").and_then(|t| t.as_array_of_tables()) {
        for table in tables {
            bins.insert(
                table
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(package)
                    .to_string(),
            );
        }
    }
    if has_main && bins.is_empty() {
        bins.insert(package.into());
    }
    let multiple = bins.len() > 1;
    Ok(bins
        .into_iter()
        .map(|binary| Target {
            package: package.into(),
            name: if multiple {
                format!("{short}:{binary}")
            } else {
                short.to_string()
            },
            binary,
            cwd: PathBuf::from("."),
            manifest: dir.join("Cargo.toml"),
        })
        .collect())
}
pub fn discover(root: &Path, config: &Config) -> Result<(Vec<Target>, Vec<Script>)> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    let mut targets = Vec::new();
    let mut skipped_dirs = Vec::new();
    for manifest in files
        .iter()
        // An app is a direct child of apps/: `apps/<name>/Cargo.toml`. Crates
        // nested deeper (a private app's own `deps/*`) are its libraries.
        .filter(|p| {
            p.starts_with("apps")
                && p.components().count() == 3
                && p.file_name().is_some_and(|n| n == "Cargo.toml")
        })
    {
        let text = fs::read_to_string(root.join(manifest)).map_err(|e| e.to_string())?;
        let doc = parse_toml(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let dir = manifest.parent().unwrap();
        let package = doc.get_path(&["package", "name"]).and_then(|v| v.as_str()).unwrap_or("");
        if skips_package(package, dir, &config.skip_apps) {
            skipped_dirs.push(dir.to_path_buf());
        }
        targets.extend(targets_from_doc(
            &doc,
            dir,
            root.join(dir).join("src/main.rs").is_file(),
            &config.skip_apps,
        )?);
    }
    // Skipping an application also skips tools inside its private workspace.
    targets.retain(|target| !skipped_dirs.iter().any(|dir| target.manifest.starts_with(dir)));
    let mut scripts = Vec::new();
    let mut covered = BTreeSet::new();
    for path in files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "ci.splash"))
    {
        if skipped_dirs.iter().any(|dir| path.starts_with(dir)) { continue; }
        let dir = path.parent().unwrap();
        let related: Vec<_> = targets
            .iter()
            .filter(|t| t.manifest.parent() == Some(dir))
            .collect();
        // Library-only packages may also supply a script. Only the explicit
        // skip list suppresses a custom script under apps/.
        if dir.starts_with("apps") {
            let manifest = root.join(dir).join("Cargo.toml");
            if manifest.is_file() {
                let text = fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
                let doc = parse_toml(&text).map_err(|e| e.to_string())?;
                let package = doc
                    .get_path(&["package", "name"])
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if skips_package(package, dir, &config.skip_apps) {
                    continue;
                }
            }
        }
        let target = related.first().map(|t| (*t).clone());
        for t in related {
            covered.insert(t.manifest.clone());
        }
        scripts.push(Script {
            path: path.clone(),
            name: if dir.as_os_str().is_empty() {
                ".".into()
            } else {
                dir.display().to_string()
            },
            target,
        });
    }
    for target in &targets {
        if !covered.contains(&target.manifest) {
            scripts.push(Script {
                path: PathBuf::from("tools/ci/default.ci.splash"),
                name: format!(
                    "{}{}",
                    target.manifest.parent().unwrap().display(),
                    if target.name.contains(':') {
                        format!(":{}", target.binary)
                    } else {
                        String::new()
                    }
                ),
                target: Some(target.clone()),
            });
        }
    }
    Ok((targets, scripts))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_list_uses_manifest_bins_main_and_skip_list() {
        let d =
            parse_toml("[package]\nname='makepad-demo'\n[[bin]]\nname='demo'\npath='src/entry.rs'")
                .unwrap();
        let t = targets_from_doc(&d, Path::new("apps/demo"), false, &[]).unwrap();
        assert_eq!(t[0].binary, "demo");
        assert_eq!(t[0].package, "makepad-demo");
        assert!(
            targets_from_doc(&d, Path::new("apps/demo"), false, &["demo".into()])
                .unwrap()
                .is_empty()
        );
        let d = parse_toml("[package]\nname='plain'").unwrap();
        assert!(targets_from_doc(&d, Path::new("apps/plain"), false, &[])
            .unwrap()
            .is_empty());
        assert_eq!(
            targets_from_doc(&d, Path::new("apps/plain"), true, &[]).unwrap()[0].binary,
            "plain"
        );
    }
    #[test]
    fn warm_app_discovery_excludes_tools_under_skipped_workspaces() {
        let root = std::env::temp_dir().join(format!("ci-discover-{}-{}", std::process::id(), crate::report::stamp()));
        for (dir, name) in [("apps/private", "private"), ("apps/private/tools/helper", "helper"), ("apps/kept", "kept")] {
            let dir = root.join(dir);
            fs::create_dir_all(dir.join("src")).unwrap();
            fs::write(dir.join("Cargo.toml"), format!("[package]\nname='{name}'\nversion='0.1.0'\n")).unwrap();
            fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
            fs::write(dir.join("ci.splash"), "nil\n").unwrap();
        }
        let config = Config {
            remote: "origin".into(), branches: vec!["work".into()], poll_secs: 60,
            checkout: root.clone(), skip_apps: vec!["private".into()], model: "test".into(),
            targets: Vec::new(), allowed_errors: Vec::new(), no_vision: true, parallel: 1, deep_tests: false,
            machines: Default::default(),
        };
        let (targets, scripts) = discover(&root, &config).unwrap();
        assert_eq!(targets.iter().map(|t| t.package.as_str()).collect::<Vec<_>>(), vec!["kept"]);
        assert_eq!(scripts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["apps/kept"]);
        fs::remove_dir_all(root).unwrap();
    }

}
