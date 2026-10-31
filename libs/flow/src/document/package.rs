//! Workflow packages.
//!
//! A package is a directory: `app.splash` (the manifest), the entry flow and
//! the modules it pins. The manifest is ordinary Splash:
//!
//! ```splash
//! use mod.app.*
//!
//! App{
//!     id: "stage.hello"
//!     name: "Hello"
//!     entry: "flow.splash"
//!     roles: [{role: "program", kind: "audio.out", required: false}]
//!     defaults: {name: "Stage"}
//! }
//! ```
//!
//! It declares capability **roles**; device, endpoint and credential
//! bindings stay with the local profile and never enter a package. A field
//! this reader does not know is an error, never ignored.

use super::closure::{Closure, ModuleSource, MAX_CLOSURE_BYTES, MAX_MODULES};
use super::ident::{DefId, DefRevision, ModulePath, PackageId};
use crate::digest::{to_hex, Sha256};
use crate::{graph, EvalError, Literal};
use std::path::{Path, PathBuf};

mod save;

pub const MANIFEST_FILE: &str = "app.splash";

#[derive(Clone, Debug, PartialEq)]
pub struct PinnedModule {
    pub path: ModulePath,
    /// The alias the module is installed under (`mod.components.<alias>`).
    pub alias: Option<String>,
    /// Lower-case hex sha256 of the module source, when the package pins it.
    pub hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleDeclaration {
    /// The package-local role name a flow refers to (`program`, `cue`).
    pub role: String,
    /// The capability kind the role needs (`audio.out`, `camera`, `browser`).
    pub kind: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppManifest {
    pub id: PackageId,
    pub name: String,
    /// The entry flow module, relative to the package directory.
    pub entry: ModulePath,
    /// The bound Splash surface module, when the package has one.
    pub surface: Option<ModulePath>,
    /// Component modules the entry flow pins.
    pub modules: Vec<PinnedModule>,
    pub roles: Vec<RoleDeclaration>,
    /// Input node id -> design default the package ships.
    pub defaults: Vec<(String, Literal)>,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub root: PathBuf,
    pub manifest: AppManifest,
    /// Original manifest text, retained so saving pins preserves authored
    /// formatting/comments and refuses an independently edited baseline.
    pub manifest_source: String,
    pub entry_source: String,
    /// The entry flow's file name, for diagnostics and run inputs.
    pub entry_file: String,
    /// The entry flow as a pinned definition.
    pub entry_def: DefId,
    /// The bound surface module's source and file name, when the manifest
    /// names one. The windowless host ignores it; the window mounts it.
    pub surface_source: Option<String>,
    pub surface_file: Option<String>,
    /// Every pinned module's bytes, read at open and hash-verified; the
    /// closure a host installs with the entry flow (see [`Self::closure`]).
    pub modules: Vec<ModuleSource>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PackageError {
    /// Anything but a directory package (an archive, a single file, a URL).
    Unsupported(String),
    Io {
        path: String,
        error: String,
    },
    Manifest(EvalError),
    Field {
        field: String,
        message: String,
    },
    UnknownField(String),
    ModuleHash {
        module: String,
        expected: String,
        found: String,
    },
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(formatter, "unsupported package: {what}"),
            Self::Io { path, error } => write!(formatter, "{path}: {error}"),
            Self::Manifest(error) => write!(formatter, "{error}"),
            Self::Field { field, message } => write!(formatter, "manifest field `{field}`: {message}"),
            Self::UnknownField(field) => write!(formatter, "manifest field `{field}` is not known to this reader"),
            Self::ModuleHash { module, expected, found } => write!(
                formatter,
                "module `{module}` does not match its pinned hash (expected {expected}, found {found})"
            ),
        }
    }
}

impl std::error::Error for PackageError {}

impl Package {
    /// Open a directory package. The manifest is evaluated, every path is
    /// confined to the directory and every pinned hash is verified.
    pub fn open(root: &Path) -> Result<Self, PackageError> {
        save::check_open(root)?;
        let package = Self::open_files(root)?;
        save::check_open(root)?;
        if read_source(&root.join(MANIFEST_FILE))? != package.manifest_source {
            return Err(field_error(
                "save",
                "the manifest changed while opening the package; retry after its save completes",
            ));
        }
        Ok(package)
    }

    fn open_files(root: &Path) -> Result<Self, PackageError> {
        let metadata = std::fs::metadata(root).map_err(|error| io(root, error))?;
        if !metadata.is_dir() {
            return Err(PackageError::Unsupported(format!(
                "`{}` is not a directory; only directory packages are supported",
                root.display()
            )));
        }
        let manifest_path = root.join(MANIFEST_FILE);
        let manifest_source = read_source(&manifest_path)?;
        let fields =
            graph::evaluate_manifest(&manifest_source, &manifest_path.display().to_string())
                .map_err(PackageError::Manifest)?;
        let manifest = AppManifest::from_fields(fields)?;

        let entry_path = confined(root, &manifest.entry, "entry")?;
        let entry_source = read_source(&entry_path)?;
        let mut surface_source = None;
        let mut surface_file = None;
        if let Some(surface) = &manifest.surface {
            let path = confined(root, surface, "surface")?;
            surface_source = Some(read_source(&path)?);
            surface_file = Some(path.display().to_string());
        }
        // Admission as the modules are read: the count and the running
        // byte total are checked before each read, so a manifest cannot
        // make this reader hold more than the closure limits.
        if manifest.modules.len() > MAX_MODULES {
            return Err(PackageError::Field {
                field: "modules".to_string(),
                message: format!(
                    "{} modules are pinned, over the {MAX_MODULES} limit",
                    manifest.modules.len()
                ),
            });
        }
        let mut modules = Vec::with_capacity(manifest.modules.len());
        let mut total = 0usize;
        for module in &manifest.modules {
            let path = confined(root, &module.path, "modules")?;
            let size = std::fs::metadata(&path)
                .map_err(|error| io(&path, error))?
                .len() as usize;
            if size > graph::MAX_SOURCE_BYTES || total + size > MAX_CLOSURE_BYTES {
                return Err(PackageError::Field {
                    field: "modules".to_string(),
                    message: format!(
                        "module `{}` would take the pinned modules past their size limit",
                        module.path.0
                    ),
                });
            }
            total += size;
            let source = read_source(&path)?;
            let found = source_hash(&source);
            match &module.hash {
                Some(expected) if expected != &found => {
                    return Err(PackageError::ModuleHash {
                        module: module.path.0.clone(),
                        expected: expected.clone(),
                        found,
                    });
                }
                Some(_) => {}
                None => {
                    return Err(PackageError::Field {
                        field: "modules.hash".to_string(),
                        message: format!("module `{}` has no pinned hash", module.path.0),
                    })
                }
            }
            let Some(alias) = module.alias.clone() else {
                return Err(PackageError::Field {
                    field: "modules.as".to_string(),
                    message: format!("module `{}` has no `as` alias", module.path.0),
                });
            };
            modules.push(ModuleSource {
                path: module.path.0.clone(),
                alias,
                hash: found,
                source,
            });
        }
        let entry_def = DefId {
            package: manifest.id.clone(),
            module: manifest.entry.clone(),
            revision: DefRevision(source_hash(&entry_source)),
        };
        Ok(Self {
            root: root.to_path_buf(),
            entry_file: entry_path.display().to_string(),
            entry_source,
            entry_def,
            manifest,
            manifest_source,
            surface_source,
            surface_file,
            modules,
        })
    }

    /// The pinned modules in dependency order, with every `use` resolved
    /// and the closure budget checked: what a host installs the entry flow
    /// with, identical for the window and the windowless runner.
    pub fn closure(&self) -> Result<Closure, EvalError> {
        Closure::ordered(self.modules.clone())
    }

    /// Whether a server's accepted source and closure are already saved.
    /// Closure order is topological; manifest order need not be.
    pub fn matches_definition(&self, source: &str, modules: &[ModuleSource]) -> bool {
        self.entry_source == source && save::same_modules(&self.modules, modules)
    }

    /// Persist one accepted definition on a worker. Aliases and authored
    /// paths stay fixed; source and manifest pins change together with
    /// staged backups and rollback on ordinary I/O errors. Interrupted
    /// transactions retain their recovery directory and make `open` refuse
    /// explicitly; this does not promise a multi-file filesystem rename.
    pub fn save_definition(
        &self,
        source: &str,
        modules: &[ModuleSource],
    ) -> Result<Self, PackageError> {
        save::definition(self, source, modules)
    }

    /// The name a host installs the entry flow under in its engine: the
    /// package id with a `pkg-` prefix and dots turned into dashes, so an
    /// opened package can never overwrite a flow that is not its own.
    pub fn definition_name(&self) -> String {
        format!("pkg-{}", self.manifest.id.0.replace('.', "-"))
    }

    /// Write `source` to one of this package's own modules: the temp file
    /// beside it is renamed into place, so a reader sees the old text or the
    /// new, never a torn one. The path is confined the way `open` confines it.
    pub fn save_module(&self, module: &ModulePath, source: &str) -> Result<(), PackageError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        save::check_open(&self.root)?;
        // One temp name per write in this process, so two writers of one
        // module (an older session's save still landing, a newer one's
        // starting) never share a temp file whatever thread they run on.
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let path = confined(&self.root, module, "module")?;
        let write = WRITES.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_extension(format!("splash.{}.{write}.tmp", std::process::id()));
        std::fs::write(&temp, source.as_bytes()).map_err(|error| io(&temp, error))?;
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(io(&path, error));
        }
        Ok(())
    }
}

impl AppManifest {
    fn from_fields(fields: Vec<(String, Literal)>) -> Result<Self, PackageError> {
        let mut id = None;
        let mut name = None;
        let mut entry = None;
        let mut surface = None;
        let mut modules = Vec::new();
        let mut roles = Vec::new();
        let mut defaults = Vec::new();
        for (field, value) in fields {
            match field.as_str() {
                "id" => id = Some(string(&field, &value)?),
                "name" => name = Some(string(&field, &value)?),
                "entry" => entry = Some(module_path(&field, &value)?),
                "surface" => {
                    surface = match &value {
                        Literal::Null => None,
                        _ => Some(module_path(&field, &value)?),
                    }
                }
                "modules" => {
                    for item in array(&field, &value)? {
                        modules.push(pinned_module(item)?);
                    }
                }
                "roles" => {
                    for item in array(&field, &value)? {
                        roles.push(role(item)?);
                    }
                }
                "defaults" => match value {
                    Literal::Obj(values) => defaults = values,
                    _ => return Err(field_error(&field, "must be an object of input node ids")),
                },
                _ => return Err(PackageError::UnknownField(field)),
            }
        }
        let id = id.ok_or_else(|| field_error("id", "is required"))?;
        if !valid_package_id(&id) {
            return Err(field_error(
                "id",
                "must be lower-case dotted, such as `stage.hello`",
            ));
        }
        let entry = entry.ok_or_else(|| field_error("entry", "is required"))?;
        Ok(Self {
            name: name.unwrap_or_else(|| id.clone()),
            id: PackageId(id),
            entry,
            surface,
            modules,
            roles,
            defaults,
        })
    }
}

fn pinned_module(value: &Literal) -> Result<PinnedModule, PackageError> {
    match value {
        Literal::Str(_) => Ok(PinnedModule {
            path: module_path("modules", value)?,
            alias: None,
            hash: None,
        }),
        Literal::Obj(fields) => {
            let mut path = None;
            let mut alias = None;
            let mut hash = None;
            for (field, value) in fields {
                match field.as_str() {
                    "path" => path = Some(module_path("modules.path", value)?),
                    "as" => alias = Some(string("modules.as", value)?),
                    "hash" => {
                        let text = string("modules.hash", value)?;
                        if text.len() != 64
                            || !text
                                .bytes()
                                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                        {
                            return Err(field_error(
                                "modules.hash",
                                "must be 64 lower-case hex digits",
                            ));
                        }
                        hash = Some(text);
                    }
                    other => return Err(PackageError::UnknownField(format!("modules.{other}"))),
                }
            }
            Ok(PinnedModule {
                path: path.ok_or_else(|| field_error("modules.path", "is required"))?,
                alias,
                hash,
            })
        }
        _ => Err(field_error(
            "modules",
            "entries are a path or `{path, hash}`",
        )),
    }
}

fn role(value: &Literal) -> Result<RoleDeclaration, PackageError> {
    let Literal::Obj(fields) = value else {
        return Err(field_error("roles", "entries are `{role, kind, required}`"));
    };
    let mut role = None;
    let mut kind = None;
    let mut required = true;
    for (field, value) in fields {
        match field.as_str() {
            "role" => role = Some(string("roles.role", value)?),
            "kind" => kind = Some(string("roles.kind", value)?),
            "required" => match value {
                Literal::Bool(value) => required = *value,
                _ => return Err(field_error("roles.required", "must be a boolean")),
            },
            other => return Err(PackageError::UnknownField(format!("roles.{other}"))),
        }
    }
    Ok(RoleDeclaration {
        role: role.ok_or_else(|| field_error("roles.role", "is required"))?,
        kind: kind.ok_or_else(|| field_error("roles.kind", "is required"))?,
        required,
    })
}

fn string(field: &str, value: &Literal) -> Result<String, PackageError> {
    match value {
        Literal::Str(text) | Literal::Id(text) if !text.is_empty() => Ok(text.clone()),
        _ => Err(field_error(field, "must be a non-empty string")),
    }
}

fn array<'a>(field: &str, value: &'a Literal) -> Result<&'a [Literal], PackageError> {
    match value {
        Literal::Arr(values) => Ok(values),
        _ => Err(field_error(field, "must be an array")),
    }
}

fn module_path(field: &str, value: &Literal) -> Result<ModulePath, PackageError> {
    let text = string(field, value)?;
    let confined = !text.starts_with('/')
        && !text.contains('\\')
        && text
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if !confined {
        return Err(field_error(
            field,
            "must be a relative path inside the package",
        ));
    }
    Ok(ModulePath(text))
}

fn confined(root: &Path, module: &ModulePath, field: &str) -> Result<PathBuf, PackageError> {
    // `module_path` already rejected absolute paths and `..`; symlinks that
    // leave the package are rejected here.
    let path = root.join(&module.0);
    let canonical = path.canonicalize().map_err(|error| io(&path, error))?;
    let canonical_root = root.canonicalize().map_err(|error| io(root, error))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(field_error(field, "resolves outside the package"));
    }
    Ok(path)
}

fn read_source(path: &Path) -> Result<String, PackageError> {
    let metadata = std::fs::metadata(path).map_err(|error| io(path, error))?;
    if metadata.len() > graph::MAX_SOURCE_BYTES as u64 {
        return Err(PackageError::Io {
            path: path.display().to_string(),
            error: format!("source exceeds {} bytes", graph::MAX_SOURCE_BYTES),
        });
    }
    std::fs::read_to_string(path).map_err(|error| io(path, error))
}

fn source_hash(source: &str) -> String {
    let mut sha = Sha256::new();
    sha.update(source.as_bytes());
    to_hex(&sha.finish())
}

fn valid_package_id(id: &str) -> bool {
    id.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    })
}

fn field_error(field: &str, message: &str) -> PackageError {
    PackageError::Field {
        field: field.to_string(),
        message: message.to_string(),
    }
}

fn io(path: &Path, error: std::io::Error) -> PackageError {
    PackageError::Io {
        path: path.display().to_string(),
        error: error.to_string(),
    }
}
