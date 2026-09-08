use crate::*;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Component, Path},
};

const MAX_FILES: usize = 512;
const MAX_ENTRIES: usize = 100_000;
const MAX_WALK_DEPTH: usize = 128;

/// Sorted Git blob hashes of eligible `.rs` files in a directory or file scope.
/// At most 512 files. On any error returns an empty manifest, never a truncated
/// one. Generators should use `try_manifest` to diagnose/narrow failed scopes.
pub fn manifest(scope: impl AsRef<Path>, repo_root: impl AsRef<Path>) -> Vec<FileHash> {
    try_manifest(scope, repo_root).unwrap_or_default()
}

/// Checked variant of `manifest`; overflow and IO failures are explicit.
pub fn try_manifest(
    scope: impl AsRef<Path>,
    repo_root: impl AsRef<Path>,
) -> Result<Vec<FileHash>, ArchError> {
    let root = root_path(repo_root.as_ref())?;
    discover(scope.as_ref(), &root)?
        .into_iter()
        .map(|path| {
            let hash = hash_file(&path, &root)?;
            Ok(FileHash { path, hash })
        })
        .collect()
}

/// Check every recorded source even if it is now outside the scope/policy.
/// New files are discovered only for directory scopes. All lists are sorted
/// and deduplicated. A node is affected by changed/removed referenced files;
/// an unrecorded ref matching an added file is affected as well.
pub fn changes_since(plan: &Plan, repo_root: impl AsRef<Path>) -> Changes {
    let mut changes = Changes::default();
    let root = match root_path(repo_root.as_ref()) {
        Ok(root) => root,
        Err(error) => {
            changes.errors.push(error);
            return changes;
        }
    };
    let mut recorded = BTreeSet::new();
    for file in &plan.source.files {
        recorded.insert(normalized(&file.path));
        if let Err(error) = crate::decode::check_hash(&file.hash, "source.files.hash") {
            changes.errors.push(error);
        }
        match hash_file(&file.path, &root) {
            Ok(hash) if hash != file.hash => changes.modified.push(normalized(&file.path)),
            Ok(_) => {}
            Err(error) if error.kind == ErrorKind::MissingPath => {
                changes.removed.push(normalized(&file.path))
            }
            Err(error) => changes.errors.push(error),
        }
    }
    match contained(Path::new(&plan.arch.scope), &root) {
        Ok(path) if path.is_dir() => match discover(Path::new(&plan.arch.scope), &root) {
            Ok(paths) => {
                for path in paths {
                    if !recorded.contains(&path) {
                        // Read new files too: unreadable discovery is not a
                        // complete freshness observation.
                        if let Err(error) = hash_file(&path, &root) {
                            changes.errors.push(error);
                        }
                        changes.added.push(path);
                    }
                }
            }
            Err(error) => changes.errors.push(error),
        },
        Ok(_) => {}
        Err(error) => changes.errors.push(error),
    }
    for paths in [
        &mut changes.added,
        &mut changes.removed,
        &mut changes.modified,
    ] {
        paths.sort();
        paths.dedup();
    }
    let changed: BTreeSet<_> = changes
        .added
        .iter()
        .chain(&changes.removed)
        .chain(&changes.modified)
        .collect();
    for (id, node) in &plan.nodes {
        let mut affected = false;
        for reference in &node.refs {
            match ref_path(reference) {
                Ok(path) => {
                    affected |= changed.contains(&normalized(Path::new(path)));
                }
                Err(error) => changes.errors.push(error),
            }
        }
        if affected {
            changes.affected_nodes.push(id.clone());
        }
    }
    changes.unchanged = changed.is_empty() && changes.errors.is_empty();
    changes
}

/// Errors: invalid structure/IDs/paths/hashes, missing witnesses/children,
/// escaping symlinks, non-files, and IO failures. Warnings: empty evidence or
/// prose, references absent from the source manifest, and older prompt tags.
/// Child paths are repository-relative, like refs and source paths. Empty
/// child strings mean no child. Symbols/line bounds and visuals are opaque v0.
pub fn validate(plan: &Plan, repo_root: impl AsRef<Path>) -> Validation {
    let mut result = Validation::default();
    // Public Plan fields may have been edited after parsing. Recheck the
    // schema and hard limits as well as the filesystem-dependent rules.
    if let Err(error) = parse(&format(plan), &Limits::default()) {
        result.errors.push(error);
    }
    let root = match root_path(repo_root.as_ref()) {
        Ok(root) => root,
        Err(error) => {
            result.errors.push(error);
            return result;
        }
    };
    if let Err(error) = contained(Path::new(&plan.arch.scope), &root) {
        result.errors.push(error);
    }
    if plan.arch.prompt != "arch/0" {
        warn(&mut result, "arch.prompt", "prompt differs from arch/0");
    }
    if plan.overview.trim().is_empty() {
        warn(&mut result, "source.overview", "overview is empty");
    }
    let mut recorded = BTreeSet::new();
    for file in &plan.source.files {
        recorded.insert(normalized(&file.path));
        check_file(&file.path, &root, "source.files", &mut result);
        if file.path.extension().and_then(|s| s.to_str()) != Some("rs") || excluded_file(&file.path)
        {
            result.errors.push(ArchError::new(
                ErrorKind::InvalidPath,
                "source.files",
                format!("ineligible source `{}`", file.path.display()),
            ));
        }
        if !normalized(&file.path).starts_with(normalized(Path::new(&plan.arch.scope))) {
            result.errors.push(ArchError::new(
                ErrorKind::InvalidPath,
                "source.files",
                "file is outside arch.scope",
            ));
        }
    }
    for (id, node) in &plan.nodes {
        let field = format!("nodes.{id}");
        if node.refs.is_empty() {
            warn(
                &mut result,
                &format!("{field}.refs"),
                "no source evidence; say unknown",
            );
        }
        for (key, prose) in [("summary", &node.summary), ("story", &node.story)] {
            if prose.trim().is_empty() {
                warn(&mut result, &format!("{field}.{key}"), "prose is empty");
            }
        }
        for reference in &node.refs {
            match ref_path(reference) {
                Ok(path) => {
                    check_file(
                        Path::new(path),
                        &root,
                        &format!("{field}.refs"),
                        &mut result,
                    );
                    if !recorded.contains(&normalized(Path::new(path))) {
                        warn(
                            &mut result,
                            &format!("{field}.refs"),
                            &format!("`{path}` is absent from source.files"),
                        );
                    }
                }
                Err(mut error) => {
                    error.field = format!("{field}.refs");
                    result.errors.push(error);
                }
            }
        }
        if let Some(child) = node.child.as_deref().filter(|s| !s.is_empty()) {
            check_file(
                Path::new(child),
                &root,
                &format!("{field}.child"),
                &mut result,
            );
        }
    }
    // Incomplete manifests are stale evidence, not malformed plans. Deleted
    // recorded paths are already errors above; newly discovered paths warn.
    if let Ok(scope) = contained(Path::new(&plan.arch.scope), &root) {
        if scope.is_dir() {
            match discover(Path::new(&plan.arch.scope), &root) {
                Ok(paths) => {
                    for path in paths {
                        if !recorded.contains(&path) {
                            warn(
                                &mut result,
                                "source.files",
                                &format!("unrecorded source `{}`", path.display()),
                            );
                        }
                    }
                }
                Err(error) => result.errors.push(error),
            }
        }
    }
    result
}

fn warn(result: &mut Validation, field: &str, message: &str) {
    result.warnings.push(Warning {
        field: field.into(),
        message: message.into(),
    });
}
fn check_file(path: &Path, root: &Path, field: &str, result: &mut Validation) {
    match contained(path, root) {
        Ok(full) if full.is_file() => {}
        Ok(_) => result.errors.push(ArchError::new(
            ErrorKind::InvalidPath,
            field,
            format!("not a file: {}", path.display()),
        )),
        Err(mut error) => {
            error.field = field.into();
            result.errors.push(error);
        }
    }
}
fn io_error(path: &Path, error: io::Error) -> ArchError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        ErrorKind::MissingPath
    } else {
        ErrorKind::Io
    };
    ArchError::new(kind, &path.to_string_lossy(), error.to_string())
}
fn root_path(root: &Path) -> Result<PathBuf, ArchError> {
    let path = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
    if !path.is_dir() {
        return Err(ArchError::new(
            ErrorKind::InvalidPath,
            "repo_root",
            "expected directory",
        ));
    }
    Ok(path)
}
pub(crate) fn normalized(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| *c != Component::CurDir)
        .collect()
}
fn lexical(path: &Path) -> Result<(), ArchError> {
    let text = path
        .to_str()
        .ok_or_else(|| ArchError::new(ErrorKind::InvalidPath, "path", "expected UTF-8 path"))?;
    if text.is_empty()
        || text.contains(['\\', ':', '\0'])
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(ArchError::new(
            ErrorKind::InvalidPath,
            text,
            "expected relative path without '..', absolute prefix, backslash, colon, or NUL",
        ));
    }
    if text.split('/').any(|part| part == "..") {
        return Err(ArchError::new(
            ErrorKind::InvalidPath,
            text,
            "parent paths are forbidden",
        ));
    }
    Ok(())
}
fn contained(path: &Path, root: &Path) -> Result<PathBuf, ArchError> {
    lexical(path)?;
    // Check every existing ancestor, including when the leaf has been deleted.
    // A missing leaf beneath an escaping symlink is not an ordinary deletion.
    let mut current = root.to_path_buf();
    for part in path.components() {
        current.push(part.as_os_str());
        let resolved = fs::canonicalize(&current).map_err(|e| io_error(path, e))?;
        if !resolved.starts_with(root) {
            return Err(ArchError::new(
                ErrorKind::InvalidPath,
                &path.to_string_lossy(),
                "symlink escapes repository root",
            ));
        }
    }
    Ok(current)
}

fn hash_file(path: &Path, root: &Path) -> Result<String, ArchError> {
    let full = contained(path, root)?;
    if !full.is_file() {
        return Err(ArchError::new(
            ErrorKind::InvalidPath,
            &path.to_string_lossy(),
            "expected regular file",
        ));
    }
    let bytes = fs::read(full).map_err(|e| io_error(path, e))?;
    let hash = makepad_code_graph::keys::content_hash(&bytes);
    let mut hex = String::with_capacity(40);
    use std::fmt::Write;
    for byte in hash {
        write!(hex, "{byte:02x}").unwrap();
    }
    Ok(hex)
}

pub(crate) fn ref_path(reference: &str) -> Result<&str, ArchError> {
    let (location, symbol) = reference
        .split_once("::")
        .map_or((reference, None), |(p, s)| (p, Some(s)));
    if symbol.is_some_and(|s| {
        s.is_empty()
            || s.split("::").any(|part| {
                part.is_empty()
                    || part
                        .chars()
                        .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | ':'))
            })
    }) {
        return Err(ArchError::new(
            ErrorKind::InvalidRef,
            reference,
            "empty or malformed symbol suffix",
        ));
    }
    let path = if let Some((path, line)) = location.rsplit_once(':') {
        if line.is_empty()
            || !line.bytes().all(|b| b.is_ascii_digit())
            || line.parse::<u32>().ok().filter(|n| *n > 0).is_none()
        {
            return Err(ArchError::new(
                ErrorKind::InvalidRef,
                reference,
                "expected positive 1-based line number",
            ));
        }
        path
    } else {
        location
    };
    lexical(Path::new(path))?;
    Ok(path)
}

// Mirrors code_graph::source::CorpusPolicy::default, with arch directories
// and all non-Rust files excluded. Only the content hash API is imported.
fn excluded_dir(path: &Path) -> bool {
    let rel = path.to_string_lossy();
    [
        "old",
        "local",
        "libs/windows",
        "libs/apple_sys",
        "libs/jni-sys",
        "libs/objc-sys",
        "libs/vulkan",
    ]
    .iter()
    .any(|prefix| rel == *prefix || rel.starts_with(&format!("{prefix}/")))
        || rel.split('/').any(|part| {
            part == "arch" || part.starts_with("target") || (part != "." && part.starts_with('.'))
        })
}
fn excluded_file(path: &Path) -> bool {
    path.parent().is_some_and(excluded_dir)
}
fn discover(scope: &Path, root: &Path) -> Result<Vec<PathBuf>, ArchError> {
    lexical(scope)?;
    let full = contained(scope, root)?;
    let mut relative = PathBuf::new();
    // Explicit scopes must obey the same ancestors and boundaries as a
    // repository-root walk, including nested repositories and symlinks.
    for part in scope
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
    {
        relative.push(part);
        let path = root.join(&relative);
        let meta = fs::symlink_metadata(&path).map_err(|e| io_error(&relative, e))?;
        if meta.file_type().is_symlink()
            || (meta.is_dir() && (excluded_dir(&relative) || path.join(".git").exists()))
        {
            return Ok(Vec::new());
        }
    }
    let mut out = BTreeSet::new();
    let mut entries = 0;
    if full.is_dir() {
        walk(&relative, root, 0, &mut entries, &mut out)?;
    } else if full.is_file()
        && relative.extension().and_then(|s| s.to_str()) == Some("rs")
        && !excluded_file(&relative)
    {
        out.insert(relative);
    }
    Ok(out.into_iter().collect())
}
fn walk(
    relative: &Path,
    root: &Path,
    depth: usize,
    entries: &mut usize,
    out: &mut BTreeSet<PathBuf>,
) -> Result<(), ArchError> {
    limit("discovery depth", depth, MAX_WALK_DEPTH)?;
    for entry in fs::read_dir(root.join(relative)).map_err(|e| io_error(relative, e))? {
        *entries += 1;
        limit("discovery entries", *entries, MAX_ENTRIES)?;
        let entry = entry.map_err(|e| io_error(relative, e))?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| io_error(&path, e))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if excluded_dir(&path) || entry.path().join(".git").exists() {
                continue;
            }
            walk(&path, root, depth + 1, entries, out)?;
        } else if kind.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
            lexical(&path)?;
            out.insert(path);
            limit("source.files", out.len(), MAX_FILES)?;
        }
    }
    Ok(())
}
