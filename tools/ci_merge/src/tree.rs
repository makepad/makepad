use crate::toml::{self, Table, Token};
use makepad_ci_merge::{digest_input, digest_platform, inert_by_crate};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").current_dir(repo).args(args)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .output().ok()?;
    if !out.status.success() { return None; }
    String::from_utf8(out.stdout).ok()
}

fn get<'a>(table: &'a Table, key: &str) -> Option<&'a str> {
    toml::string(table.fields.get(key)?)
}

fn dependencies(table: &Table) -> Option<Option<&str>> {
    let parts: Vec<_> = table.name.split('.').collect();
    let i = parts.iter().position(|p| matches!(*p, "dependencies" | "dev-dependencies" | "build-dependencies"))?;
    match &parts[i + 1..] { [] => Some(None), [name] => Some(Some(*name)), _ => None }
}

/// HEAD is the candidate tree Git exposes while a low-level driver runs. It is not a
/// completed merge result (merge-tree supplies no such tree to its driver). CI still checks
/// every retained proof against the eventual tree before reusing it.
pub struct Digests {
    root: Option<PathBuf>,
    dir: PathBuf,
    inputs: BTreeMap<&'static str, Option<String>>,
}

impl Digests {
    pub fn new(path: &Path) -> Self {
        let repo = std::env::current_dir().ok().and_then(|cwd| git(&cwd, &["rev-parse", "--show-toplevel"]))
            .map(|s| PathBuf::from(s.trim()));
        let dir = repo.as_ref().map(|r| r.join(path).parent().unwrap_or(r).to_path_buf()).unwrap_or_default();
        let root = repo.map(|r| {
            if r.ends_with("apps/commercial") { r.parent().and_then(Path::parent).unwrap_or(&r).to_path_buf() } else { r }
        });
        Self { root, dir, inputs: BTreeMap::new() }
    }

    pub fn get(&mut self, section: &str) -> Option<String> {
        let (unit, os) = section.rsplit_once('.')?;
        let kind = match unit { "app" => "app", "tests" => "tests", _ => "job" };
        if !self.inputs.contains_key(kind) {
            let input = self.root.as_ref().and_then(|r| input(r, &self.dir, kind));
            self.inputs.insert(kind, input);
        }
        self.inputs.get(kind)?.as_ref().map(|s| digest_platform(s, os))
    }
}

fn manifest(dir: &Path) -> Option<Vec<Table>> {
    toml::parse(&std::fs::read_to_string(dir.join("Cargo.toml")).ok()?).ok()
}

fn closure(root: &Path, start: &Path, workspace: &BTreeMap<String, PathBuf>) -> Option<BTreeSet<PathBuf>> {
    let mut todo = vec![start.to_path_buf()];
    let mut dirs = BTreeSet::new();
    while let Some(dir) = todo.pop() {
        let dir = dir.canonicalize().ok()?;
        if !dir.starts_with(root) { return None; }
        if !dirs.insert(dir.clone()) { continue; }
        for table in manifest(&dir)? {
            if table.name.starts_with("workspace.") { continue; }
            let Some(named) = dependencies(&table) else { continue };
            if let Some(name) = named {
                if let Some(path) = get(&table, "path") { todo.push(dir.join(path)); }
                else if table.fields.get("workspace").is_some_and(|v| v == &[Token::Text("true".into())]) {
                    if let Some(path) = workspace.get(name) { todo.push(path.clone()); }
                }
            } else {
                for (name, value) in table.fields {
                    if let Some(path) = toml::field(&value, "path") { todo.push(dir.join(toml::string(path)?)); }
                    else if toml::field(&value, "workspace") == Some(&[Token::Text("true".into())][..]) {
                        if let Some(path) = workspace.get(&name) { todo.push(path.clone()); }
                    }
                }
            }
        }
    }
    Some(dirs)
}

/// `git grep -l` at HEAD: the files naming `name` under `dirs`.
fn naming(repo: &Path, name: &str, dirs: &[&str]) -> Option<Vec<String>> {
    let mut args = vec!["grep", "-l", "-F", "-e", name, "HEAD", "--"];
    args.extend(dirs.iter().copied());
    let out = Command::new("git").current_dir(repo).args(&args)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .output().ok()?;
    match out.status.code() {
        Some(0) => Some(String::from_utf8(out.stdout).ok()?.lines().filter_map(|l| l.strip_prefix("HEAD:")).map(str::to_string).collect()),
        Some(1) => Some(Vec::new()),
        _ => None,
    }
}

fn input(root: &Path, dir: &Path, kind: &str) -> Option<String> {
    let app = kind == "app";
    let root = root.canonicalize().ok()?;
    let commercial = root.join("apps/commercial");
    let mut workspace = BTreeMap::new();
    for table in manifest(&root)? {
        if table.name == "workspace.dependencies" {
            for (name, value) in table.fields {
                if let Some(path) = toml::field(&value, "path") { workspace.insert(name, root.join(toml::string(path)?)); }
            }
        } else if let Some(name) = table.name.strip_prefix("workspace.dependencies.") {
            if let Some(path) = get(&table, "path") { workspace.insert(name.to_string(), root.join(path)); }
        }
    }
    let mut dirs = closure(&root, dir, &workspace)?;
    if app {
        // Same runner closure and exclusions as devhub's app proofs.
        dirs.extend(closure(&root, &root.join("tools/run"), &workspace)?.into_iter()
            .filter(|p| !p.starts_with(root.join("libs/devhub")) && !p.starts_with(root.join("apps/devhub"))));
    }
    let mut names = BTreeSet::new();
    let mut by_repo: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    let mut crates = BTreeSet::new();
    for dir in &dirs {
        for table in manifest(dir)? {
            if table.name == "package" { names.insert(get(&table, "name")?.to_string()); }
        }
        let repo = if dir.starts_with(&commercial) { &commercial } else { &root };
        let rel = dir.strip_prefix(repo).ok()?.to_str()?.replace('\\', "/");
        crates.insert(if repo == &commercial { format!("apps/commercial/{rel}").trim_end_matches('/').to_string() } else { rel.clone() });
        by_repo.entry(repo.clone()).or_default().push(if rel.is_empty() { ".".into() } else { rel });
    }
    let mut lines = String::new();
    for (repo, paths) in by_repo {
        let mut args = vec!["ls-tree", "-r", "-z", "HEAD", "--"];
        args.extend(paths.iter().map(String::as_str));
        let listing = git(&repo, &args)?;
        if listing.is_empty() { return None; }
        let mut listed = BTreeSet::new();
        for row in listing.split('\0').filter(|r| !r.is_empty()) {
            let (head, path) = row.split_once('\t')?;
            if path.contains(['\n', '\r']) { return None; }
            listed.insert(path);
            let prefix = if repo == commercial { "apps/commercial/" } else { "" };
            lines.push_str(&format!("{head}\t{prefix}{path}\n"));
        }
        for dir in paths {
            let manifest = if dir == "." { "Cargo.toml".into() } else { format!("{dir}/Cargo.toml") };
            if !listed.contains(manifest.as_str()) { return None; }
        }
    }
    // Same documents (and tests' ci.splash) left out as devhub's proofs: those no Rust source of
    // their crate names.
    let rows: Vec<&str> = lines.lines().filter_map(|l| l.split_once('\t').map(|(_, p)| p)).collect();
    let mut out = BTreeSet::new();
    for (krate, files) in inert_by_crate(rows.into_iter(), &crates, kind == "tests") {
        let (repo, rel) = match krate.strip_prefix("apps/commercial/") { Some(r) => (&commercial, r), None => (&root, krate.as_str()) };
        let mut named: BTreeMap<&str, bool> = BTreeMap::new();
        for file in &files {
            let name = file.rsplit('/').next().unwrap_or(file);
            if !named.contains_key(name) {
                let found = naming(repo, name, &[if rel.is_empty() { "." } else { rel }])?;
                named.insert(name, found.iter().any(|p| p.ends_with(".rs")));
            }
            if !named[name] { out.insert(file.to_string()); }
        }
    }
    let lines: String = lines.lines().filter(|l| l.split_once('\t').is_none_or(|(_, p)| !out.contains(p))).map(|l| format!("{l}\n")).collect();
    // A missing or ambiguous lock file is not a digest with an empty external set.
    let external = external(&std::fs::read_to_string(root.join("Cargo.lock")).ok()?, &names)?;
    Some(digest_input(&lines, &external))
}

fn external(text: &str, roots: &BTreeSet<String>) -> Option<Vec<String>> {
    let packages: Vec<_> = toml::parse(text).ok()?.into_iter().filter(|t| t.array && t.name == "package").collect();
    let mut todo = Vec::new();
    for name in roots {
        let local: Vec<_> = packages.iter().enumerate().filter(|(_, p)| get(p, "name") == Some(name.as_str()) && get(p, "source").unwrap_or("").is_empty()).map(|(i, _)| i).collect();
        if local.is_empty() { return None; }
        todo.extend(local);
    }
    let mut seen = BTreeSet::new();
    let mut out = BTreeSet::new();
    while let Some(i) = todo.pop() {
        if !seen.insert(i) { continue; }
        let p = &packages[i];
        let source = get(p, "source").unwrap_or("");
        if !source.is_empty() {
            out.insert(format!("{}@{} {}", get(p, "name")?, get(p, "version")?, get(p, "checksum").filter(|s| !s.is_empty()).unwrap_or(source)));
        }
        if let Some(deps) = p.fields.get("dependencies") {
            let [Token::Punct('['), deps @ .., Token::Punct(']')] = deps.as_slice() else { return None };
            for dep in deps.iter().filter(|t| **t != Token::Punct(',')) {
                let Token::String(dep) = dep else { return None };
                let mut words = dep.split_whitespace();
                let name = words.next()?;
                let version = words.next();
                let source = words.next().map(|s| s.trim_matches(['(', ')']));
                let ids: Vec<_> = packages.iter().enumerate().filter(|(_, p)| get(p, "name") == Some(name)
                    && version.is_none_or(|v| get(p, "version") == Some(v))
                    && source.is_none_or(|s| get(p, "source") == Some(s))).map(|(i, _)| i).collect();
                if ids.len() != 1 { return None; }
                todo.extend(ids);
            }
        }
    }
    Some(out.into_iter().collect())
}
