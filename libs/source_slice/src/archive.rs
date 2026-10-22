//! A source archive: the slice of one repository with a root workspace
//! (Makepad's) for `roots`, as one ZIP that builds offline
//! (`cargo build --release --offline -p <root>`): the loader download of the
//! entry scripts (tools/run/entry), `makepad run`'s crates.
//!
//! The slice is cut as usual (`Source::slice`: the roots' path-dependency
//! closure, `{ workspace = true }` and patched registry dependencies
//! resolved through the root manifest). The ZIP holds, under `top/`, the
//! slice's files without its generated members package, and:
//! - a generated workspace `Cargo.toml`: `members` = the slice's crates that
//!   are members of the source workspace (cargo's rule: listed, or a path
//!   dependency of a member not under a `workspace.exclude` entry), `exclude` = the others
//!   (crates the source keeps out of its workspace stay out of this one), the source's
//!   `resolver`, its `[workspace.dependencies]` and `[patch.crates-io]`
//!   entries whose path the slice ships (or that have none), its other
//!   `[workspace.*]` tables, and its `[profile.*]` tables (a
//!   `[profile.*.package.<name>]` only for a package of the slice);
//! - for each member's dev-dependency outside the slice a placeholder package
//!   (cargo reads every member's dev-dependency manifest); the members'
//!   manifests stay byte for byte as written, since makepad run's source
//!   identity of a provided crate hashes its Cargo.toml and the loader built
//!   from the archive must match the same crates read from the full tree;
//! - the source's `Cargo.lock` (cargo drops the entries the smaller
//!   workspace does not use, offline);
//! - `SOURCE_COMMITS`: the source commit.
//!
//! The archive needs nothing but itself and a Rust toolchain when the source
//! workspace resolves nothing from a registry (Makepad's does not). The ZIP
//! has fixed timestamps and sorted members, so the same source commit and
//! roots give the same bytes.
use crate::{git, read_blobs, resolve, Slice, Source};
use makepad_zip_file::writer::{ZipMethod, ZipWriter};
use std::{collections::BTreeSet, path::Path};

/// A source archive and the slice it was made of.
pub struct Archive {
    pub zip: Vec<u8>,
    pub slice: Slice,
    /// Files in the ZIP.
    pub files: usize,
}

/// Cut the slice of `roots` at `commit` (written into `repo`) and pack it.
pub fn workspace_archive(repo: &Path, commit: &str, roots: &[String], app: &str, top: &str) -> Result<Archive, String> {
    if top.is_empty() || resolve("", top).as_deref() != Some(top) {
        return Err(format!("Invalid archive folder {top:?}"));
    }
    let source = Source::load(repo, commit)?;
    let workspace = source.workspace.as_ref().ok_or("The repository root has no [workspace]: use a slice")?;
    let slice = source.slice(repo, app, roots, &[])?;
    let names: BTreeSet<&str> = slice.crates.iter().chain(&slice.placeholders).map(|(n, _)| n.as_str()).collect();
    let dirs: BTreeSet<&str> = slice.crates.iter().chain(&slice.placeholders).map(|(_, d)| d.as_str()).collect();
    // cargo's membership: the listed members, and the path dependencies of members (of every
    // kind) inside the workspace that no workspace.exclude entry covers, transitively
    let excluded = |d: &str| workspace.exclude.iter().any(|e| crate::under(d, e.trim_end_matches('/')));
    let mut members: BTreeSet<&str> = BTreeSet::new();
    let mut todo: Vec<&str> = slice
        .crates
        .iter()
        .map(|(_, d)| d.as_str())
        .filter(|d| workspace.members.iter().any(|m| member_matches(m, d)))
        .collect();
    while let Some(dir) = todo.pop() {
        if !members.insert(dir) {
            continue;
        }
        let krate = &source.crates[dir];
        for dep in krate.deps.iter().chain(&krate.dev) {
            if let Some(dep) = dirs.get(dep.as_str()) {
                if !excluded(dep) && !members.contains(dep) {
                    todo.push(dep);
                }
            }
        }
    }
    let manifest = workspace_manifest(workspace, &dirs, &members, &names, app, roots, &source.commit);

    let listing = git(repo, &["ls-tree", "-r", "-z", "--full-tree", &slice.commit], None, &[])?;
    let mut paths = Vec::new();
    let mut oids = Vec::new();
    for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = std::str::from_utf8(record).map_err(|_| "A path in the slice is not UTF-8".to_string())?;
        let (meta, path) = record.split_once('\t').ok_or("Invalid git ls-tree record")?;
        let oid = meta.split_whitespace().nth(2).ok_or("Invalid git ls-tree record")?;
        // the slice's generated members package: the archive has its own root
        if path == "Cargo.toml" || path == "src/lib.rs" {
            continue;
        }
        paths.push(path.to_owned());
        oids.push(oid.to_owned());
    }
    if let Some(lock) = source.by_path.get("Cargo.lock").map(|i| &source.entries[*i]).filter(|e| e.kind == "blob") {
        paths.push("Cargo.lock".into());
        oids.push(lock.oid.clone());
    }
    let blobs = read_blobs(repo, &oids.iter().map(String::as_str).collect::<Vec<_>>())?;
    let mut files: Vec<(String, Vec<u8>)> = paths.into_iter().zip(blobs).collect();
    // The members' dev-dependencies outside the slice, as placeholder packages: cargo reads the
    // manifest of every member's dev-dependency, and the members' own manifests stay as written
    // (makepad run's source identity of a provided crate hashes its Cargo.toml).
    let mut dev = BTreeSet::new();
    for dir in &members {
        for dep in &source.crates[*dir].dev {
            if !dirs.contains(dep.as_str()) {
                dev.insert(dep.clone());
            }
        }
    }
    for dir in &dev {
        let krate = source.crates.get(dir).ok_or_else(|| format!("{dir}: a member's dev-dependency without a readable package Cargo.toml"))?;
        files.push((format!("{dir}/Cargo.toml"), crate::placeholder_manifest(krate).into_bytes()));
        files.push((format!("{dir}/src/lib.rs"), Vec::new()));
    }
    files.push(("Cargo.toml".into(), manifest.into_bytes()));
    files.push(("SOURCE_COMMITS".into(), format!("{}\n", source.commit).into_bytes()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut zip = ZipWriter::new();
    for (path, data) in &files {
        zip.add(&format!("{top}/{path}"), data, ZipMethod::Deflate).map_err(|e| e.to_string())?;
    }
    let zip = zip.finish().map_err(|e| e.to_string())?;
    Ok(Archive { zip, slice, files: files.len() })
}

/// Whether a `workspace.members` entry names `dir` (a `*` matches one path
/// segment).
fn member_matches(pattern: &str, dir: &str) -> bool {
    let (p, d): (Vec<&str>, Vec<&str>) = (pattern.split('/').collect(), dir.split('/').collect());
    p.len() == d.len() && p.iter().zip(&d).all(|(p, d)| *p == "*" || p == d)
}

/// The `path = "..."` of a one-line dependency entry, when it has one.
fn entry_path(line: &str) -> Option<&str> {
    let at = line.find("path")?;
    let rest = line[at + 4..].trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
    rest.split('"').next()
}

fn table_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let inner = trimmed.trim_start_matches('[');
    let end = inner.rfind(']')?;
    Some(inner[..end].trim_end_matches(']').trim())
}

/// The archive's workspace manifest (see the module documentation).
fn workspace_manifest(
    workspace: &crate::Workspace,
    dirs: &BTreeSet<&str>,
    members: &BTreeSet<&str>,
    names: &BTreeSet<&str>,
    app: &str,
    roots: &[String],
    commit: &str,
) -> String {
    let list = |items: &mut dyn Iterator<Item = &str>| items.map(|d| format!("    \"{d}\",\n")).collect::<String>();
    let excluded: Vec<&str> = dirs.iter().copied().filter(|d| !members.contains(d)).collect();
    let mut out = format!(
        "# Generated by makepad-source-slice: the source archive of {app}, roots {}, from {commit}.\n\
         [workspace]\nresolver = \"{}\"\nmembers = [\n{}]\nexclude = [\n{}]\n",
        roots.join(", "),
        workspace.resolver.as_deref().unwrap_or("2"),
        list(&mut members.iter().copied()),
        list(&mut excluded.into_iter()),
    );
    // Copy the root manifest's tables after the generated [workspace]: the
    // dependency tables filtered to what the slice ships, profiles for its
    // packages, other workspace tables as written; the top-level keys before
    // the first table (Makepad's dotted workspace.members) are replaced above.
    #[derive(PartialEq)]
    enum Keep {
        All,
        Entries,
        Skip,
    }
    let mut keep = Keep::Skip;
    for line in workspace.manifest.lines() {
        if let Some(name) = table_name(line) {
            keep = if name == "workspace" {
                Keep::Skip
            } else if name == "workspace.dependencies" || name.starts_with("patch.") {
                Keep::Entries
            } else if let Some((_, package)) = name.strip_prefix("profile.").and_then(|p| p.split_once(".package.")) {
                let package = package.trim_matches('"');
                if package == "*" || names.contains(package) { Keep::All } else { Keep::Skip }
            } else if name.starts_with("profile.") || name.starts_with("workspace.") {
                Keep::All
            } else {
                Keep::Skip
            };
            if keep != Keep::Skip {
                out.push('\n');
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        match keep {
            Keep::Skip => {}
            Keep::All => {
                out.push_str(line);
                out.push('\n');
            }
            Keep::Entries => {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if entry_path(line).and_then(|p| resolve("", p)).is_some_and(|dir| !dirs.contains(dir.as_str())) {
                    continue;
                }
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}
