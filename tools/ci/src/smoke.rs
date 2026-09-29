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
    /// A licensed app's slice of the commercial repository (`slice:<app>`).
    pub slice: Option<Slice>,
}
/// One entry of the commercial repository's `slices.json`: an app license
/// and the root packages its slice is cut for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub app: String,
    pub roots: Vec<String>,
}
/// Where the private commercial repository (Stage with Amp, Scope, Sandbox)
/// is checked out inside the Makepad checkout.
pub const COMMERCIAL: &str = "apps/commercial";
/// The script every slice tile runs.
pub const SLICE_SCRIPT: &str = "tools/ci/slice.ci.splash";

/// The licensed slices the commercial repository declares in its root
/// `slices.json`, `{"amp": ["makepad-amp"], "scope": ["makepad-scope"]}`:
/// app license -> root packages. The server's `source.json` roots for each
/// app must match it. None without a commercial checkout.
pub fn slices(root: &Path) -> Result<Vec<Slice>> {
    let path = root.join(COMMERCIAL).join("slices.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let invalid = || format!("{}: expected {{\"<app>\": [\"<root package>\", ...]}}", path.display());
    let value = makepad_strict_json::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let makepad_strict_json::Value::Obj(apps) = value else { return Err(invalid()) };
    let id = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let mut slices = Vec::new();
    for (app, roots) in apps {
        let roots: Vec<String> = roots
            .as_arr()
            .ok_or_else(invalid)?
            .iter()
            .map(|r| r.as_str().filter(|s| id(s)).map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<_>>()?;
        if !id(&app) || roots.is_empty() {
            return Err(invalid());
        }
        slices.push(Slice { app, roots });
    }
    slices.sort_by(|a, b| a.app.cmp(&b.app));
    Ok(slices)
}

/// What a tile is called: `apps/wm` is `wm`, the root script is
/// `workspace`. In the commercial repository a product is named by itself
/// (`apps/commercial/scope` is `scope`, its own app `.../stage/app` is
/// `stage`) and an app of a product by the app (`.../stage/apps/amp` is
/// `amp`); slice tiles are `slice:<app>`.
pub fn tile_name(name: &str) -> String {
    if matches!(name, "." | "") {
        return "workspace".into();
    }
    if let Some(rest) = name.strip_prefix("apps/commercial/") {
        let parts: Vec<&str> = rest.split('/').collect();
        return match parts.as_slice() {
            [product] | [product, "app"] => product.to_string(),
            [_, "apps", app] => app.to_string(),
            [product, .., leaf] => format!("{product}/{leaf}"),
            [] => rest.into(),
        };
    }
    let name = name.strip_prefix("apps/").unwrap_or(name);
    if name.contains('/') {
        let mut parts = name.rsplit('/');
        let leaf = parts.next().unwrap_or(name);
        format!("{}/{leaf}", parts.next().unwrap_or(""))
    } else {
        name.into()
    }
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
            // Hidden directories hold tool state (`.git`, agents' `.claude/worktrees`
            // with whole checkouts), never the checkout's own crates.
            if !matches!(name.as_ref(), "target" | "local") && !name.starts_with("target-") && !name.starts_with('.')
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
            slice: None,
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
                slice: None,
            });
        }
    }
    // One tile per licensed slice: the slice of the tested commercial commit
    // in a clean Makepad checkout, linted and checked (tools/ci/slice.ci.splash).
    for slice in slices(root)? {
        scripts.push(Script {
            path: PathBuf::from(SLICE_SCRIPT),
            name: format!("slice:{}", slice.app),
            target: None,
            slice: Some(slice),
        });
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

    #[test]
    fn commercial_scripts_are_found_at_their_depth_and_every_licensed_slice_is_a_tile() {
        let root = std::env::temp_dir().join(format!("ci-commercial-{}-{}", std::process::id(), crate::report::stamp()));
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("apps/commercial/Cargo.toml", "[package]\nname='makepad-commercial-members'\nversion='0.0.0'\n");
        write("apps/commercial/src/lib.rs", "");
        write("apps/commercial/slices.json", "{\"scope\":[\"makepad-scope\"],\"amp\":[\"makepad-amp\"]}");
        write("apps/commercial/scope/Cargo.toml", "[package]\nname='makepad-scope'\nversion='0.1.0'\n[[bin]]\nname='scope'\npath='src/main.rs'\n");
        write("apps/commercial/scope/src/main.rs", "fn main() {}\n");
        write("apps/commercial/scope/ci.splash", "nil\n");
        write("apps/commercial/stage/apps/amp/Cargo.toml", "[package]\nname='makepad-amp'\nversion='0.1.0'\n");
        write("apps/commercial/stage/apps/amp/src/main.rs", "fn main() {}\n");
        write("apps/commercial/stage/apps/amp/ci.splash", "nil\n");
        write("apps/commercial/local/scratch/ci.splash", "nil\n");
        let config = Config {
            remote: "origin".into(), branches: vec!["work".into()], poll_secs: 60,
            checkout: root.clone(), skip_apps: crate::watch::DEFAULT_SKIPS.iter().map(|s| s.to_string()).collect(), model: "test".into(),
            targets: Vec::new(), allowed_errors: Vec::new(), no_vision: true, parallel: 1, deep_tests: false,
            machines: Default::default(),
        };
        let (targets, scripts) = discover(&root, &config).unwrap();
        assert!(targets.is_empty(), "the members package is no app");
        let names: Vec<(&str, String)> = scripts.iter().map(|s| (s.name.as_str(), tile_name(&s.name))).collect();
        assert_eq!(
            names,
            vec![
                ("apps/commercial/scope", "scope".into()),
                ("apps/commercial/stage/apps/amp", "amp".into()),
                ("slice:amp", "slice:amp".into()),
                ("slice:scope", "slice:scope".into()),
            ]
        );
        let amp = scripts.iter().find(|s| s.name == "slice:amp").unwrap();
        assert_eq!(amp.path, Path::new(SLICE_SCRIPT));
        assert_eq!(amp.slice, Some(Slice { app: "amp".into(), roots: vec!["makepad-amp".into()] }));
        write("apps/commercial/slices.json", "{\"amp\":[]}");
        assert!(discover(&root, &config).is_err(), "a slice without roots stops discovery");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tiles_are_named_by_app() {
        assert_eq!(tile_name("."), "workspace");
        assert_eq!(tile_name("apps/wm"), "wm");
        assert_eq!(tile_name("apps/commercial/scope"), "scope");
        assert_eq!(tile_name("apps/commercial/stage/app"), "stage");
        assert_eq!(tile_name("apps/commercial/stage/apps/amp"), "amp");
        assert_eq!(tile_name("apps/commercial/sandbox"), "sandbox");
        assert_eq!(tile_name("apps/commercial/sandbox/tools/editor"), "sandbox/editor");
        assert_eq!(tile_name("slice:amp"), "slice:amp");
        assert_eq!(tile_name("libs/ai/hub"), "ai/hub");
    }

}
