//! The relocatable checkout the Android super-app ships and builds against:
//! `<stage>/src`.
//!
//! What goes in: every path package reachable (any platform, non-dev edges)
//! from the host, its engine and the tile apps — git-tracked plus
//! untracked-but-not-ignored files of each package directory — under a
//! generated root Cargo.toml whose members are those roots (path deps inside
//! the tree are implicit members). Registry crates are vendored (the phone has
//! no network; cargo must resolve the whole lock offline). The registry crates
//! that the Android target actually compiles become `[patch.crates-io]` PATH
//! packages: cargo hashes a registry package's absolute src path into its
//! fingerprint, a path package only its name — the tree relocates only if
//! those are path packages on both the Mac and the phone.
//!
//! The Mac cross-build runs FROM this tree (`CARGO_TARGET_DIR=<stage>/target`),
//! so the fingerprints in that target are the ones the on-device cargo
//! recomputes. The tree's `.cargo/config.toml` makes feature resolution
//! workspace-wide (`feature-unification = "workspace"`, nightly-gated: every
//! cargo that touches the tree runs with `RUSTC_BOOTSTRAP=1`), so a `-p <app>`
//! build on the phone resolves the very units the engine cross-build
//! compiled — no app manifest names the engine; the host forces it into the
//! app's crate graph at the rustc level (apps/wm/src/dylib_host.rs).
//!
//! Mtimes are part of that identity: every file gets a whole-second mtime and
//! each package's Cargo.toml is its newest file ([`normalize_mtimes`]), because
//! the phone restores tar mtimes without nanoseconds and cargo fingerprints
//! build scripts without `rerun-if-*` by the newest file's mtime string.

use super::metadata::{cargo_metadata, Metadata};
use super::{Dyn, Kind, Wrapper, PROC_APPS};
use makepad_toml_parser::{parse_toml, Toml, TomlTable};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const BOOTSTRAP: &str = "wmdyn-bootstrap";
const MARKER: &str = "local/README";
const MARKER_TEXT: &str =
    "Makepad on-device checkout marker: the WM looks for Cargo.toml beside a local/ dir.\n";

/// Stage the tree. Prints what stage.py printed: package and file counts,
/// what was copied, what was vendored, the android registry patches and the
/// proc-macros, the mtime normalization.
pub fn stage(d: &Dyn) -> Result<(), String> {
    let repo = &d.checkout;
    let src = &d.src;
    let roots: Vec<&str> = d.roots();
    // The controlled env (RUSTC_BOOTSTRAP=1 for the stage's nightly-gated
    // config, the target dir, our wrapper) for every cargo of the stage.
    let env = d.cargo_env(&d.target)?;

    let m = cargo_metadata(repo, &[], &env)?;
    if !d.is_proc() {
        check_lib_names(&m, &d.engine, &d.apps)?;
    }
    let all_ids = m.closure(&roots)?;
    let mut path_ids: Vec<&String> = Vec::new();
    let mut registry: BTreeSet<(String, String)> = BTreeSet::new();
    let mut member_dirs: Vec<String> = Vec::new();
    let mut pkg_dirs: BTreeSet<String> = BTreeSet::new();
    for i in &all_ids {
        let p = m.package(i)?;
        if p.source.is_none() {
            path_ids.push(i);
            pkg_dirs.insert(rel_dir(repo, m.dir(i)?)?);
        } else {
            registry.insert((p.name.clone(), p.version.clone()));
        }
        if roots.contains(&p.name.as_str()) {
            member_dirs.push(rel_dir(repo, m.dir(i)?)?);
        }
    }
    member_dirs.sort();
    // Resolve closure (what compiles) ∪ manifest walk (what cargo must load).
    pkg_dirs.extend(manifest_walk(repo, &member_dirs, &workspace_exclude(repo)?)?);
    let pkg_dirs: Vec<String> = pkg_dirs.into_iter().collect();

    let mut files = git_files(repo, &pkg_dirs)?;
    for extra in ["rust-toolchain", "rust-toolchain.toml"] {
        if repo.join(extra).is_file() {
            files.push(extra.to_string());
        }
    }
    println!(
        "path packages {} in {} dirs, {} files; registry (all platforms) {}",
        path_ids.len(),
        pkg_dirs.len(),
        files.len(),
        registry.len()
    );
    let (mut copied, total) = sync_tree(repo, src, &files)?;
    println!("tree {:.0} MB, {} files copied", total as f64 / 1e6, copied.len());
    // Proc: the generated wrapper crates (one cdylib per hosted app) are
    // members of the staged workspace; the phone builds them with `-p`.
    let mut wrapper_packages: Vec<String> = Vec::new();
    if let Kind::Proc { wrappers } = &d.kind {
        let checkout = repo.canonicalize().map_err(|e| format!("{}: {e}", repo.display()))?;
        for w in wrappers {
            let app_dir = m.dir(m.id_of(&w.app)?)?;
            if let Some(path) = write_wrapper(src, &checkout, app_dir, w)? {
                copied.push(path);
            }
            member_dirs.push(w.dir());
            wrapper_packages.push(w.package());
        }
        member_dirs.sort();
    }

    fs::create_dir_all(src.join("local")).map_err(|e| format!("mkdir local: {e}"))?;
    fs::write(src.join(MARKER), MARKER_TEXT).map_err(|e| format!("{MARKER}: {e}"))?;
    // 1. Root manifest without the android patches, so `cargo vendor` can resolve.
    let orig = fs::read_to_string(repo.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    fs::write(src.join("Cargo.toml"), root_manifest(&orig, &member_dirs, &[], false, &BTreeMap::new())?)
        .map_err(|e| format!("stage Cargo.toml: {e}"))?;
    let lock = repo.join("Cargo.lock");
    let vendored_lock = d.stage.join("vendor.lock"); // the checkout lock vendor/ was made from
    if lock.is_file() {
        fs::copy(&lock, src.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?; // pins the same versions
    }
    if !(src.join("vendor").is_dir() && vendored_lock.is_file() && same_content(&lock, &vendored_lock)?) {
        // `cargo vendor` rewrites every vendored file (new mtimes: the patched
        // crates and the engine above them would rebuild): only when the lock moved.
        let status = d
            .command("cargo", &d.target)?
            .args(["vendor", "--versioned-dirs", "--quiet", "vendor"])
            .current_dir(src)
            .stdout(Stdio::null())
            .status()
            .map_err(|e| format!("cargo vendor: {e}"))?;
        if !status.success() {
            return Err(format!("cargo vendor failed ({status})"));
        }
        fs::copy(&lock, &vendored_lock).map_err(|e| format!("vendor.lock: {e}"))?;
        println!("vendored");
    }
    // Registry crates the Android target compiles in the super-app graph (the
    // apps without their `standalone` defaults): these become path patches.
    let ma = if d.is_proc() {
        cargo_metadata(src, &["--filter-platform", d.triple()], &env)?
    } else {
        cargo_metadata(src, &["--filter-platform", d.triple(), "--no-default-features"], &env)?
    };
    let mut android_roots: Vec<&str> = roots.clone();
    android_roots.extend(wrapper_packages.iter().map(String::as_str));
    let android_ids = ma.closure(&android_roots)?;
    let android_registry: Vec<(String, String)> = android_ids
        .iter()
        .filter_map(|i| ma.packages.get(i))
        .filter(|p| p.source.is_some())
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // The host-kind proc-macro units the device rebuilds, with the feature
    // set cargo resolved for them in the app graph: built as roots with
    // their defaults they would be different units (other hash), and the
    // engine's fingerprints name these exact ones.
    let mut pm_ids: Vec<String> = android_ids
        .iter()
        .filter(|i| ma.package(i).map(|p| p.source.is_none() && p.proc_macro).unwrap_or(false))
        .cloned()
        .collect();
    pm_ids.sort_by(|a, b| ma.packages[a].name.cmp(&ma.packages[b].name));
    let proc_macros: Vec<String> = pm_ids
        .iter()
        .map(|i| format!("{} {}", ma.packages[i].name, ma.features.get(i).map(|f| f.join(",")).unwrap_or_default()).trim_end().to_string())
        .collect();
    println!("android registry (patched to path) {android_registry:?}; proc-macros {proc_macros:?}");
    // The device rebuilds the proc-macros (host units) through this crate:
    // as dependencies they get the build-override profile and exactly these
    // features, i.e. the same unit hashes the engine's fingerprints name. As
    // `-p <proc-macro>` roots they would get the release profile and their
    // own defaults — different units.
    let boot = src.join(BOOTSTRAP);
    fs::create_dir_all(boot.join("src")).map_err(|e| format!("{BOOTSTRAP}: {e}"))?;
    let mut deps = String::new();
    for i in &pm_ids {
        let rel = relative_path(&boot, ma.dir(i)?);
        let features = ma
            .features
            .get(i)
            .map(|f| f.iter().map(|x| format!("\"{x}\"")).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        deps.push_str(&format!(
            "{} = {{ path = \"{rel}\", default-features = false, features = [{features}] }}\n",
            ma.packages[i].name
        ));
    }
    fs::write(
        boot.join("Cargo.toml"),
        format!(
            "# GENERATED by `cargo makepad android dyn-pack`: builds the proc-macros the phone\n\
             # must rebuild (host units) as dependencies with their resolved features.\n\
             [package]\nname = \"{BOOTSTRAP}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n\n\
             [dependencies]\n{deps}"
        ),
    )
    .map_err(|e| format!("{BOOTSTRAP}/Cargo.toml: {e}"))?;
    fs::write(boot.join("src/lib.rs"), "//! GENERATED: nothing to see; the dependencies are the point.\n")
        .map_err(|e| format!("{BOOTSTRAP}/src/lib.rs: {e}"))?;
    // 2. Now the android registry crates as path packages, offline re-resolve.
    let mut path_by_name: BTreeMap<String, (String, String)> = BTreeMap::new();
    for i in &path_ids {
        path_by_name.insert(m.packages[*i].name.clone(), (m.packages[*i].version.clone(), rel_dir(repo, m.dir(i)?)?));
    }
    let mut members = member_dirs.clone();
    members.push(BOOTSTRAP.to_string());
    fs::write(src.join("Cargo.toml"), root_manifest(&orig, &members, &android_registry, true, &path_by_name)?)
        .map_err(|e| format!("stage Cargo.toml: {e}"))?;
    fs::create_dir_all(src.join(".cargo")).map_err(|e| format!(".cargo: {e}"))?;
    fs::write(
        src.join(".cargo/config.toml"),
        "# GENERATED by `cargo makepad android dyn-pack`: crates.io from the vendored\n\
         # tree, on the Mac and on the phone alike (paths are config-relative).\n\
         [source.crates-io]\nreplace-with = \"vendored-sources\"\n\n\
         [source.vendored-sources]\ndirectory = \"vendor\"\n\n[net]\noffline = true\n\n\
         # Features resolve across ALL members, whichever package is selected:\n\
         # the engine cross-build (-p makepad-wm-dyn) and every on-device\n\
         # `-p <app>` build then share one unit graph, and the app's manifest\n\
         # needs no engine dependency. Nightly-gated on this cargo: honored only\n\
         # with RUSTC_BOOTSTRAP=1 in the environment (dyn-pack, env.txt), and\n\
         # that flag is in every crate's SVH, so it is set on both sides or on\n\
         # neither. `--no-default-features` applies to every member here.\n\
         [unstable]\nfeature-unification = true\n\n[resolver]\nfeature-unification = \"workspace\"\n",
    )
    .map_err(|e| format!(".cargo/config.toml: {e}"))?;
    let status = d
        .command("cargo", &d.target)?
        .args(["generate-lockfile", "--offline"])
        .current_dir(src)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("cargo generate-lockfile: {e}"))?;
    if !status.success() {
        return Err(format!("cargo generate-lockfile --offline failed ({status}) in {}", src.display()));
    }
    fs::write(d.stage.join("proc-macros.txt"), format!("{}\n", proc_macros.join("\n")))
        .map_err(|e| format!("proc-macros.txt: {e}"))?;
    // Host units the phone rebuilds (and the pack strips): the proc-macros
    // and everything they depend on. Other host units — build scripts and
    // THEIR deps — stay as the Mac built them: Fresh, never executed.
    let strip = ma.normal_closure(&pm_ids);
    let mut lines: Vec<String> = strip
        .iter()
        .map(|i| format!("{} {}\n", ma.packages[i].name, ma.packages[i].first_target.replace('-', "_")))
        .collect();
    lines.sort();
    fs::write(d.stage.join("host-strip.txt"), lines.concat()).map_err(|e| format!("host-strip.txt: {e}"))?;
    let fresh: HashSet<PathBuf> = copied.into_iter().collect();
    let (set_now, truncated, manifests) = normalize_mtimes(src, &fresh)?;
    println!("mtimes: {set_now} files -> now, {truncated} truncated to the second, {manifests} Cargo.toml made newest");
    println!("staged {}; proc-macros -> {}", src.display(), d.stage.join("proc-macros.txt").display());
    Ok(())
}

/// Proc: `<src>/proc-apps/app_<bin>/Cargo.toml`, the hosted app's library:
/// its `src/main.rs` (the `app_main!` whose `makepad_hosted_main` the
/// launcher calls) as a cdylib, depending on the app's library and on the
/// app's own dependencies — the manifest sections proc-pack's checkout
/// wrapper copies, with every path made relative to the staged tree (the
/// phone builds the same file from `files/src`). Written only when its
/// content changed; returns the path then (its mtime becomes now).
fn write_wrapper(src: &Path, checkout: &Path, app_dir: &Path, w: &Wrapper) -> Result<Option<PathBuf>, String> {
    let manifest = fs::read_to_string(app_dir.join("Cargo.toml")).map_err(|e| format!("{}: {e}", app_dir.display()))?;
    if !app_dir.join("src/main.rs").is_file() {
        return Err(format!("{} has no src/main.rs to host", w.app));
    }
    let up = "../../";
    let abs = |p: &Path| -> Result<String, String> {
        let p = p.canonicalize().map_err(|e| format!("{}: {e}", p.display()))?;
        let rel = p.strip_prefix(checkout).map_err(|_| format!("{}: outside the checkout", p.display()))?;
        Ok(format!("{up}{}", utf8(rel)?.replace('\\', "/")))
    };
    let app_rel = abs(app_dir)?;
    let sections = crate::android::proc_pack::copied_sections(&manifest);
    let absolute = crate::android::compile::rewrite_wrapper_manifest_paths(&sections, app_dir);
    let prefix = format!("\"{}/", utf8(checkout)?);
    let relative = absolute.replace(&prefix, &format!("\"{up}"));
    if relative.contains(utf8(checkout)?) {
        return Err(format!("{}: a dependency path stays absolute after relocation", w.app));
    }
    let mut out = format!(
        "# GENERATED by `cargo makepad android proc-pack`: the hosted app's library\n\
         # (its src/main.rs as a cdylib), built on the phone with `-p {pkg}`.\n\
         [package]\nname = \"{pkg}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [lib]\nname = \"{lib}\"\npath = \"{app_rel}/src/main.rs\"\ncrate-type = [\"cdylib\"]\n",
        pkg = w.package(),
        lib = w.lib(),
    );
    let dep_line = format!("{} = {{ path = \"{app_rel}\" }}\n", w.app);
    if relative.contains("[dependencies]\n") {
        out.push_str(&relative.replacen("[dependencies]\n", &format!("[dependencies]\n{dep_line}"), 1));
    } else {
        out.push_str(&format!("\n[dependencies]\n{dep_line}{relative}"));
    }
    let dir = src.join(w.dir());
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("Cargo.toml");
    if fs::read_to_string(&path).ok().as_deref() == Some(out.as_str()) {
        return Ok(None);
    }
    fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
}

/// A package directory relative to the checkout, `/`-separated. Not UTF-8
/// or not under the checkout is an error: the name goes into manifests.
fn rel_dir(repo: &Path, dir: &Path) -> Result<String, String> {
    let rel = dir.strip_prefix(repo).map_err(|_| format!("{}: not under the checkout {}", dir.display(), repo.display()))?;
    Ok(utf8(rel)?.replace('\\', "/"))
}

/// A path as text; a path that is not UTF-8 is an error, never a lossy
/// stand-in (it would be staged under another name or dropped).
fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("{}: path is not UTF-8", path.display()))
}

/// The device names an app's dylib `lib<package with _>.so`
/// (apps/wm/src/dylib_host.rs `lib_stem`) and the engine
/// `lib<engine with _>.so`: a `[lib].name` that says otherwise would build a
/// file the device never finds. Refuse it here, before anything is staged.
fn check_lib_names(m: &Metadata, engine: &str, apps: &[String]) -> Result<(), String> {
    for pkg in [engine].into_iter().chain(apps.iter().map(String::as_str)) {
        let manifest = &m.package(m.id_of(pkg)?)?.manifest_path;
        let text = fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let toml = parse_toml(&text).map_err(|e| format!("{}: {}", manifest.display(), e.msg))?;
        if let Some(name) = toml.get_path(&["lib", "name"]).and_then(Toml::as_str) {
            let expected = pkg.replace('-', "_");
            if name != expected {
                return Err(format!(
                    "{}: [lib] name = \"{name}\" but the device loads lib{expected}.so (the package name with `-` -> `_`); rename the lib or the package",
                    manifest.display()
                ));
            }
        }
    }
    Ok(())
}

fn workspace_exclude(repo: &Path) -> Result<BTreeSet<String>, String> {
    let text = fs::read_to_string(repo.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let toml = parse_toml(&text).map_err(|e| format!("Cargo.toml: {}", e.msg))?;
    Ok(toml
        .get_path(&["workspace", "exclude"])
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(normpath).collect())
        .unwrap_or_default())
}

/// `os.path.normpath` for `/`-separated relative paths.
pub fn normpath(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if matches!(out.last(), Some(&"..")) || out.is_empty() {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            p => out.push(p),
        }
    }
    if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    }
}

/// Package dirs cargo loads when it walks path dependencies from the
/// members (`Workspace::find_path_deps`): every declared path dep of every
/// kind and platform, optional or not — except that an excluded package is
/// loaded but its own deps are not walked.
fn manifest_walk(repo: &Path, root_dirs: &[String], exclude: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = root_dirs.to_vec();
    while let Some(d) = stack.pop() {
        let d = normpath(&d);
        if !seen.insert(d.clone()) {
            continue;
        }
        if exclude.contains(&d) {
            continue;
        }
        let manifest = repo.join(&d).join("Cargo.toml");
        let text = fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let toml = parse_toml(&text).map_err(|e| format!("{}: {}", manifest.display(), e.msg))?;
        let mut tables: Vec<&TomlTable> = Vec::new();
        for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(t) = toml.get_path(&[kind]).and_then(|v| v.as_table()) {
                tables.push(t);
            }
        }
        if let Some(targets) = toml.get_path(&["target"]).and_then(|v| v.as_table()) {
            for t in targets.values().filter_map(|v| v.as_table()) {
                for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(deps) = t.get(kind).and_then(|v| v.as_table()) {
                        tables.push(deps);
                    }
                }
            }
        }
        for table in tables {
            for (name, spec) in table {
                let Some(path) = spec.as_table().and_then(|s| s.get("path")).and_then(Toml::as_str) else {
                    continue;
                };
                let dep = normpath(&format!("{d}/{path}"));
                if !repo.join(&dep).join("Cargo.toml").is_file() {
                    return Err(format!("{d}: path dependency {name} -> {dep} has no Cargo.toml"));
                }
                stack.push(dep);
            }
        }
    }
    Ok(seen)
}

/// Tracked and untracked-but-not-ignored files under `dirs`, sorted; never
/// anything under a `target/`.
fn git_files(repo: &Path, dirs: &[String]) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"])
        .args(dirs)
        .current_dir(repo)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut files = BTreeSet::new();
    for p in out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let f = String::from_utf8(p.to_vec()).map_err(|_| format!("git ls-files: a path is not UTF-8: {}", String::from_utf8_lossy(p)))?;
        files.insert(f);
    }
    files.retain(|f| repo.join(f).is_file() && !f.contains("/target/"));
    Ok(files.into_iter().collect())
}

/// The checkout's root Cargo.toml reshaped for the stage: the members, the
/// checkout's `workspace.exclude` block, and everything from
/// `[patch.crates-io]` on (patches, profiles) with the android registry
/// patches inserted when asked.
fn root_manifest(
    orig: &str,
    member_dirs: &[String],
    android_registry: &[(String, String)],
    with_patches: bool,
    path_by_name: &BTreeMap<String, (String, String)>,
) -> Result<String, String> {
    let patch_at = orig.find("[patch.crates-io]").ok_or("Cargo.toml: no [patch.crates-io] section")?;
    let mut tail = orig[patch_at..].lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>().join("\n") + "\n";
    if with_patches {
        // A name the checkout already patches (e.g. bitflags -> libs/bitflags
        // 2.10, while rustix wants 2.11 from the registry) gets cargo's
        // multi-version form: a renamed key with `package = "<name>"`.
        let block = match tail[1..].find("\n[") {
            Some(i) => &tail[..i + 1],
            None => &tail,
        };
        let taken: BTreeSet<String> = block
            .lines()
            .skip(1)
            .filter(|l| l.contains('='))
            .map(|l| l.split_once('=').unwrap().0.trim().to_string())
            .collect();
        // A registry crate the tree also carries as a path package of the same
        // version (libs/cfg-if 1.0.4 next to flate2's registry cfg-if) is
        // patched to THAT package: two path packages of one name+version
        // cannot share a lockfile.
        let mut patches = String::new();
        for (name, ver) in android_registry {
            let same = path_by_name.get(name).map(|(v, _)| v == ver).unwrap_or(false);
            if taken.contains(name) {
                if same {
                    continue;
                }
                patches.push_str(&format!(
                    "{name}-{} = {{ package = \"{name}\", path = \"vendor/{name}-{ver}\" }}\n",
                    ver.replace('.', "_")
                ));
            } else if same {
                patches.push_str(&format!("{name} = {{ path = \"{}\" }}\n", path_by_name[name].1));
            } else {
                patches.push_str(&format!("{name} = {{ path = \"vendor/{name}-{ver}\" }}\n"));
            }
        }
        tail = tail.replacen("[patch.crates-io]\n", &format!("[patch.crates-io]\n{patches}"), 1);
    }
    let members: String = member_dirs.iter().map(|d| format!("    \"{d}\",\n")).collect();
    // The checkout's `workspace.exclude` block, verbatim (a top-level dotted
    // key, like the checkout's root): excluded path packages are not walked
    // as members, which is what lets a vendored lib such as
    // libs/unicode/unicode-bidi carry an optional `../smallvec` path dep
    // that does not exist in the tree.
    let start = orig.find("workspace.exclude = [").ok_or("Cargo.toml: no workspace.exclude block")?;
    let end = orig[start..].find("\n]").ok_or("Cargo.toml: unterminated workspace.exclude")? + start + 2;
    let exclude = orig[start..end].lines().filter(|l| !l.trim().starts_with('#')).collect::<Vec<_>>().join("\n") + "\n";
    Ok(format!(
        "# GENERATED by `cargo makepad android dyn-pack` — the Android super-app workspace.\n\
         # Members are the roots; every path dependency inside the tree is an\n\
         # implicit member. Profiles, patches and excludes are the checkout's.\n\
         workspace.members = [\n{members}]\nworkspace.resolver = \"2\"\n\n{exclude}\n{tail}"
    ))
}

fn same_content(a: &Path, b: &Path) -> Result<bool, String> {
    let (ma, mb) = (fs::metadata(a), fs::metadata(b));
    let (Ok(ma), Ok(mb)) = (ma, mb) else { return Ok(false) };
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let da = fs::read(a).map_err(|e| format!("{}: {e}", a.display()))?;
    let db = fs::read(b).map_err(|e| format!("{}: {e}", b.display()))?;
    Ok(da == db)
}

/// Copy the checkout's files into the stage; returns (paths copied, bytes).
/// Content decides what is copied (the staged mtimes are normalized, so
/// they never match the checkout's).
fn sync_tree(repo: &Path, src: &Path, files: &[String]) -> Result<(Vec<PathBuf>, u64), String> {
    let mut copied = Vec::new();
    let mut total = 0u64;
    let keep: HashSet<&str> = files.iter().map(String::as_str).collect();
    for f in files {
        let s = repo.join(f);
        let d = src.join(f);
        let size = fs::metadata(&s).map_err(|e| format!("{}: {e}", s.display()))?.len();
        total += size;
        if d.is_file() && fs::metadata(&d).map(|m| m.len()).unwrap_or(u64::MAX) == size && same_content(&s, &d)? {
            continue;
        }
        if let Some(parent) = d.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::copy(&s, &d).map_err(|e| format!("copy {} -> {}: {e}", s.display(), d.display()))?;
        copied.push(d);
    }
    // Drop what is no longer in the set (never vendor/, .cargo/, Cargo.lock, the marker).
    if src.is_dir() {
        let mut stack = vec![src.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
                let p = entry.path();
                let rel = utf8(p.strip_prefix(src).map_err(|_| format!("{}: outside the stage", p.display()))?)?.replace('\\', "/");
                let top = rel.split('/').next().unwrap_or("");
                if ["vendor", ".cargo", "local", BOOTSTRAP, PROC_APPS].contains(&top) {
                    continue;
                }
                let ft = entry.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
                if ft.is_dir() {
                    stack.push(p);
                    continue;
                }
                if keep.contains(rel.as_str()) || rel == "Cargo.toml" || rel == "Cargo.lock" {
                    continue;
                }
                fs::remove_file(&p).map_err(|e| format!("remove {}: {e}", p.display()))?;
            }
        }
    }
    Ok((copied, total))
}

/// Whole-second mtimes, one deterministic newest file per package.
///
/// cargo fingerprints the run of a build script that prints no `rerun-if-*`
/// (ash, naga, pulldown-cmark, makepad-script) by the STRING
/// "<mtime>.<9 digits>s (<newest file>)" of its path package
/// (`PathSource::fingerprint` -> `LocalFingerprint::Precalculated`); the
/// string is hashed into that unit and, through `deps`, into every unit
/// above it. The phone restores the tar's ustar mtime — whole seconds — so
/// any nanosecond in that string is a Dirty build script on the phone (its
/// exec is EACCES: W^X) and a rebuilt engine. Rules:
///   - a file copied this run -> now (whole second): newer than the last
///     cross-build's dep-info, so cargo rebuilds what changed;
///   - any other file with a fractional mtime -> truncated (older, not
///     newer: still Fresh);
///   - every package's Cargo.toml -> newest other file of the package + 1 s
///     (cargo lists a package down to nested packages, never target/, and
///     keeps the FIRST of equal mtimes: a unique newest file makes the
///     string independent of a filesystem's listing order).
/// Returns (files set to now, files truncated, manifests bumped).
fn normalize_mtimes(root: &Path, fresh: &HashSet<PathBuf>) -> Result<(usize, usize, usize), String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) - 1;
    let (mut set_now, mut truncated) = (0usize, 0usize);
    let mut manifests = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let p = entry.path();
            let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            if p.file_name().map(|n| n == "Cargo.toml").unwrap_or(false) {
                manifests.push(p.clone());
            }
            let nanos = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            if fresh.contains(&p) {
                set_mtime(&p, now)?;
                set_now += 1;
            } else if nanos % 1_000_000_000 != 0 {
                set_mtime(&p, (nanos / 1_000_000_000) as u64)?;
                truncated += 1;
            }
        }
    }
    for m in &manifests {
        let pkg = m.parent().unwrap();
        let mut newest = 0u64;
        let mut stack = vec![pkg.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
                let p = entry.path();
                let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                if meta.file_type().is_symlink() || &p == m {
                    continue;
                }
                if meta.is_dir() {
                    let nested = p.join("Cargo.toml").is_file();
                    let target = dir == pkg && p.file_name().map(|n| n == "target").unwrap_or(false);
                    if !nested && !target {
                        stack.push(p);
                    }
                    continue;
                }
                let secs = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                newest = newest.max(secs);
            }
        }
        set_mtime(m, if newest > 0 { newest + 1 } else { now })?;
    }
    Ok((set_now, truncated, manifests.len()))
}

pub fn set_mtime(path: &Path, secs: u64) -> Result<(), String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .map_err(|e| format!("set mtime {}: {e}", path.display()))
}

/// `os.path.relpath(to, from)` for two absolute paths.
pub fn relative_path(from: &Path, to: &Path) -> String {
    let f: Vec<Component> = from.components().collect();
    let t: Vec<Component> = to.components().collect();
    let common = f.iter().zip(t.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..f.len() {
        parts.push("..".to_string());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// The `(package, version)` -> stage dir lookups the pack needs after
/// staging: `proc-macros.txt` and `host-strip.txt` as (package, crate)
/// pairs.
pub fn read_pairs(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
        })
        .collect())
}
