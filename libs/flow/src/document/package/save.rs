//! Worker-only package persistence. The directory is also a nonblocking
//! writer reservation and an interruption marker for package readers.
//!
//! `journal.json` lists bounded relative paths and old/new hashes. `N.old`
//! retains every replaced file, including the manifest, until verification
//! completes. A crash never silently turns partial files into an accepted
//! package: opening refuses and names the retained recovery directory.

use super::*;
use makepad_script::{
    tokenizer::ScriptTokenizer,
    ScriptVmBase,
};
use makepad_strict_json::{obj, s, Value};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
};

const TRANSACTION: &str = ".stage-package-save";

pub(super) fn check_open(root: &Path) -> Result<(), PackageError> {
    let path = root.join(TRANSACTION);
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(field_error("save", &format!(
            "a package save is active or was interrupted; inspect `{}` and its journal/backups before reopening; no recovery files were discarded",
            path.display(),
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(&path, error)),
    }
}

pub(super) fn same_modules(left: &[ModuleSource], right: &[ModuleSource]) -> bool {
    left.len() == right.len()
        && left.iter().all(|module| {
            left.iter()
                .filter(|other| other.alias == module.alias)
                .count()
                == 1
                && right
                    .iter()
                    .filter(|other| other.alias == module.alias)
                    .count()
                    == 1
                && right.iter().any(|other| other == module)
        })
}

struct Change {
    relative: String,
    path: PathBuf,
    old: String,
    new: String,
    permissions: fs::Permissions,
}

pub(super) fn definition(
    package: &Package,
    source: &str,
    modules: &[ModuleSource],
) -> Result<Package, PackageError> {
    if source.len() > graph::MAX_SOURCE_BYTES {
        return Err(field_error(
            "entry",
            "source exceeds the package source limit",
        ));
    }
    if modules.len() > MAX_MODULES
        || modules
            .iter()
            .try_fold(0usize, |bytes, module| {
                (module.source.len() <= graph::MAX_SOURCE_BYTES).then_some(())?;
                bytes
                    .checked_add(module.source.len())
                    .filter(|bytes| *bytes <= MAX_CLOSURE_BYTES)
            })
            .is_none()
    {
        return Err(field_error(
            "modules",
            "accepted closure exceeds the package count or source limits",
        ));
    }
    let closure = Closure::ordered(modules.to_vec()).map_err(PackageError::Manifest)?;
    if closure.modules.len() != package.modules.len()
        || package.modules.iter().any(|old| {
            !closure
                .modules
                .iter()
                .any(|new| new.alias == old.alias && new.path == old.path)
        })
    {
        return Err(field_error(
            "modules",
            "saving a definition cannot add, remove or rename pinned modules",
        ));
    }
    let root = package
        .root
        .canonicalize()
        .map_err(|error| io(&package.root, error))?;
    let transaction = root.join(TRANSACTION);
    fs::create_dir(&transaction).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            field_error(
                "save",
                &format!(
                    "another or interrupted save owns `{}`; its recovery files were preserved",
                    transaction.display()
                ),
            )
        } else {
            io(&transaction, error)
        }
    })?;
    // No destination is touched until every baseline, source, path, pin and
    // backup below is ready. Preparation failures can remove our reservation.
    let prepared = prepare(package, source, &closure.modules, &root, &transaction);
    let changes = match prepared {
        Ok(changes) => changes,
        Err(error) => return Err(clean_failed_preparation(&transaction, error)),
    };
    let mut applied = Vec::new();
    let install = (|| {
        for (index, change) in changes.iter().enumerate() {
            // Repeat confinement and baseline checks immediately before the
            // rename, refusing detected edits made by another writer.
            let path = target(&root, &change.relative)?;
            if path != change.path || read_source(&path)? != change.old {
                return Err(field_error(
                    "save",
                    &format!(
                        "`{}` changed while saving; reload before retrying",
                        change.relative
                    ),
                ));
            }
            let staged = transaction.join(format!("{index}.new"));
            fs::rename(&staged, &path).map_err(|error| io(&path, error))?;
            applied.push(index);
            sync_directory(path.parent().unwrap())?;
        }
        let saved = Package::open_files(&package.root)?;
        if !saved.matches_definition(source, &closure.modules) {
            return Err(field_error(
                "save",
                "the written package does not match the accepted definition",
            ));
        }
        Ok(saved)
    })();
    match install {
        Ok(saved) => {
            // A cleanup failure is explicit; the complete committed files
            // remain, and open will refuse while the marker still exists.
            fs::remove_dir_all(&transaction).map_err(|error| io(&transaction, error))?;
            sync_directory(&root)?;
            Ok(saved)
        }
        Err(error) => {
            let rollback = rollback(&root, &transaction, &changes, &applied);
            match rollback {
                Ok(()) => Err(clean_failed_preparation(&transaction, error)),
                Err(rollback) => Err(field_error("save", &format!(
                    "{error}; rollback could not complete: {rollback}; original files and journal remain in `{}`; reopening is refused",
                    transaction.display(),
                ))),
            }
        }
    }
}

fn prepare(
    package: &Package,
    source: &str,
    modules: &[ModuleSource],
    root: &Path,
    transaction: &Path,
) -> Result<Vec<Change>, PackageError> {
    // Fail closed on file aliases/symlinks, even when the alias currently
    // resolves inside this package. A save replaces named regular files.
    let mut paths = HashSet::new();
    let mut resolved_paths = HashSet::new();
    #[cfg(unix)]
    let mut file_ids = HashSet::new();
    for path in std::iter::once(MANIFEST_FILE)
        .chain(std::iter::once(package.manifest.entry.0.as_str()))
        .chain(package.modules.iter().map(|module| module.path.as_str()))
    {
        if !paths.insert(path.to_string()) {
            return Err(field_error(
                "save",
                "manifest, entry and component paths must be distinct",
            ));
        }
        let resolved = target(root, path)?;
        if !resolved_paths.insert(resolved.clone()) {
            return Err(field_error(
                "save",
                "writable package paths alias the same file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&resolved).map_err(|error| io(&resolved, error))?;
            if !file_ids.insert((metadata.dev(), metadata.ino())) {
                return Err(field_error(
                    "save",
                    "writable package paths share a file identity",
                ));
            }
        }
    }
    if package
        .manifest
        .surface
        .as_ref()
        .is_some_and(|surface| paths.contains(&surface.0))
    {
        return Err(field_error(
            "save",
            "the surface must not share a writable flow or manifest path",
        ));
    }
    let disk = Package::open_files(&package.root)?;
    if disk.manifest_source != package.manifest_source
        || !disk.matches_definition(&package.entry_source, &package.modules)
    {
        return Err(field_error(
            "save",
            "the package was edited outside this session; reload it before saving",
        ));
    }
    let manifest_source = patch_pins(package, modules)?;
    let mut changes = Vec::new();
    let mut add = |relative: &str, old: &str, new: &str| -> Result<(), PackageError> {
        if old != new {
            let path = target(root, relative)?;
            let permissions = fs::metadata(&path)
                .map_err(|error| io(&path, error))?
                .permissions();
            changes.push(Change {
                relative: relative.to_string(),
                path,
                old: old.to_string(),
                new: new.to_string(),
                permissions,
            });
        }
        Ok(())
    };
    add(&package.manifest.entry.0, &package.entry_source, source)?;
    for old in &package.modules {
        let new = modules
            .iter()
            .find(|new| new.alias == old.alias)
            .expect("membership checked");
        add(&old.path, &old.source, &new.source)?;
    }
    // Publish the pin set last. Package::open refuses while our reservation
    // exists, so cooperating readers never accept a mixed generation.
    add(MANIFEST_FILE, &package.manifest_source, &manifest_source)?;
    for (index, change) in changes.iter().enumerate() {
        write_new(&transaction.join(format!("{index}.old")), &change.old)?;
        let staged = transaction.join(format!("{index}.new"));
        write_new(&staged, &change.new)?;
        fs::set_permissions(&staged, change.permissions.clone())
            .map_err(|error| io(&staged, error))?;
    }
    let entries = changes
        .iter()
        .enumerate()
        .map(|(index, change)| {
            obj(vec![
                ("path", s(&change.relative)),
                ("old", s(format!("{index}.old"))),
                ("new", s(format!("{index}.new"))),
                ("old_hash", s(source_hash(&change.old))),
                ("new_hash", s(source_hash(&change.new))),
            ])
        })
        .collect();
    let journal = obj(vec![
        ("version", Value::Int(1)),
        ("files", Value::Arr(entries)),
    ])
    .to_json();
    write_new(&transaction.join("journal.json"), &journal)?;
    sync_directory(transaction)?;
    sync_directory(root)?;
    Ok(changes)
}

fn rollback(
    root: &Path,
    transaction: &Path,
    changes: &[Change],
    applied: &[usize],
) -> Result<(), PackageError> {
    for &index in applied.iter().rev() {
        let change = &changes[index];
        let path = target(root, &change.relative)?;
        let current = read_source(&path)?;
        if current == change.old {
            continue;
        }
        if current != change.new {
            return Err(field_error(
                "save",
                &format!(
                    "`{}` changed outside the transaction; recovery will not overwrite it",
                    change.relative
                ),
            ));
        }
        let staged = transaction.join(format!("{index}.rollback"));
        write_new(&staged, &change.old)?;
        fs::set_permissions(&staged, change.permissions.clone())
            .map_err(|error| io(&staged, error))?;
        fs::rename(&staged, &path).map_err(|error| io(&path, error))?;
        sync_directory(path.parent().unwrap())?;
    }
    Ok(())
}

fn clean_failed_preparation(transaction: &Path, error: PackageError) -> PackageError {
    match fs::remove_dir_all(transaction) {
        Ok(()) => error,
        Err(cleanup) => field_error(
            "save",
            &format!(
                "{error}; recovery directory `{}` could not be removed: {cleanup}",
                transaction.display()
            ),
        ),
    }
}

fn target(root: &Path, relative: &str) -> Result<PathBuf, PackageError> {
    let path = module_path("save.path", &Literal::Str(relative.to_string()))?;
    if relative.contains(':')
        || relative.contains('\0')
        || Path::new(relative)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(field_error(
            "save.path",
            "must be a portable relative file path",
        ));
    }
    if path.0.split('/').next() == Some(TRANSACTION) {
        return Err(field_error(
            "save.path",
            "the transaction directory is reserved",
        ));
    }
    let mut at = root.to_path_buf();
    let components: Vec<_> = path.0.split('/').collect();
    for (index, component) in components.iter().enumerate() {
        at.push(component);
        let metadata = fs::symlink_metadata(&at).map_err(|error| io(&at, error))?;
        if metadata.file_type().is_symlink()
            || (index + 1 == components.len() && !metadata.is_file())
            || (index + 1 != components.len() && !metadata.is_dir())
        {
            return Err(field_error(
                "save.path",
                &format!("`{relative}` must name a regular file without symlink components"),
            ));
        }
    }
    let canonical = at.canonicalize().map_err(|error| io(&at, error))?;
    if !canonical.starts_with(root) {
        return Err(field_error("save.path", "resolves outside the package"));
    }
    Ok(canonical)
}

fn write_new(path: &Path, text: &str) -> Result<(), PackageError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io(path, error))?;
    file.write_all(text.as_bytes())
        .map_err(|error| io(path, error))?;
    file.sync_all().map_err(|error| io(path, error))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), PackageError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), PackageError> {
    Ok(())
}

/// Patch only literal hash contents in matching `{path, as, hash}` records.
/// Splash's tokenizer owns comments, escapes and strings; computed or
/// ambiguous pin declarations are refused rather than flattened/reformatted.
fn patch_pins(package: &Package, modules: &[ModuleSource]) -> Result<String, PackageError> {
    let changed: Vec<_> = package
        .modules
        .iter()
        .filter_map(|old| {
            modules
                .iter()
                .find(|new| new.alias == old.alias)
                .filter(|new| new.hash != old.hash)
                .map(|new| (old, new))
        })
        .collect();
    if changed.is_empty() {
        return Ok(package.manifest_source.clone());
    }
    let source = &package.manifest_source;
    let mut base = ScriptVmBase::new();
    let mut tokenizer = ScriptTokenizer::default();
    let tokens = tokenizer.tokenize(source, base.heap_mut()).to_vec();
    let mut candidates = Vec::new();
    for (start, token) in tokens.iter().enumerate() {
        if !token.token().is_open_curly() {
            continue;
        }
        let mut index = start + 1;
        let mut fields = Vec::new();
        while let Some(token) = tokens.get(index) {
            if token.token().is_close_curly() {
                break;
            }
            let Some(name) = token.token().as_identifier() else {
                break;
            };
            let Some(name) = name.as_string(|name| name.map(str::to_string)) else {
                break;
            };
            if !matches!(name.as_str(), "path" | "as" | "hash")
                || fields.iter().any(|(old, _, _)| old == &name)
            {
                break;
            }
            let Some(colon) = tokens.get(index + 1) else {
                break;
            };
            if !colon.token().operator_or_separator().is_some_and(|id| id.as_string(|text| text == Some(":")))
            {
                break;
            }
            let Some(value) = tokens.get(index + 2) else {
                break;
            };
            // a finished string only (as_string() also answers "" for StringUnfinished, which the
            // `let ScriptToken::String(value) = .. else` this replaces did not)
            let Some(value) = value.token().as_string().filter(|_| value.token().kind() == makepad_script::tokenizer::ScriptTokenTag::String) else {
                break;
            };
            let Some(value) = base.heap_ref().string_with(value, |_, value| value.to_string()) else {
                break;
            };
            fields.push((name, value, index + 2));
            index += 3;
            if tokens.get(index).is_some_and(|token| token.token().kind() == makepad_script::tokenizer::ScriptTokenTag::Separator && token.token().separator().as_string(|text| text == Some(","))) { index += 1; }
        }
        if fields.len() == 3
            && tokens
                .get(index)
                .is_some_and(|token| token.token().is_close_curly())
        {
            candidates.push(fields);
        }
    }
    let mut patches = Vec::new();
    for (old, new) in changed {
        let matching: Vec<_> = candidates
            .iter()
            .filter(|fields| {
                fields
                    .iter()
                    .any(|(name, value, _)| name == "path" && value == &old.path)
                    && fields
                        .iter()
                        .any(|(name, value, _)| name == "as" && value == &old.alias)
                    && fields
                        .iter()
                        .any(|(name, value, _)| name == "hash" && value == &old.hash)
            })
            .collect();
        if matching.len() != 1 {
            return Err(field_error("modules.hash", &format!("`{}` needs one literal {{path, as, hash}} declaration to update its pin without rewriting the manifest", old.alias)));
        }
        let token = matching[0]
            .iter()
            .find(|(name, _, _)| name == "hash")
            .unwrap()
            .2;
        let (row, col) = tokenizer
            .token_index_to_row_col(token as u32)
            .ok_or_else(|| field_error("modules.hash", "cannot locate its literal"))?;
        let line_start = source
            .split_inclusive('\n')
            .take(row as usize)
            .map(str::len)
            .sum::<usize>();
        let offset = line_start
            + source[line_start..]
                .char_indices()
                .nth(col as usize)
                .map_or(0, |(index, _)| index);
        let end = offset + old.hash.len();
        let quote = source.as_bytes().get(offset.wrapping_sub(1)).copied();
        if source.get(offset..end) != Some(old.hash.as_str())
            || !matches!(quote, Some(b'"' | b'\''))
            || source.as_bytes().get(end).copied() != quote
        {
            return Err(field_error(
                "modules.hash",
                "pin hashes must be plain quoted hexadecimal literals",
            ));
        }
        patches.push((offset, end, new.hash.clone()));
    }
    patches.sort_by_key(|patch| patch.0);
    if patches.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(field_error("modules.hash", "pin source ranges overlap"));
    }
    let mut written = source.clone();
    for (start, end, hash) in patches.into_iter().rev() {
        written.replace_range(start..end, &hash);
    }
    let mut expected = package.manifest.clone();
    for pin in &mut expected.modules {
        let updated = modules
            .iter()
            .find(|module| pin.alias.as_deref() == Some(module.alias.as_str()))
            .ok_or_else(|| {
                field_error(
                    "modules",
                    "a manifest pin is absent from the accepted closure",
                )
            })?;
        pin.hash = Some(updated.hash.clone());
    }
    let found = AppManifest::from_fields(
        graph::evaluate_manifest(&written, MANIFEST_FILE).map_err(PackageError::Manifest)?,
    )?;
    if found != expected {
        return Err(field_error(
            "modules.hash",
            "editing the pin changes other evaluated manifest fields",
        ));
    }
    Ok(written)
}
