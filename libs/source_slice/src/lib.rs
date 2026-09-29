//! A slice of a private source repository for one app license: exactly the
//! Cargo path-dependency closure of the app's root packages, as a synthetic
//! Git commit.
//!
//! The private repository (makepad/commercial) is checked out inside a
//! Makepad checkout at `apps/commercial`. Its root holds only a members
//! package (a `Cargo.toml` whose `[target.'cfg(any())'.dependencies]` name
//! every crate by path, and an empty `src/lib.rs`), so its crates are members
//! of Makepad's workspace. Someone licensed for one app gets:
//!
//! - every crate the app's root packages reach through `path` dependencies
//!   (`dependencies`, `build-dependencies` and each `target.*` variant of
//!   both; never `dev-dependencies`) that resolve inside the repository;
//!   paths that leave it point into Makepad, which the public repository
//!   provides. A root's optional dependencies count only when the slice's
//!   features turn them on, as the Builder builds the app: `cargo build -p
//!   <root> --no-default-features --features <features>` (see
//!   [`Crate::enabled_deps`]); every other crate's optional dependencies
//!   count, since which of their features the build enables is not
//!   followed;
//! - for each optional dependency of a root that the features leave off and
//!   that lies in the repository, a placeholder package in its directory
//!   instead of its source: Cargo reads the manifest of every path
//!   dependency, optional or not, to load the workspace. The placeholder has
//!   the package's name, version and feature names, no dependencies and an
//!   empty library, and is never built;
//! - each such crate's whole directory, less the directories of other
//!   packages nested in it that are not part of the closure;
//! - the paths a crate declares it needs outside its directory, in
//!   `[package.metadata.commercial] include = ["../../resources/x", ...]`
//!   (relative to the crate);
//! - a generated root `Cargo.toml` of the same members-package shape naming
//!   only those crates, and an empty `src/lib.rs`.
//!
//! Nothing else: no repository-root files, no other crates. Symbolic links
//! and submodules in the slice are refused. The tree is built with Git's own
//! object commands in the repository (`git mktree`, `git commit-tree`,
//! nothing is checked out) and committed without a parent under fixed
//! metadata, so the same source commit, roots and app give the same hash on
//! every machine. [`lint`] finds references a crate makes to files outside
//! its directory that its `include` does not cover.
//!
//! Everything goes through the `git` command line in the repository, which
//! may be bare.
use makepad_toml_parser::{parse_toml, Toml, TomlTable};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// The generated root package's name.
pub const MEMBERS_PACKAGE: &str = "makepad-commercial-members";
/// Author and committer of every slice commit.
pub const AUTHOR: (&str, &str) = ("Makepad", "info@makepad.nl");
/// The fixed author and commit date of every slice commit.
pub const DATE: &str = "@0 +0000";

/// Run `git -C repo args...`, feeding `input` on stdin, and return stdout.
pub fn git(repo: &Path, args: &[&str], input: Option<&[u8]>, env: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|e| format!("git {}: {e}", args.first().unwrap_or(&"")))?;
    // A large input goes in beside the reading of the output, so neither
    // side fills its pipe and waits for the other.
    let writer = match (input, child.stdin.take()) {
        (Some(data), Some(mut stdin)) => {
            let data = data.to_vec();
            Some(std::thread::spawn(move || stdin.write_all(&data)))
        }
        _ => None,
    };
    let out = child.wait_with_output().map_err(|e| format!("git {}: {e}", args.first().unwrap_or(&"")))?;
    if let Some(writer) = writer {
        let written = writer.join().map_err(|_| "git input writer panicked".to_string())?;
        if out.status.success() {
            written.map_err(|e| format!("git {}: {e}", args.first().unwrap_or(&"")))?;
        }
    }
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim().chars().take(1024).collect::<String>()
        ));
    }
    Ok(out.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map(|s| s.trim().to_owned()).map_err(|e| e.to_string())
}

/// `rel` resolved against the repository-relative directory `base`, without
/// touching the file system. `None` when the result leaves the repository
/// (a `..` above its root, or an absolute path).
pub fn resolve(base: &str, rel: &str) -> Option<String> {
    if rel.starts_with('/') || rel.contains('\\') || rel.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for segment in rel.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            segment => parts.push(segment),
        }
    }
    Some(parts.join("/"))
}

/// `path` equals `dir` or lies inside it (repository-relative, `""` is the root).
pub fn under(path: &str, dir: &str) -> bool {
    dir.is_empty() || path == dir || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
}

fn parent(path: &str) -> Option<&str> {
    if path.is_empty() {
        None
    } else {
        Some(path.rfind('/').map_or("", |i| &path[..i]))
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// One entry of the source commit's tree.
#[derive(Clone, Debug)]
struct Entry {
    mode: String,
    kind: String,
    oid: String,
    size: u64,
    path: String,
}

/// A package of the source repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crate {
    /// The package name.
    pub name: String,
    /// Its version (`0.0.0` when the manifest does not state one).
    pub version: String,
    /// Its directory, relative to the repository root.
    pub dir: String,
    /// Its library target's name (what `crate_resource("<lib>:…")` names).
    pub lib: String,
    /// Directories of its path dependencies that lie inside the repository:
    /// normal, build and every target's, never dev-dependencies, optional
    /// ones included.
    pub deps: Vec<String>,
    /// Its optional dependencies (normal, build and every target's) by the
    /// name its features use, each with the directories it resolves to
    /// inside the repository (none for a dependency from elsewhere).
    pub optional: BTreeMap<String, Vec<String>>,
    /// Directories of its path dependencies no feature is needed for.
    pub required: Vec<String>,
    /// Its `[features]` table.
    pub features: BTreeMap<String, Vec<String>>,
    /// `[package.metadata.commercial] include`, as written.
    pub include: Vec<String>,
    /// Paths the manifest itself names (`build`, `[lib] path`, `[[bin]] path`, ...).
    pub targets: Vec<String>,
    /// The build script, relative to the crate, when it has one.
    pub build: Option<String>,
}

impl Crate {
    fn parse(dir: &str, text: &str) -> Result<Option<Crate>, String> {
        let doc = parse_toml(text).map_err(|e| e.to_string())?;
        let Some(name) = doc.get_path(&["package", "name"]).and_then(Toml::as_str) else {
            return Ok(None);
        };
        let mut deps = BTreeSet::new();
        let mut required = BTreeSet::new();
        let mut optional: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut tables: Vec<&TomlTable> = Vec::new();
        for key in ["dependencies", "build-dependencies"] {
            if let Some(table) = doc.root.get(key).and_then(Toml::as_table) {
                tables.push(table);
            }
        }
        if let Some(targets) = doc.root.get("target").and_then(Toml::as_table) {
            for target in targets.values().filter_map(Toml::as_table) {
                for key in ["dependencies", "build-dependencies"] {
                    if let Some(table) = target.get(key).and_then(Toml::as_table) {
                        tables.push(table);
                    }
                }
            }
        }
        for table in tables {
            for (key, spec) in table.iter() {
                let spec = spec.as_table();
                let is_optional = spec.and_then(|t| t.get("optional")).and_then(Toml::as_bool).unwrap_or(false);
                let resolved = spec.and_then(|t| t.get("path")).and_then(Toml::as_str).and_then(|path| resolve(dir, path));
                if is_optional {
                    let dirs = optional.entry(key.clone()).or_default();
                    dirs.extend(resolved.clone());
                }
                if let Some(resolved) = resolved {
                    deps.insert(resolved.clone());
                    if !is_optional {
                        required.insert(resolved);
                    }
                }
            }
        }
        let mut features = BTreeMap::new();
        if let Some(table) = doc.root.get("features").and_then(Toml::as_table) {
            for (feature, list) in table.iter() {
                let list = list
                    .as_array()
                    .ok_or_else(|| format!("features.{feature} must be an array"))?
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| format!("features.{feature} must be an array of strings")))
                    .collect::<Result<Vec<_>, _>>()?;
                features.insert(feature.clone(), list);
            }
        }
        let include = match doc.get_path(&["package", "metadata", "commercial", "include"]) {
            None => Vec::new(),
            Some(value) => value
                .as_array()
                .ok_or("package.metadata.commercial.include must be an array of paths")?
                .iter()
                .map(|v| v.as_str().map(str::to_owned).ok_or("package.metadata.commercial.include must be an array of paths"))
                .collect::<Result<_, _>>()?,
        };
        let mut targets = Vec::new();
        let build = match doc.get_path(&["package", "build"]) {
            Some(Toml::Str(path, _)) => {
                targets.push(path.clone());
                Some(path.clone())
            }
            Some(Toml::Bool(false, _)) => None,
            _ => Some("build.rs".to_owned()),
        };
        if let Some(path) = doc.get_path(&["lib", "path"]).and_then(Toml::as_str) {
            targets.push(path.to_owned());
        }
        for kind in ["bin", "example", "test", "bench"] {
            for table in doc.root.get(kind).and_then(Toml::as_array_of_tables).unwrap_or(&[]) {
                if let Some(path) = table.get("path").and_then(Toml::as_str) {
                    targets.push(path.to_owned());
                }
            }
        }
        let lib = doc
            .get_path(&["lib", "name"])
            .and_then(Toml::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| name.replace('-', "_"));
        let version = doc.get_path(&["package", "version"]).and_then(Toml::as_str).unwrap_or("0.0.0").to_owned();
        Ok(Some(Crate {
            name: name.to_owned(),
            version,
            dir: dir.to_owned(),
            lib,
            deps: deps.into_iter().collect(),
            optional,
            required: required.into_iter().collect(),
            features,
            include,
            targets,
            build,
        }))
    }

    /// Whether `name` is a feature of this crate: a `[features]` entry, or
    /// the implicit feature of an optional dependency that no feature names
    /// as `dep:<name>`.
    pub fn has_feature(&self, name: &str) -> bool {
        self.features.contains_key(name) || self.implicit_feature(name)
    }

    fn implicit_feature(&self, name: &str) -> bool {
        self.optional.contains_key(name) && !self.features.values().flatten().any(|entry| entry.strip_prefix("dep:") == Some(name))
    }

    /// The optional dependencies `features` turn on, with no default
    /// features (`--no-default-features --features <features>`), followed
    /// through this crate's `[features]`: `dep:x` and `x/f` turn on the
    /// optional dependency `x`, `x?/f` does not, and a name is a feature of
    /// the table or the implicit feature of an optional dependency. What
    /// the enabled dependencies' own features do is not followed here.
    pub fn enabled_optional(&self, features: &[String]) -> Result<BTreeSet<String>, String> {
        let mut enabled = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut todo: Vec<String> = features.to_vec();
        while let Some(feature) = todo.pop() {
            if !seen.insert(feature.clone()) {
                continue;
            }
            if let Some(list) = self.features.get(&feature) {
                for entry in list {
                    if let Some(dep) = entry.strip_prefix("dep:") {
                        enabled.insert(dep.to_owned());
                    } else if let Some((dep, _)) = entry.split_once('/') {
                        // `x?/f` only reaches into `x` when something else
                        // turned it on; `x/f` turns on an optional `x`.
                        if !dep.ends_with('?') && self.optional.contains_key(dep) {
                            enabled.insert(dep.to_owned());
                        }
                    } else {
                        todo.push(entry.clone());
                    }
                }
            } else if self.implicit_feature(&feature) {
                enabled.insert(feature);
            } else {
                return Err(format!("{} has no feature {feature}", self.name));
            }
        }
        Ok(enabled)
    }

    /// The directories of the path dependencies inside the repository that
    /// a build with `features` (and no default features) compiles: every
    /// required one, and the optional ones those features turn on.
    pub fn enabled_deps(&self, features: &[String]) -> Result<Vec<String>, String> {
        let enabled = self.enabled_optional(features)?;
        let mut dirs: BTreeSet<String> = self.required.iter().cloned().collect();
        for dep in &enabled {
            dirs.extend(self.optional.get(dep).into_iter().flatten().cloned());
        }
        Ok(dirs.into_iter().collect())
    }
}

/// The source commit's tree and its packages, read once.
pub struct Source {
    /// The full hash of the source commit.
    pub commit: String,
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
    children: HashMap<String, Vec<usize>>,
    /// Every package, by directory.
    pub crates: BTreeMap<String, Crate>,
    /// Manifests that could not be read as packages, by directory.
    pub broken: BTreeMap<String, String>,
}

/// Read many blobs at once through `git cat-file --batch`.
fn read_blobs(repo: &Path, oids: &[&str]) -> Result<Vec<Vec<u8>>, String> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let input = oids.iter().map(|oid| format!("{oid}\n")).collect::<String>();
    let out = git(repo, &["cat-file", "--batch"], Some(input.as_bytes()), &[])?;
    let mut blobs = Vec::with_capacity(oids.len());
    let mut at = 0;
    for oid in oids {
        let end = out[at..].iter().position(|b| *b == b'\n').ok_or("git cat-file: short answer")? + at;
        let header = std::str::from_utf8(&out[at..end]).map_err(|e| e.to_string())?;
        let mut fields = header.split(' ');
        let (Some(_), Some(kind), Some(size)) = (fields.next(), fields.next(), fields.next()) else {
            return Err(format!("git cat-file: {oid}: {header}"));
        };
        if kind != "blob" {
            return Err(format!("git cat-file: {oid} is a {kind}"));
        }
        let size: usize = size.parse().map_err(|_| format!("git cat-file: {header}"))?;
        let start = end + 1;
        let data = out.get(start..start + size).ok_or("git cat-file: short answer")?;
        blobs.push(data.to_vec());
        at = start + size + 1;
    }
    Ok(blobs)
}

impl Source {
    /// Read the tree and every `Cargo.toml` of `commit` in `repo`.
    pub fn load(repo: &Path, commit: &str) -> Result<Source, String> {
        let commit = text(git(repo, &["rev-parse", "--verify", "--end-of-options", &format!("{commit}^{{commit}}")], None, &[])?)?;
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("Invalid source commit {commit}"));
        }
        let listing = git(repo, &["ls-tree", "-r", "-t", "-l", "-z", "--full-tree", &commit], None, &[])?;
        let mut entries = Vec::new();
        for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = std::str::from_utf8(record).map_err(|_| "A path in the source is not UTF-8".to_string())?;
            let (meta, path) = record.split_once('\t').ok_or("Invalid git ls-tree record")?;
            let mut fields = meta.split_whitespace();
            let (Some(mode), Some(kind), Some(oid), Some(size)) = (fields.next(), fields.next(), fields.next(), fields.next()) else {
                return Err(format!("Invalid git ls-tree record {meta}"));
            };
            entries.push(Entry { mode: mode.into(), kind: kind.into(), oid: oid.into(), size: size.parse().unwrap_or(0), path: path.into() });
        }
        let mut by_path = HashMap::new();
        let mut children: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            by_path.insert(entry.path.clone(), i);
            children.entry(parent(&entry.path).unwrap_or("").to_owned()).or_default().push(i);
        }
        let manifests: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.kind == "blob" && (e.path == "Cargo.toml" || e.path.ends_with("/Cargo.toml")))
            .collect();
        let blobs = read_blobs(repo, &manifests.iter().map(|e| e.oid.as_str()).collect::<Vec<_>>())?;
        let mut crates = BTreeMap::new();
        let mut broken = BTreeMap::new();
        for (entry, blob) in manifests.iter().zip(blobs) {
            let dir = parent(&entry.path).unwrap_or("").to_owned();
            let parsed = String::from_utf8(blob).map_err(|e| e.to_string()).and_then(|text| Crate::parse(&dir, &text));
            match parsed {
                Ok(Some(krate)) => {
                    crates.insert(dir, krate);
                }
                Ok(None) => {}
                Err(error) => {
                    broken.insert(dir, error);
                }
            }
        }
        Ok(Source { commit, entries, by_path, children, crates, broken })
    }

    /// Whether the source tree has a file or directory at `path`.
    pub fn exists(&self, path: &str) -> bool {
        path.is_empty() || self.by_path.contains_key(path)
    }

    fn named(&self, name: &str) -> Result<&Crate, String> {
        let found: Vec<&Crate> = self.crates.values().filter(|c| c.name == name).collect();
        match found.as_slice() {
            [one] => Ok(one),
            [] if self.broken.is_empty() => Err(format!("No package {name} in the source")),
            [] => Err(format!(
                "No package {name} in the source (unreadable manifests: {})",
                self.broken.iter().map(|(dir, e)| format!("{dir}/Cargo.toml: {e}")).collect::<Vec<_>>().join("; ")
            )),
            many => Err(format!(
                "Package {name} is ambiguous: {}",
                many.iter().map(|c| c.dir.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// The root packages' dependency directories with `features`: each
    /// root's required ones and the optional ones the features turn on. A
    /// feature applies to every root that has it; one no root has is an
    /// error.
    fn root_deps(&self, roots: &[String], features: &[String]) -> Result<BTreeMap<String, Vec<String>>, String> {
        let mut out = BTreeMap::new();
        let mut used = BTreeSet::new();
        for root in roots {
            let krate = self.named(root)?;
            let own: Vec<String> = features.iter().filter(|f| krate.has_feature(f)).cloned().collect();
            used.extend(own.iter().cloned());
            out.insert(krate.dir.clone(), krate.enabled_deps(&own)?);
        }
        if let Some(feature) = features.iter().find(|f| !used.contains(*f)) {
            return Err(format!("No root package ({}) has the feature {feature}", roots.join(", ")));
        }
        Ok(out)
    }

    /// The crates `roots` (package names) reach through path dependencies
    /// inside the repository when built with `features` and no default
    /// features, sorted by directory. The roots' optional dependencies are
    /// followed only as far as the features turn them on; every other
    /// crate's are all followed (which features a build enables in them is
    /// not tracked, so the slice errs on the side of shipping them).
    pub fn closure(&self, roots: &[String], features: &[String]) -> Result<Vec<&Crate>, String> {
        if roots.is_empty() {
            return Err("A slice needs at least one root package".into());
        }
        let root_deps = self.root_deps(roots, features)?;
        let mut todo = Vec::new();
        for root in roots {
            todo.push(self.named(root)?.dir.clone());
        }
        let mut seen = BTreeSet::new();
        while let Some(dir) = todo.pop() {
            if !seen.insert(dir.clone()) {
                continue;
            }
            let krate = &self.crates[&dir];
            if krate.dir.is_empty() {
                return Err(format!("{} is the repository's root package and cannot be part of a slice", krate.name));
            }
            for dep in root_deps.get(&dir).unwrap_or(&krate.deps) {
                if self.crates.contains_key(dep) {
                    todo.push(dep.clone());
                } else if let Some(error) = self.broken.get(dep) {
                    return Err(format!("{} depends on {dep}, whose Cargo.toml cannot be read: {error}", krate.name));
                } else if dep.is_empty() {
                    return Err(format!("{} depends on the repository root, which is not a crate of the slice", krate.name));
                } else if self.exists(dep) {
                    return Err(format!("{} depends on {dep}, which has no package Cargo.toml", krate.name));
                } else {
                    return Err(format!("{} depends on {dep}, which does not exist", krate.name));
                }
            }
        }
        Ok(seen.iter().map(|dir| &self.crates[dir]).collect())
    }

    /// A crate's `include` paths resolved to repository paths; every one must
    /// exist and stay inside the repository (and not be its root).
    pub fn includes(&self, krate: &Crate) -> Result<Vec<String>, String> {
        krate
            .include
            .iter()
            .map(|path| {
                let resolved = resolve(&krate.dir, path)
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| format!("{}: include {path} is outside the repository or its root", krate.name))?;
                if !self.exists(&resolved) {
                    return Err(format!("{}: include {path} ({resolved}) does not exist", krate.name));
                }
                Ok(resolved)
            })
            .collect()
    }

    fn selection(&self, closure: &[&Crate]) -> Result<Selection, String> {
        let mut marks = BTreeMap::new();
        let mut crate_dirs = BTreeSet::new();
        for krate in closure {
            marks.insert(krate.dir.clone(), true);
            crate_dirs.insert(krate.dir.clone());
            for include in self.includes(krate)? {
                marks.insert(include, true);
            }
        }
        for path in marks.keys() {
            if path == "Cargo.toml" || under(path, "src") {
                return Err(format!("{path} would replace the slice's generated root package"));
            }
        }
        let mut selection = Selection { marks, below: BTreeSet::new() };
        // A package nested in a crate of the slice that is not itself part
        // of it is cut out, unless a crate includes it explicitly.
        let nested: Vec<&str> = self.crates.keys().chain(self.broken.keys()).map(String::as_str).collect();
        let mut cuts = Vec::new();
        for dir in nested {
            if selection.marks.contains_key(dir) {
                continue;
            }
            let owner = selection.deepest(dir);
            if owner.is_some_and(|(path, selected)| selected && crate_dirs.contains(path)) {
                cuts.push(dir.to_owned());
            }
        }
        for dir in cuts {
            selection.marks.insert(dir, false);
        }
        for path in selection.marks.keys() {
            let mut at = parent(path);
            while let Some(dir) = at {
                selection.below.insert(dir.to_owned());
                at = parent(dir);
            }
        }
        Ok(selection)
    }

    /// Build the slice of `roots` with `features` for `app` as a parentless
    /// commit in `repo`.
    pub fn slice(&self, repo: &Path, app: &str, roots: &[String], features: &[String]) -> Result<Slice, String> {
        if app.is_empty() || !app.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(format!("Invalid app name {app:?}"));
        }
        let closure = self.closure(roots, features)?;
        let mut selection = self.selection(&closure)?;
        let placeholders = self.placeholders(roots, &closure)?;
        for krate in &placeholders {
            selection.cut(&krate.dir);
        }
        let mut files = 0;
        let mut bytes = 0;
        for entry in &self.entries {
            if entry.kind == "tree" || !selection.selected(&entry.path) {
                continue;
            }
            match entry.mode.as_str() {
                "120000" => return Err(format!("{} is a symbolic link; a slice holds only regular files", entry.path)),
                "160000" => return Err(format!("{} is a submodule; a slice holds only regular files", entry.path)),
                "100644" | "100755" => {}
                mode => return Err(format!("{} has the unexpected mode {mode}", entry.path)),
            }
            files += 1;
            bytes += entry.size;
        }
        let manifest = members_manifest(&closure.iter().map(|c| (c.name.clone(), c.dir.clone())).collect::<Vec<_>>());
        let manifest_oid = text(git(repo, &["hash-object", "-w", "--stdin"], Some(manifest.as_bytes()), &[])?)?;
        let lib_oid = text(git(repo, &["hash-object", "-w", "--stdin"], Some(b""), &[])?)?;
        let src = mktree(repo, &[("100644", "blob", &lib_oid, "lib.rs")])?;
        let mut stubs = HashMap::new();
        for krate in &placeholders {
            let text = placeholder_manifest(krate);
            let oid = text_oid(repo, &text)?;
            stubs.insert(krate.dir.clone(), mktree(repo, &[("100644", "blob", &oid, "Cargo.toml"), ("040000", "tree", &src, "src")])?);
            files += 2;
            bytes += text.len() as u64;
        }
        let tree = self
            .tree(repo, "", false, &selection, &stubs, &[("100644", "blob", &manifest_oid, "Cargo.toml"), ("040000", "tree", &src, "src")])?
            .ok_or("The slice is empty")?;
        let message = format!("commercial {} {app}", &self.commit[..12]);
        let (name, email) = AUTHOR;
        let commit = text(git(
            repo,
            &["commit-tree", "--no-gpg-sign", "-m", &message, &tree],
            None,
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_AUTHOR_DATE", DATE),
                ("GIT_COMMITTER_NAME", name),
                ("GIT_COMMITTER_EMAIL", email),
                ("GIT_COMMITTER_DATE", DATE),
            ],
        )?)?;
        Ok(Slice {
            source: self.commit.clone(),
            commit,
            app: app.to_owned(),
            crates: closure.iter().map(|c| (c.name.clone(), c.dir.clone())).collect(),
            placeholders: placeholders.iter().map(|c| (c.name.clone(), c.dir.clone())).collect(),
            files: files + 2,
            bytes: bytes + manifest.len() as u64,
        })
    }

    /// The optional dependencies of `roots` inside the repository that the
    /// slice leaves out (not in `closure`), by directory: each ships as a
    /// placeholder package.
    fn placeholders(&self, roots: &[String], closure: &[&Crate]) -> Result<Vec<&Crate>, String> {
        let shipped: BTreeSet<&str> = closure.iter().map(|c| c.dir.as_str()).collect();
        let mut out = BTreeMap::new();
        for root in roots {
            let root = self.named(root)?;
            for dir in root.optional.values().flatten() {
                if shipped.contains(dir.as_str()) {
                    continue;
                }
                let krate = self
                    .crates
                    .get(dir)
                    .ok_or_else(|| format!("{} depends on {dir}, which has no readable package Cargo.toml", root.name))?;
                out.insert(dir.clone(), krate);
            }
        }
        Ok(out.into_values().collect())
    }

    /// The slice's tree for `dir`: whole subtrees where nothing is cut below,
    /// new trees along the paths that lead to selected or cut entries, and
    /// the placeholder packages' trees (`stubs`, by directory).
    fn tree(
        &self,
        repo: &Path,
        dir: &str,
        inherited: bool,
        selection: &Selection,
        stubs: &HashMap<String, String>,
        extra: &[(&str, &str, &str, &str)],
    ) -> Result<Option<String>, String> {
        let mut rows: Vec<(String, String, String, String)> =
            extra.iter().map(|(m, k, o, n)| (m.to_string(), k.to_string(), o.to_string(), n.to_string())).collect();
        for i in self.children.get(dir).map(Vec::as_slice).unwrap_or(&[]) {
            let entry = &self.entries[*i];
            let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            let selected = selection.marks.get(&entry.path).copied().unwrap_or(inherited);
            if let Some(oid) = stubs.get(&entry.path) {
                rows.push(("040000".into(), "tree".into(), oid.clone(), name.into()));
            } else if entry.kind == "tree" {
                if selection.below.contains(&entry.path) {
                    if let Some(oid) = self.tree(repo, &entry.path, selected, selection, stubs, &[])? {
                        rows.push(("040000".into(), "tree".into(), oid, name.into()));
                    }
                } else if selected {
                    rows.push((entry.mode.clone(), entry.kind.clone(), entry.oid.clone(), name.into()));
                }
            } else if selected {
                rows.push((entry.mode.clone(), entry.kind.clone(), entry.oid.clone(), name.into()));
            }
        }
        if rows.is_empty() {
            return Ok(None);
        }
        let rows: Vec<(&str, &str, &str, &str)> = rows.iter().map(|(m, k, o, n)| (m.as_str(), k.as_str(), o.as_str(), n.as_str())).collect();
        mktree(repo, &rows).map(Some)
    }

    /// References the closure's crates make to files outside their own
    /// directory that the slice does not cover (see [`lint`]).
    pub fn lint(&self, repo: &Path, roots: &[String], features: &[String]) -> Result<Vec<Finding>, String> {
        let closure = self.closure(roots, features)?;
        let selection = self.selection(&closure)?;
        let in_closure: BTreeSet<&str> = closure.iter().map(|c| c.dir.as_str()).collect();
        let libs: HashMap<&str, &Crate> = self.crates.values().map(|c| (c.lib.as_str(), c)).collect();
        let mut findings = Vec::new();
        // The files each crate owns: under its directory and no deeper crate's.
        let mut files: Vec<(&Crate, &Entry)> = Vec::new();
        for entry in &self.entries {
            if entry.kind != "blob" || !selection.selected(&entry.path) {
                continue;
            }
            let owner = closure.iter().filter(|c| under(&entry.path, &c.dir)).max_by_key(|c| c.dir.len());
            let Some(owner) = owner else { continue };
            let relative = &entry.path[owner.dir.len() + 1..];
            let script = owner.build.as_deref().is_some_and(|b| resolve("", b).as_deref() == Some(relative));
            // A `fixtures/` directory holds inputs a test reads as data (Scope's
            // syntax fixtures are Rust source that is parsed, never compiled), so
            // the references written in them are not the crate's.
            let fixture = relative.split('/').any(|part| part == "fixtures");
            if !fixture && (script || entry.path.ends_with(".rs") || entry.path.ends_with(".splash")) {
                files.push((owner, entry));
            }
        }
        for krate in &closure {
            for target in &krate.targets {
                let covered = self.covered(krate, &selection, &in_closure);
                if let Some(resolved) = resolve(&krate.dir, target) {
                    if !under(&resolved, &krate.dir) && !covered(&resolved)? {
                        findings.push(Finding {
                            krate: krate.name.clone(),
                            file: join(&krate.dir, "Cargo.toml"),
                            line: 0,
                            reference: format!("path = {target:?}"),
                            target: resolved,
                            problem: "outside the crate and not in its include".into(),
                        });
                    }
                }
            }
        }
        let blobs = read_blobs(repo, &files.iter().map(|(_, e)| e.oid.as_str()).collect::<Vec<_>>())?;
        for ((krate, entry), blob) in files.iter().zip(blobs) {
            let Ok(code) = String::from_utf8(blob) else { continue };
            let file_dir = parent(&entry.path).unwrap_or("");
            let relative = &entry.path[krate.dir.len() + 1..];
            let script = krate.build.as_deref().is_some_and(|b| resolve("", b).as_deref() == Some(relative));
            for reference in references(&code, script) {
                let (base, path) = match &reference.base {
                    Base::File => (file_dir.to_owned(), reference.path.clone()),
                    Base::Crate => (krate.dir.clone(), reference.path.clone()),
                    Base::Named(lib) => match libs.get(lib.as_str()) {
                        Some(other) if !in_closure.contains(other.dir.as_str()) => {
                            findings.push(Finding {
                                krate: krate.name.clone(),
                                file: entry.path.clone(),
                                line: reference.line,
                                reference: reference.text.clone(),
                                target: other.dir.clone(),
                                problem: format!("names {}, which is not part of the slice", other.name),
                            });
                            continue;
                        }
                        Some(other) => (other.dir.clone(), reference.path.clone()),
                        // A Makepad crate, from the public repository.
                        None => continue,
                    },
                };
                // Paths that leave the repository are Makepad's.
                let Some(resolved) = resolve(&base, &path) else { continue };
                let home = match &reference.base {
                    Base::Named(lib) => libs[lib.as_str()].dir.clone(),
                    _ => krate.dir.clone(),
                };
                let problem = if !self.exists(&resolved) {
                    "does not exist in the source"
                } else if under(&resolved, &home) && selection.selected(&resolved) {
                    continue;
                } else if self.covered(krate, &selection, &in_closure)(&resolved)? {
                    continue;
                } else {
                    "outside the crate and not in its include"
                };
                findings.push(Finding {
                    krate: krate.name.clone(),
                    file: entry.path.clone(),
                    line: reference.line,
                    reference: reference.text.clone(),
                    target: resolved,
                    problem: problem.into(),
                });
            }
        }
        findings.sort_by(|a, b| (&a.file, a.line, &a.reference).cmp(&(&b.file, b.line, &b.reference)));
        findings.dedup();
        Ok(findings)
    }

    /// Whether `path` is shipped because `krate` itself or one of its own
    /// dependencies (never another root's) brings it: it lies under one of
    /// their directories or includes, and is not cut from the slice.
    fn covered<'a>(
        &'a self,
        krate: &'a Crate,
        selection: &'a Selection,
        in_closure: &'a BTreeSet<&str>,
    ) -> impl Fn(&str) -> Result<bool, String> + 'a {
        move |path: &str| {
            if !selection.selected(path) {
                return Ok(false);
            }
            let mut todo = vec![krate.dir.clone()];
            let mut seen = BTreeSet::new();
            while let Some(dir) = todo.pop() {
                if !seen.insert(dir.clone()) || !in_closure.contains(dir.as_str()) {
                    continue;
                }
                let own = &self.crates[&dir];
                if under(path, &own.dir) || self.includes(own)?.iter().any(|include| under(path, include)) {
                    return Ok(true);
                }
                todo.extend(own.deps.iter().cloned());
            }
            Ok(false)
        }
    }
}

/// What the slice holds: `marks` are the selected (true) and cut (false)
/// paths, the deepest mark above a path deciding for it; `below` are the
/// directories some mark lies in.
struct Selection {
    marks: BTreeMap<String, bool>,
    below: BTreeSet<String>,
}

impl Selection {
    fn deepest<'a>(&'a self, path: &'a str) -> Option<(&'a str, bool)> {
        let mut at = Some(path);
        while let Some(p) = at {
            if let Some((key, selected)) = self.marks.get_key_value(p) {
                return Some((key.as_str(), *selected));
            }
            at = parent(p);
        }
        None
    }
    fn selected(&self, path: &str) -> bool {
        self.deepest(path).is_some_and(|(_, selected)| selected)
    }
    /// Leave `dir` out, walking the directories above it.
    fn cut(&mut self, dir: &str) {
        self.marks.insert(dir.to_owned(), false);
        let mut at = parent(dir);
        while let Some(up) = at {
            self.below.insert(up.to_owned());
            at = parent(up);
        }
    }
}

fn mktree(repo: &Path, rows: &[(&str, &str, &str, &str)]) -> Result<String, String> {
    let mut input = Vec::new();
    for (mode, kind, oid, name) in rows {
        input.extend_from_slice(format!("{mode} {kind} {oid}\t{name}").as_bytes());
        input.push(0);
    }
    text(git(repo, &["mktree", "-z"], Some(&input), &[])?)
}

/// The generated root `Cargo.toml`: the members package naming `crates`
/// (package name, directory) as never-built path dependencies.
pub fn members_manifest(crates: &[(String, String)]) -> String {
    let mut sorted = crates.to_vec();
    sorted.sort();
    let mut out = format!(
        "# Generated by makepad-source-slice: the crates of this slice of the\n\
         # private repository. Checked out inside a Makepad checkout at\n\
         # apps/commercial, this package makes them members of Makepad's\n\
         # workspace; cfg(any()) never holds, so nothing builds through it.\n\
         [package]\n\
         name = \"{MEMBERS_PACKAGE}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [target.'cfg(any())'.dependencies]\n"
    );
    for (name, dir) in sorted {
        out.push_str(&format!("\"{name}\" = {{ path = \"{dir}\" }}\n"));
    }
    out
}

/// A placeholder for `krate`, an optional dependency of a root that the
/// slice leaves out: its name, version and feature names (so the features
/// that name it still resolve), no dependencies, an empty library.
pub fn placeholder_manifest(krate: &Crate) -> String {
    let mut out = format!(
        "# Generated by makepad-source-slice: {} is not part of this slice (an\n\
         # optional dependency its features leave off). Cargo reads the manifest\n\
         # of every path dependency to load the workspace; this one is never built.\n\
         [package]\n\
         name = \"{}\"\n\
         version = \"{}\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [lib]\n\
         path = \"src/lib.rs\"\n",
        krate.name, krate.name, krate.version
    );
    let names: BTreeSet<&str> = krate
        .features
        .keys()
        .map(String::as_str)
        .chain(krate.optional.keys().map(String::as_str).filter(|name| krate.implicit_feature(name)))
        .collect();
    if !names.is_empty() {
        out.push_str("\n[features]\n");
        for name in names {
            out.push_str(&format!("\"{name}\" = []\n"));
        }
    }
    out
}

fn text_oid(repo: &Path, content: &str) -> Result<String, String> {
    text(git(repo, &["hash-object", "-w", "--stdin"], Some(content.as_bytes()), &[])?)
}

/// A slice commit and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    /// The source commit it was cut from.
    pub source: String,
    /// The synthetic, parentless slice commit.
    pub commit: String,
    pub app: String,
    /// Its crates: (package name, directory), sorted by directory.
    pub crates: Vec<(String, String)>,
    /// The roots' optional dependencies it holds only as placeholder
    /// packages: (package name, directory), sorted by directory.
    pub placeholders: Vec<(String, String)>,
    /// Files in the slice, the generated ones included.
    pub files: u64,
    /// Their size in bytes.
    pub bytes: u64,
}

/// Cut the slice of `roots` (package names) built with `features` at
/// `commit` for `app`. The commit is written into `repo` and its hash
/// returned in [`Slice::commit`].
pub fn slice(repo: &Path, commit: &str, app: &str, roots: &[String], features: &[String]) -> Result<Slice, String> {
    Source::load(repo, commit)?.slice(repo, app, roots, features)
}

/// References the crates of the slice of `roots` at `commit` make to files
/// outside their own directory that the slice does not ship through their
/// `include` (or a dependency's directory): `include_str!`, `include_bytes!`,
/// `include!` and `#[path]` relative to the file, `crate_resource("self:…")`
/// and `CARGO_MANIFEST_DIR` joins relative to the crate, `crate_resource`
/// of a repository crate outside the slice, `"../…"` literals in build
/// scripts, and manifest target paths. Paths that leave the repository are
/// Makepad's and are not reported.
pub fn lint(repo: &Path, commit: &str, roots: &[String], features: &[String]) -> Result<Vec<Finding>, String> {
    Source::load(repo, commit)?.lint(repo, roots, features)
}

/// Write the tree of `commit` into `out` (which must be empty or absent)
/// through `git archive`.
pub fn materialize(repo: &Path, commit: &str, out: &Path) -> Result<(), String> {
    if out.exists() && std::fs::read_dir(out).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty", out.display()));
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let archive = git(repo, &["archive", "--format=tar", commit], None, &[])?;
    let mut tar = Command::new("tar")
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("tar: {e}"))?;
    let mut stdin = tar.stdin.take().ok_or("tar: no stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&archive));
    let result = tar.wait_with_output().map_err(|e| format!("tar: {e}"))?;
    writer.join().map_err(|_| "tar writer panicked".to_string())?.map_err(|e| format!("tar: {e}"))?;
    if !result.status.success() {
        return Err(format!("tar: {}", String::from_utf8_lossy(&result.stderr).trim()));
    }
    Ok(())
}

/// A reference [`lint`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The package whose file makes it.
    pub krate: String,
    /// The repository path of that file.
    pub file: String,
    /// Its line, from 1 (0: the manifest as a whole).
    pub line: usize,
    /// The reference as written.
    pub reference: String,
    /// The repository path it resolves to.
    pub target: String,
    pub problem: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {} -> {}: {} ({})", self.file, self.line, self.reference, self.target, self.problem, self.krate)
    }
}

enum Base {
    /// Relative to the referring file's directory.
    File,
    /// Relative to the referring crate's directory.
    Crate,
    /// Relative to the directory of the crate with this library name.
    Named(String),
}

struct Reference {
    base: Base,
    path: String,
    text: String,
    line: usize,
}

/// A string literal (plain or raw) at the start of `s` after whitespace:
/// its value and the byte length consumed.
fn literal(s: &str) -> Option<(String, usize)> {
    let trimmed = s.trim_start();
    let skipped = s.len() - trimmed.len();
    let bytes = trimmed.as_bytes();
    if bytes.first() == Some(&b'r') {
        let hashes = bytes[1..].iter().take_while(|b| **b == b'#').count();
        if bytes.get(1 + hashes) != Some(&b'"') {
            return None;
        }
        let close = format!("\"{}", "#".repeat(hashes));
        let body = &trimmed[2 + hashes..];
        let end = body.find(&close)?;
        return Some((body[..end].to_owned(), skipped + 2 + hashes + end + close.len()));
    }
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut value = String::new();
    let mut chars = trimmed[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((value, skipped + 1 + i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                other => value.push(other),
            },
            c => value.push(c),
        }
    }
    None
}

/// Whether the match at `at` is code: not preceded by an identifier
/// character and not after `//` on its line.
fn code_at(code: &str, at: usize) -> bool {
    let before = code[..at].chars().next_back();
    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    let line = &code[code[..at].rfind('\n').map_or(0, |i| i + 1)..at];
    !line.contains("//")
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

/// The file references in `code` (a `.rs` or `.splash` file, or a build
/// script when `script`).
fn references(code: &str, script: bool) -> Vec<Reference> {
    let mut out = Vec::new();
    let find = |needle: &str, f: &mut dyn FnMut(usize, &str)| {
        let mut from = 0;
        while let Some(i) = code[from..].find(needle) {
            let at = from + i;
            from = at + needle.len();
            if code_at(code, at) {
                f(at, &code[at + needle.len()..]);
            }
        }
    };
    for mac in ["include_str!", "include_bytes!", "include!"] {
        find(mac, &mut |at, rest| {
            let rest_trim = rest.trim_start();
            let Some(args) = rest_trim.strip_prefix('(') else { return };
            if let Some((path, len)) = literal(args) {
                let text = format!("{mac}({})", &args[..len].trim());
                out.push(Reference { base: Base::File, path, text, line: line_of(code, at) });
            }
        });
    }
    find("#[path", &mut |at, rest| {
        let Some(rest) = rest.trim_start().strip_prefix('=') else { return };
        if let Some((path, len)) = literal(rest) {
            out.push(Reference { base: Base::File, text: format!("#[path = {}]", rest[..len].trim()), path, line: line_of(code, at) });
        }
    });
    find("crate_resource(", &mut |at, rest| {
        if let Some((value, len)) = literal(rest) {
            let Some((name, path)) = value.split_once(':') else { return };
            let base = if name == "self" { Base::Crate } else { Base::Named(name.to_owned()) };
            out.push(Reference { base, path: path.to_owned(), text: format!("crate_resource({})", rest[..len].trim()), line: line_of(code, at) });
        }
    });
    find("env!(\"CARGO_MANIFEST_DIR\")", &mut |at, rest| {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(',').unwrap_or(rest);
        if let Some((value, len)) = literal(rest) {
            if value.starts_with("/..") {
                out.push(Reference {
                    base: Base::Crate,
                    path: value[1..].to_owned(),
                    text: format!("env!(\"CARGO_MANIFEST_DIR\") + {}", rest[..len].trim()),
                    line: line_of(code, at),
                });
            }
        }
    });
    if script {
        find("\"../", &mut |at, _| {
            if let Some((path, len)) = literal(&code[at..]) {
                out.push(Reference { base: Base::Crate, path, text: code[at..at + len].to_owned(), line: line_of(code, at) });
            }
        });
    }
    out
}

#[cfg(test)]
mod tests;
