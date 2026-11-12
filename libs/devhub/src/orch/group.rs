//! A tip's delta against a base as one commit per logical block ([`BLOCKS_FILE`], in dependency
//! order): the set `devhub push` sends to origin/work and `devhub promote` sends to main. The last
//! commit's tree is the tip's tree, or no set comes out. Trees and commits are written in-process;
//! only the text diff (for the line counts and a caller's scan) and the commit identity ask git.

use makepad_git::commit::parse_signature;
use makepad_git::{Commit, GitError, ObjectId, Repository, Signature, Tree, TreeChange, TreeEntry};
use makepad_toml_parser::{parse_toml, Toml, TomlDocument, TomlTable};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub const BLOCKS_FILE: &str = "tools/devhub/promote_blocks.txt";
const ROOT_MANIFEST: &str = "Cargo.toml";
/// A commit message names this many areas, and how many more there are.
const AREAS: usize = 12;

pub struct Block {
    pub title: String,
    patterns: Vec<String>,
}

/// What the tip has at a path: object and mode.
type Entry = (ObjectId, u32);

/// One file of the delta.
#[derive(Clone)]
pub struct Change {
    pub path: String,
    /// None: the tip has no file here.
    to: Option<Entry>,
    /// The base has no file here.
    added: bool,
}

/// One block's part of the delta.
#[derive(Default, Clone)]
pub struct Stat {
    pub files: Vec<Change>,
    pub added: u64,
    pub removed: u64,
}

/// A package of a tree: its name and the directories of its path dependencies.
pub struct Crate {
    pub name: String,
    pub deps: BTreeSet<String>,
    /// The ones it needs outside its tests: not its dev-dependencies.
    pub non_test_deps: BTreeSet<String>,
}

/// A crate the tip has and the base doesn't.
#[derive(Debug, PartialEq)]
pub struct NewCrate {
    pub dir: String,
    pub name: String,
    /// The crates that name it as a path dependency, the private checkout's too.
    pub users: Vec<String>,
    /// Private crates use it and no public one needs it outside its tests, followed up through the
    /// public crates among its users.
    pub only_private: bool,
}

/// Every manifest of a tree, by directory ("" is the root), and the ones that didn't parse.
#[derive(Default)]
pub struct Manifests {
    pub docs: BTreeMap<String, TomlDocument>,
    pub unread: Vec<String>,
}

pub struct Groups {
    pub base: String,
    pub tip: String,
    base_tree: ObjectId,
    tip_tree: ObjectId,
    pub blocks: Vec<Block>,
    pub stats: Vec<Stat>,
    pub binaries: u32,
    /// The text diff of base and tip (`--unified=0`), for a caller's scan.
    pub diff: String,
    /// The tip's root manifest.
    pub root: String,
    root_entry: Option<Entry>,
    pub manifests: Manifests,
    /// Each block's `Contributors:` line ([`Self::credit`]; empty: none).
    pub contributors: Vec<String>,
}

/// `*` matches anything, `/` too; everything else is literal.
pub fn matches(pattern: &str, path: &str) -> bool {
    let (p, s) = (pattern.as_bytes(), path.as_bytes());
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < s.len() {
        if i < p.len() && p[i] == b'*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if i < p.len() && p[i] == s[j] {
            i += 1;
            j += 1;
        } else if let Some(at) = star {
            i = at + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == b'*')
}

pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some(title) = line.strip_prefix("block ") {
            blocks.push(Block { title: title.to_string(), patterns: Vec::new() });
        } else if let Some(block) = blocks.last_mut() {
            block.patterns.push(line.to_string());
        }
    }
    blocks
}

/// One commit of the set, as [`contributor_lines`] reads it: `files` are what it changed (for a
/// merge, what it brought in over its first parent).
#[derive(Clone, Debug, Default)]
pub struct Landed {
    pub name: String,
    pub email: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<String>,
}

/// The PR in a merge subject: `Merge PR #N from <handle>: ...` or `Merge pull request #N from <handle>/<branch>`.
pub fn merged_pr(subject: &str) -> Option<(String, u32)> {
    let rest = subject.strip_prefix("Merge PR #").or_else(|| subject.strip_prefix("Merge pull request #"))?;
    let (number, rest) = rest.split_once(" from ")?;
    let handle: String = rest.chars().take_while(|c| !matches!(c, ':' | '/' | ' ')).collect();
    (!handle.is_empty()).then_some(())?;
    Some((handle, number.trim().parse().ok()?))
}

/// The number in a subject ending `(#N)`.
fn subject_pr(subject: &str) -> Option<u32> {
    let inner = subject.trim_end().strip_suffix(')')?;
    inner[inner.rfind("(#")? + 2..].parse().ok()
}

/// A GitHub handle out of a no-reply address (`<id>+<handle>@users.noreply.github.com`).
fn noreply_handle(email: &str) -> Option<String> {
    let local = email.trim().strip_suffix("@users.noreply.github.com")?;
    Some(local.rsplit('+').next().unwrap_or(local).to_string())
}

/// A PR a commit says it was ported from, `Based on makepad/<repo>#N by <handle>` in its body:
/// (repo, number, handle).
fn based_on(body: &str) -> Vec<(String, u32, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(rest) = line.trim().strip_prefix("Based on makepad/") else { continue };
        let Some((repo, rest)) = rest.split_once('#') else { continue };
        let Some((number, rest)) = rest.split_once(" by ") else { continue };
        let handle = rest.trim().trim_start_matches('@').trim_end_matches(|c: char| !c.is_alphanumeric());
        if let (Ok(n), false) = (number.trim().parse(), handle.is_empty()) {
            out.push((if repo == "makepad" { String::new() } else { repo.to_string() }, n, handle.to_string()));
        }
    }
    out
}

/// The `Contributors:` line of each block's commit (empty: none; `held`: the blocks with a
/// commit). The outside people whose PRs the set carries are named by handle and PR number, never
/// an address. A PR is one of: a `Merge PR #N from <handle>` commit (its handle; the files it
/// brought in), a commit by someone other than `own` (by email, in any case) whose subject ends
/// `(#N)` (its author's no-reply handle, else the handle another of their commits gave, else
/// their name), or a `Based on makepad/<repo>#N by <handle>` port. A PR is named in each block
/// holding a file it changed, or in the last commit when none of those has one (its change came
/// and went). A PR of another repository reads `relax#31`, one of this repository `#1403`.
pub fn contributor_lines(blocks: &[Block], held: &[bool], commits: &[Landed], own: &str) -> Vec<String> {
    let is_own = |email: &str| email.trim().is_empty() || email.trim().eq_ignore_ascii_case(own.trim());
    let mut handles: HashMap<&str, String> = HashMap::new();
    for c in commits {
        if let Some(h) = noreply_handle(&c.email) {
            handles.insert(c.name.trim(), h);
        }
    }
    let last = held.iter().rposition(|h| *h);
    let mut out: Vec<BTreeMap<(String, String), BTreeSet<(String, u32)>>> = vec![BTreeMap::new(); blocks.len()];
    for c in commits {
        let mut prs: Vec<(String, String, u32)> = Vec::new();
        if let Some((handle, n)) = merged_pr(&c.subject) {
            prs.push((handle, String::new(), n));
        } else if let (false, Some(n)) = (is_own(&c.email), subject_pr(&c.subject)) {
            let handle = noreply_handle(&c.email).or_else(|| handles.get(c.name.trim()).cloned()).unwrap_or_else(|| c.name.trim().to_string());
            prs.push((handle, String::new(), n));
        }
        prs.extend(based_on(&c.body).into_iter().map(|(repo, n, handle)| (handle, repo, n)));
        for (handle, repo, n) in prs {
            let mut blocks_of: BTreeSet<usize> = c.files.iter().map(|f| block_of(blocks, f)).filter(|b| held.get(*b) == Some(&true)).collect();
            if blocks_of.is_empty() {
                blocks_of.extend(last);
            }
            for b in blocks_of {
                out[b].entry((handle.to_lowercase(), handle.clone())).or_default().insert((repo.clone(), n));
            }
        }
    }
    out.into_iter()
        .map(|people| {
            if people.is_empty() {
                return String::new();
            }
            let named: Vec<String> = people.into_iter().map(|((_, handle), prs)| format!("{handle} ({})", pr_list(&prs.into_iter().collect::<Vec<_>>()))).collect();
            format!("Contributors: {}", named.join(", "))
        })
        .collect()
}

/// `#1403-#1405, #1409, relax#31`: runs of three or more consecutive numbers of the person's own
/// list become a range, so a range never hides a number somebody else holds.
fn pr_list(prs: &[(String, u32)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < prs.len() {
        let mut end = at;
        while end + 1 < prs.len() && prs[end + 1].0 == prs[at].0 && prs[end + 1].1 == prs[end].1 + 1 {
            end += 1;
        }
        let (repo, from) = (&prs[at].0, prs[at].1);
        out.push(if end - at >= 2 { format!("{repo}#{from}-#{}", prs[end].1) } else { (at..=end).map(|i| format!("{repo}#{}", prs[i].1)).collect::<Vec<_>>().join(", ") });
        at = end + 1;
    }
    out.join(", ")
}

pub fn block_of(blocks: &[Block], path: &str) -> usize {
    blocks.iter().position(|b| b.patterns.iter().any(|p| matches(p, path))).unwrap_or(blocks.len() - 1)
}

/// `a/b/../c` as `a/c`.
pub fn normal(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The directories of a crate's path dependencies of these `kinds` (`dev-dependencies` and so on),
/// as written in its manifest; `workspace` is the nearest workspace's directory and dependency
/// table, for `workspace = true`.
fn path_deps(dir: &str, doc: &TomlTable, workspace: Option<(&str, &TomlTable)>, kinds: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut tables: Vec<&TomlTable> = Vec::new();
    tables.extend(kinds.iter().filter_map(|n| doc.get(*n).and_then(Toml::as_table)));
    if let Some(targets) = doc.get("target").and_then(Toml::as_table) {
        for target in targets.values().filter_map(Toml::as_table) {
            tables.extend(kinds.iter().filter_map(|n| target.get(*n).and_then(Toml::as_table)));
        }
    }
    for table in tables {
        for (name, dep) in table {
            let Some(dep) = dep.as_table() else { continue };
            if dep.get("workspace").and_then(Toml::as_bool) == Some(true) {
                let Some((at, deps)) = workspace else { continue };
                if let Some(p) = deps.get(name).and_then(Toml::as_table).and_then(|d| d.get("path")).and_then(Toml::as_str) {
                    out.insert(normal(&format!("{at}/{p}")));
                }
            } else if let Some(p) = dep.get("path").and_then(Toml::as_str) {
                out.insert(normal(&format!("{dir}/{p}")));
            }
        }
    }
    out
}

/// The workspace `dir` belongs to: the nearest manifest up the tree with a `[workspace]`.
fn workspace_of<'a>(dir: &str, docs: &'a BTreeMap<String, TomlDocument>) -> Option<(&'a str, &'a TomlTable)> {
    let mut at = dir;
    loop {
        if let Some((key, doc)) = docs.get_key_value(at) {
            if doc.root.contains_key("workspace") {
                return doc.get_path(&["workspace", "dependencies"]).and_then(Toml::as_table).map(|t| (key.as_str(), t));
            }
        }
        if at.is_empty() {
            return None;
        }
        at = at.rsplit_once('/').map_or("", |(up, _)| up);
    }
}

/// The packages among `docs`, by directory.
pub fn crates_of(docs: &BTreeMap<String, TomlDocument>) -> BTreeMap<String, Crate> {
    docs.iter()
        .filter(|(dir, doc)| !dir.is_empty() && doc.root.contains_key("package"))
        .map(|(dir, doc)| {
            let name = doc.get_path(&["package", "name"]).and_then(Toml::as_str).unwrap_or("").to_string();
            let workspace = workspace_of(dir, docs);
            let deps = path_deps(dir, &doc.root, workspace, &["dependencies", "build-dependencies", "dev-dependencies"]);
            let non_test_deps = path_deps(dir, &doc.root, workspace, &["dependencies", "build-dependencies"]);
            (dir.clone(), Crate { name, deps, non_test_deps })
        })
        .collect()
}

/// The public crates that only private ones end up using. Non-test users decide: a public crate's
/// tests don't keep a crate public, and a crate only public tests use is no private crate's.
fn only_private<'a>(all: &'a BTreeMap<String, Crate>, is_private: &dyn Fn(&str) -> bool) -> BTreeSet<&'a str> {
    let mut non_test: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut private_test: BTreeSet<&str> = BTreeSet::new();
    for (dir, c) in all {
        for dep in &c.non_test_deps {
            non_test.entry(dep.as_str()).or_default().insert(dir.as_str());
        }
        if is_private(dir) {
            private_test.extend(c.deps.iter().map(String::as_str));
        }
    }
    // every public crate a private crate or a non-test user names, until a public non-test user
    // outside the set clears it
    let mut only: BTreeSet<&str> = all.keys().map(String::as_str).filter(|d| !is_private(d) && (non_test.contains_key(d) || private_test.contains(d))).collect();
    loop {
        let cleared: Vec<&str> = only.iter().copied().filter(|d| non_test.get(d).is_some_and(|users| users.iter().any(|u| !is_private(u) && !only.contains(u)))).collect();
        if cleared.is_empty() {
            return only;
        }
        cleared.iter().for_each(|d| {
            only.remove(d);
        });
    }
}

/// Every manifest of the tree of commit `at` in the repository of `dir`, keyed as a path under
/// `under` ("" for the tree's own place).
pub fn manifests_at(dir: &Path, at: &str, under: &str) -> Result<Manifests, String> {
    let mut out = Manifests::default();
    let files = crate::gitread::files_at(dir, at)?;
    for (path, oid) in files.iter().filter(|(p, _)| p.as_str() == ROOT_MANIFEST || p.ends_with("/Cargo.toml")) {
        let bytes = crate::gitread::blob(dir, oid)?;
        let local = path.strip_suffix(ROOT_MANIFEST).unwrap_or(path).trim_end_matches('/');
        let key = normal(&format!("{under}/{local}"));
        match std::str::from_utf8(&bytes).ok().and_then(|text| parse_toml(text).ok()) {
            Some(doc) => {
                out.docs.insert(key, doc);
            }
            None => out.unread.push(normal(&format!("{under}/{path}"))),
        }
    }
    Ok(out)
}

/// The lines each file gains and loses in a `--unified=0` diff (path to added, removed; the hunk
/// headers count them, so no content line can be taken for a header), and its binary files.
fn line_counts(diff: &str) -> (HashMap<&str, (u64, u64)>, u32) {
    let (mut counts, mut binaries): (HashMap<&str, (u64, u64)>, u32) = (HashMap::new(), 0);
    let (mut file, mut header) = ("", false);
    for l in diff.lines() {
        if l.starts_with("diff --git ") {
            (file, header) = ("", true);
        } else if let Some(hunk) = l.strip_prefix("@@ ") {
            header = false;
            let side = |sign: char| -> u64 { hunk.split_whitespace().find_map(|w| w.strip_prefix(sign)).map_or(0, |w| w.split_once(',').map_or(1, |(_, n)| n.parse().unwrap_or(0))) };
            let row = counts.entry(file).or_default();
            row.0 += side('+');
            row.1 += side('-');
        } else if header {
            if let Some(p) = l.strip_prefix("--- a/").or_else(|| l.strip_prefix("+++ b/")) {
                file = p;
            } else if l.starts_with("Binary files ") {
                binaries += 1;
            }
        }
    }
    (counts, binaries)
}

/// `tree` with `changes` (paths below it) applied; None: nothing is left in it.
fn edit_tree(repo: &mut Repository, tree: Option<ObjectId>, changes: &[(&str, Option<Entry>)]) -> Result<Option<ObjectId>, GitError> {
    let mut entries: BTreeMap<String, TreeEntry> = match tree {
        Some(oid) => repo.read_tree(&oid)?.entries.into_iter().map(|e| (e.name.clone(), e)).collect(),
        None => BTreeMap::new(),
    };
    let mut below: BTreeMap<&str, Vec<(&str, Option<Entry>)>> = BTreeMap::new();
    for (path, to) in changes {
        match (path.split_once('/'), to) {
            (Some((name, rest)), _) => below.entry(name).or_default().push((rest, *to)),
            (None, Some((oid, mode))) => {
                entries.insert(path.to_string(), TreeEntry { mode: *mode, name: path.to_string(), oid: *oid });
            }
            // a file that became a directory: an earlier block may already have put the directory here
            (None, None) => {
                if entries.get(*path).is_some_and(|e| !e.is_tree()) {
                    entries.remove(*path);
                }
            }
        }
    }
    for (name, rest) in below {
        let sub = entries.get(name).filter(|e| e.is_tree()).map(|e| e.oid);
        // a directory that became a file in this or an earlier block: its files are gone with it
        if sub.is_none() && rest.iter().all(|(_, to)| to.is_none()) {
            continue;
        }
        match edit_tree(repo, sub, &rest)? {
            Some(oid) => {
                entries.insert(name.to_string(), TreeEntry { mode: 0o040000, name: name.to_string(), oid });
            }
            None => {
                entries.remove(name);
            }
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    repo.write_tree(&Tree { entries: entries.into_values().collect() }).map(Some)
}

/// The identity git itself would commit with (a checkout with no user.name gets the system's).
fn identity(dir: &Path) -> Result<Signature, String> {
    let ident = super::git(dir, &["var", "GIT_COMMITTER_IDENT"])?;
    parse_signature(&ident).map_err(|e| format!("git var GIT_COMMITTER_IDENT: {e}"))
}

fn own_repository(dir: &Path) -> Result<Repository, String> {
    let paths = crate::gitread::paths(dir).ok_or_else(|| format!("{}: no repository", dir.display()))?;
    crate::gitread::worktree(paths).map_err(|e| format!("{}: {e}", dir.display()))
}

fn oid(hex: &str) -> Result<ObjectId, String> {
    ObjectId::from_hex(hex).map_err(|e| format!("{hex}: {e}"))
}

impl Groups {
    /// The delta from commit `base` to commit `tip` (full ids) sorted into `tip`'s blocks.
    pub fn between(dir: &Path, base: &str, tip: &str) -> Result<Groups, String> {
        let mut repo = own_repository(dir)?;
        let (base_tree, tip_tree) = (
            repo.read_commit(&oid(base)?).map_err(|e| format!("commit {base}: {e}"))?.tree,
            repo.read_commit(&oid(tip)?).map_err(|e| format!("commit {tip}: {e}"))?.tree,
        );
        let top = repo.read_tree(&tip_tree).map_err(|e| format!("tree of {tip}: {e}"))?;
        let root_entry = top.entries.iter().find(|e| e.name == ROOT_MANIFEST).map(|e| (e.oid, e.mode));
        let root = root_entry.map_or(Ok(Vec::new()), |(o, _)| repo.read_blob(&o)).map_err(|e| format!("{ROOT_MANIFEST} of {tip}: {e}"))?;
        let root = String::from_utf8_lossy(&root).to_string();
        let listed = repo.entry_at(&tip_tree, BLOCKS_FILE).map_err(|e| format!("{BLOCKS_FILE}: {e}"))?.ok_or_else(|| format!("{tip} has no {BLOCKS_FILE}"))?;
        let blocks = parse_blocks(&String::from_utf8_lossy(&repo.read_blob(&listed).map_err(|e| format!("{BLOCKS_FILE}: {e}"))?));
        if blocks.is_empty() {
            return Err(format!("{BLOCKS_FILE} names no blocks"));
        }
        let diff = super::git(dir, &["diff", "--unified=0", "--no-color", "--no-ext-diff", "--no-renames", base, tip])?;
        let (counts, binaries) = line_counts(&diff);
        let mut stats = vec![Stat::default(); blocks.len()];
        let changes = repo.diff_trees(&base_tree, &tip_tree).map_err(|e| format!("diff of {base} and {tip}: {e}"))?;
        for change in changes {
            let (path, to, added) = match change {
                TreeChange::Added { path, oid, mode } => (path, Some((oid, mode)), true),
                TreeChange::Deleted { path, .. } => (path, None, false),
                TreeChange::Modified { path, new_oid, new_mode, .. } => (path, Some((new_oid, new_mode)), false),
            };
            let stat = &mut stats[block_of(&blocks, &path)];
            let (a, r) = counts.get(path.as_str()).copied().unwrap_or((0, 0));
            stat.added += a;
            stat.removed += r;
            stat.files.push(Change { path, to, added });
        }
        for stat in &mut stats {
            stat.files.sort_by(|a, b| a.path.cmp(&b.path));
        }
        let manifests = manifests_at(dir, tip, "")?;
        Ok(Groups { base: base.to_string(), tip: tip.to_string(), base_tree, tip_tree, blocks, stats, binaries, diff, root, root_entry, manifests, contributors: Vec::new() })
    }

    /// No file differs.
    pub fn is_empty(&self) -> bool {
        self.base_tree == self.tip_tree
    }

    /// The commits of the set.
    pub fn commits(&self) -> usize {
        self.stats.iter().filter(|s| !s.files.is_empty()).count()
    }

    /// The files of the delta.
    pub fn files(&self) -> usize {
        self.stats.iter().map(|s| s.files.len()).sum()
    }

    /// Block `at`'s top two path levels, most files first.
    pub fn areas(&self, at: usize) -> Vec<String> {
        let mut areas: BTreeMap<String, u32> = BTreeMap::new();
        for file in &self.stats[at].files {
            *areas.entry(file.path.split('/').take(2).collect::<Vec<_>>().join("/")).or_default() += 1;
        }
        let mut top: Vec<(String, u32)> = areas.into_iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        top.into_iter().map(|(area, _)| area).collect()
    }

    /// What block `at`'s commit says about itself, with `note` as a paragraph of its own.
    pub fn message(&self, at: usize, note: Option<&str>) -> String {
        let (block, stat, areas) = (&self.blocks[at], &self.stats[at], self.areas(at));
        let mut named = areas.iter().take(AREAS).map(String::as_str).collect::<Vec<_>>().join(", ");
        if areas.len() > AREAS {
            named.push_str(&format!(" and {} more", areas.len() - AREAS));
        }
        let note = note.map_or(String::new(), |n| format!("{n}\n\n"));
        let credits = self.contributors.get(at).filter(|c| !c.is_empty()).map_or(String::new(), |c| format!("{c}\n"));
        format!("{}\n\n{} files, +{} -{}. Areas: {named}.\n\n{note}{}\n{credits}", block.title, stat.files.len(), stat.added, stat.removed, super::push::TRAILER)
    }

    /// Credits the outside contributors of the PRs the set carries (`since..tip`; `since`: the work
    /// commit origin's tip was pushed from): [`contributor_lines`], for [`Self::message`]. The area
    /// commits are written under our identity, and an outside contributor's authorship was lost
    /// (PR #1390, 11-06).
    pub fn credit(&mut self, dir: &Path, since: &str) -> Result<(), String> {
        let own = identity(dir)?.email;
        let range = format!("{since}..{}", self.tip);
        let log = super::git(dir, &["-c", "core.quotePath=false", "log", "--no-merges", "--format=%x01%an%x02%ae%x02%s%x02%b%x03", "--name-only", &range])?;
        let mut commits: Vec<Landed> = Vec::new();
        for chunk in log.split('\u{1}').skip(1) {
            let (head, files) = chunk.split_once('\u{3}').unwrap_or((chunk, ""));
            let mut f = head.splitn(4, '\u{2}');
            let (name, email, subject, body) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
            commits.push(Landed { name: name.into(), email: email.into(), subject: subject.into(), body: body.into(), files: files.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect() });
        }
        // a merge of a PR: what it brought in over its first parent
        let merges = super::git(dir, &["log", "--merges", "--format=%H %P%x02%s", &range])?;
        for row in merges.lines() {
            let Some((ids, subject)) = row.split_once('\u{2}') else { continue };
            if merged_pr(subject).is_none() {
                continue;
            }
            let mut ids = ids.split_whitespace();
            let (Some(sha), Some(first)) = (ids.next(), ids.next()) else { continue };
            let files = super::git(dir, &["-c", "core.quotePath=false", "diff", "--name-only", "--no-renames", first, sha])?;
            commits.push(Landed { subject: subject.into(), files: files.lines().map(String::from).collect(), ..Landed::default() });
        }
        let held: Vec<bool> = self.stats.iter().map(|s| !s.files.is_empty()).collect();
        self.contributors = contributor_lines(&self.blocks, &held, &commits, &own);
        Ok(())
    }

    /// Writes the commits on the base, one per block with changes: (block, sha), in order. The last
    /// has the tip's tree and `extra_parents` too. `note(n, of)` adds to the message of commit `n`;
    /// `root_at(block)` replaces the root manifest of a commit but the last.
    pub fn build(&self, dir: &Path, extra_parents: &[String], note: &dyn Fn(usize, usize) -> Option<String>, root_at: &dyn Fn(usize) -> Option<String>) -> Result<Vec<(usize, String)>, String> {
        let at: Vec<usize> = (0..self.stats.len()).filter(|at| !self.stats[*at].files.is_empty()).collect();
        let Some(last) = at.last().copied() else { return Err("nothing differs".into()) };
        let extra = extra_parents.iter().map(|p| oid(p)).collect::<Result<Vec<_>, _>>()?;
        let who = identity(dir)?;
        let mut repo = own_repository(dir)?;
        let (mut tree, mut parent, mut out) = (self.base_tree, oid(&self.base)?, Vec::new());
        for (n, block) in at.iter().copied().enumerate() {
            let mut changes: Vec<(&str, Option<Entry>)> = self.stats[block].files.iter().map(|f| (f.path.as_str(), f.to)).collect();
            let generated = if block == last { None } else { root_at(block) };
            if block == last || generated.is_some() {
                changes.retain(|(path, _)| *path != ROOT_MANIFEST);
            }
            if let Some(text) = generated {
                let blob = repo.write_blob(text.as_bytes()).map_err(|e| format!("{ROOT_MANIFEST}: {e}"))?;
                changes.push((ROOT_MANIFEST, Some((blob, 0o100644))));
            } else if block == last {
                changes.push((ROOT_MANIFEST, self.root_entry));
            }
            tree = edit_tree(&mut repo, Some(tree), &changes).map_err(|e| format!("tree of `{}`: {e}", self.blocks[block].title))?.ok_or("a commit would have an empty tree")?;
            let mut parents = vec![parent];
            if block == last {
                parents.extend(extra.iter().copied());
            }
            let message = self.message(block, note(n + 1, at.len()).as_deref());
            let commit = Commit { tree, parents, author: who.clone(), committer: who.clone(), message };
            parent = repo.write_commit(&commit).map_err(|e| format!("commit of `{}`: {e}", self.blocks[block].title))?;
            out.push((block, parent.to_hex()));
        }
        if tree != self.tip_tree {
            return Err(format!("the last tree {} is not the tip's {}", tree.to_hex(), self.tip_tree.to_hex()));
        }
        Ok(out)
    }

    /// The crates the base doesn't have, flagged when only `private` manifests (a checkout outside
    /// the tree, its keys already under the tree's paths) end up using them.
    pub fn new_crates(&self, private: &BTreeMap<String, TomlDocument>) -> Vec<NewCrate> {
        let mut docs = self.manifests.docs.clone();
        docs.extend(private.iter().map(|(dir, doc)| (dir.clone(), doc.clone())));
        let all = crates_of(&docs);
        let mut users: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (dir, c) in &all {
            for dep in &c.deps {
                users.entry(dep.as_str()).or_default().insert(dir.as_str());
            }
        }
        let only = only_private(&all, &|dir| !self.manifests.docs.contains_key(dir));
        let added = self.stats.iter().flat_map(|s| &s.files).filter(|f| f.added).filter_map(|f| f.path.strip_suffix("/Cargo.toml"));
        let mut out: Vec<NewCrate> = added
            .filter_map(|dir| all.get(dir).map(|c| (dir, c)))
            .map(|(dir, c)| NewCrate {
                dir: dir.to_string(),
                name: c.name.clone(),
                users: users.get(dir).map_or(Vec::new(), |u| u.iter().map(|s| s.to_string()).collect()),
                only_private: only.contains(dir),
            })
            .collect();
        out.sort_by(|a, b| a.dir.cmp(&b.dir));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_across_directories() {
        assert!(matches("libs/render_*/*", "libs/render_lines/src/a.rs"));
        assert!(matches("apps/wm*", "apps/wm-all/Cargo.toml"));
        assert!(matches("*", "any/thing"));
        assert!(!matches("libs/xr_*/*", "libs/render/xr_a/b"));
    }

    #[test]
    fn a_path_takes_the_first_block_and_the_last_takes_the_rest() {
        let blocks = parse_blocks("# c\nblock One\nplatform/script/*\nblock Two\nplatform/*\nblock Rest\n*\n");
        assert_eq!(blocks.len(), 3);
        assert_eq!(block_of(&blocks, "platform/script/a.rs"), 0);
        assert_eq!(block_of(&blocks, "platform/a.rs"), 1);
        assert_eq!(block_of(&blocks, "README.md"), 2);
    }

    #[test]
    fn line_counts_read_the_hunk_headers_not_the_content() {
        // a deleted line that reads like a header, a new file, a deletion, a binary file
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1 @@\n--- a/other.rs\n-x\n+y\n\
                    diff --git a/n.rs b/n.rs\nnew file mode 100644\n--- /dev/null\n+++ b/n.rs\n@@ -0,0 +1,3 @@\n+1\n+2\n+3\n\
                    diff --git a/d.rs b/d.rs\ndeleted file mode 100644\n--- a/d.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-1\n\
                    diff --git a/p.png b/p.png\nBinary files a/p.png and b/p.png differ\n";
        let (counts, binaries) = line_counts(diff);
        assert_eq!(counts["a.rs"], (1, 2));
        assert_eq!(counts["n.rs"], (3, 0));
        assert_eq!(counts["d.rs"], (0, 1));
        assert_eq!(counts.len(), 3);
        assert_eq!(binaries, 1);
    }

    fn toml(text: &str) -> TomlDocument {
        parse_toml(text).unwrap()
    }

    /// A repository with `files` (path, mode, text) as the tree of a commit on `parent`.
    fn commit_of(repo: &mut Repository, parent: Option<ObjectId>, files: &[(&str, u32, &str)]) -> (ObjectId, ObjectId) {
        let changes: Vec<(&str, Option<Entry>)> = files.iter().map(|(p, m, t)| (*p, Some((repo.write_blob(t.as_bytes()).unwrap(), *m)))).collect();
        let tree = edit_tree(repo, None, &changes).unwrap().unwrap();
        let who = Signature { name: "t".into(), email: "t@t".into(), timestamp: 1, tz_offset: "+0000".into() };
        let commit = Commit { tree, parents: parent.into_iter().collect(), author: who.clone(), committer: who, message: "m\n".into() };
        (repo.write_commit(&commit).unwrap(), tree)
    }

    /// A PR is named by handle and number in each block holding a file it changed, once: from a
    /// merge subject, from an outside author's `(#N)` commit (a no-reply address gives the handle,
    /// the same name elsewhere shares it) and from a `Based on` port; our own identity never is,
    /// and no address is ever written; a PR whose change came and went is in the last block;
    /// consecutive numbers collapse to a range.
    #[test]
    fn contributors_are_named_by_handle_and_pr_in_the_blocks_holding_their_files() {
        let blocks = parse_blocks("block First\nlibs/a/*\nblock Second\nlibs/b/*\nblock Third\nlibs/c/*\nblock Rest\n*\n");
        let held = [true, true, false, true];
        let c = |name: &str, email: &str, subject: &str, body: &str, files: &[&str]| Landed { name: name.into(), email: email.into(), subject: subject.into(), body: body.into(), files: files.iter().map(|f| f.to_string()).collect() };
        let commits = vec![
            c("", "", "Merge PR #7 from ana-gh: A fix", "", &["libs/a/x.rs", "libs/b/y.rs"]),
            c("", "", "Merge pull request #8 from ana-gh/branch", "", &["libs/a/x.rs"]),
            c("", "", "Merge PR #9 from ana-gh: more", "", &["libs/a/w.rs"]),
            c("Bo Gone", "bo@example.net", "gone (#12)", "", &["libs/c/gone.rs"]),
            c("Cy", "1+cy-gh@users.noreply.github.com", "other (#20)", "", &["libs/b/q.rs"]),
            c("Cy", "cy@example.net", "again (#21)", "", &["libs/b/q.rs"]),
            c("Us", "US@example.com", "ours (#30)", "Based on makepad/relax#31 by dee-gh\n", &["libs/a/z.rs"]),
            c("Us", "US@example.com", "ours too (#32)", "", &["libs/a/z.rs"]),
        ];
        let lines = contributor_lines(&blocks, &held, &commits, "us@example.com");
        assert_eq!(lines[0], "Contributors: ana-gh (#7-#9), dee-gh (relax#31)", "first block");
        assert_eq!(lines[1], "Contributors: ana-gh (#7), cy-gh (#20, #21)", "second block");
        assert_eq!(lines[2], "", "a block without a commit");
        assert_eq!(lines[3], "Contributors: Bo Gone (#12)", "a change that came and went lands in the last block");
        assert!(lines.iter().all(|l| !l.contains('@') && !l.contains("Us") && !l.contains("#30") && !l.contains("#32")), "our identity and addresses never appear");
    }

    #[test]
    fn a_delta_becomes_one_commit_per_block_and_ends_on_the_tip_tree() {
        let dir = std::env::temp_dir().join(format!("devhub-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        super::super::git(&dir, &["init", "-q"]).unwrap();
        super::super::git(&dir, &["config", "user.name", "t"]).unwrap();
        super::super::git(&dir, &["config", "user.email", "t@t"]).unwrap();
        let mut repo = own_repository(&dir).unwrap();
        let blocks = "block First\nlibs/a/*\nswap/*\nblock Second\nlibs/new/*\nlibs/only/*\nlibs/behind/*\nswap2\nblock Rest\n*\n";
        let (x, f) = (0o100755, 0o100644);
        let (base, _) = commit_of(&mut repo, None, &[
            (BLOCKS_FILE, f, blocks),
            ("Cargo.toml", f, "[workspace]\nmembers = [\"libs/a\"]\n"),
            ("libs/a/Cargo.toml", f, "[package]\nname = \"a\"\n"),
            ("libs/a/src/lib.rs", f, "old\n"),
            ("gone/only.txt", f, "x\n"),
            ("run.sh", f, "#!/bin/sh\n"),
            ("swap", f, "a file that becomes a directory\n"),
            ("swap2/x.txt", f, "a directory that becomes a file\n"),
            ("swap2/y.txt", f, "a directory that becomes a file\n"),
        ]);
        let (tip, tip_tree) = commit_of(&mut repo, Some(base), &[
            (BLOCKS_FILE, f, blocks),
            ("Cargo.toml", f, "[workspace]\nmembers = [\"libs/a\", \"libs/new\"]\n"),
            ("libs/a/Cargo.toml", f, "[package]\nname = \"a\"\n[dependencies]\nnew = { path = \"../new\" }\n"),
            ("libs/a/src/lib.rs", f, "new\nnew2\n"),
            ("libs/new/Cargo.toml", f, "[package]\nname = \"new\"\n"),
            ("libs/only/Cargo.toml", f, "[package]\nname = \"only\"\n"),
            ("libs/behind/Cargo.toml", f, "[package]\nname = \"behind\"\n[dependencies]\nonly = { path = \"../only\" }\n"),
            ("libs/alone/Cargo.toml", f, "[package]\nname = \"alone\"\n"),
            ("run.sh", x, "#!/bin/sh\n"),
            ("swap/inner.txt", f, "now a directory\n"),
            ("swap2", f, "now a file\n"),
        ]);
        drop(repo);
        let groups = Groups::between(&dir, &base.to_hex(), &tip.to_hex()).unwrap();
        assert_eq!(groups.commits(), 3);
        assert_eq!((groups.stats[0].added, groups.stats[0].removed), (5, 1), "libs/a's manifest +2, its lib +2 -1, swap/inner.txt +1");
        let extra = base.to_hex();
        let set = groups.build(&dir, std::slice::from_ref(&extra), &|n, of| Some(format!("Part {n} of {of}.")), &|_| None).unwrap();
        assert_eq!(set.iter().map(|(at, _)| *at).collect::<Vec<_>>(), vec![0, 1, 2]);
        let mut repo = own_repository(&dir).unwrap();
        let mut parent = base;
        for (n, (at, sha)) in set.iter().enumerate() {
            let c = repo.read_commit(&oid(sha).unwrap()).unwrap();
            assert_eq!(c.parents.first().copied(), Some(parent), "commit {n} stands on the one before");
            assert_eq!(c.parents.len(), if n == 2 { 2 } else { 1 }, "extra parents go on the last");
            assert!(c.message.starts_with(&groups.blocks[*at].title));
            assert!(c.message.contains(&format!("Part {} of 3.", n + 1)) && c.message.contains("Areas: ") && c.message.contains(super::super::push::TRAILER));
            parent = oid(sha).unwrap();
        }
        let last = repo.read_commit(&parent).unwrap();
        assert_eq!(last.tree, tip_tree, "the last tree is the tip's");
        // the first commit holds its block's files and nothing else
        let first = repo.read_commit(&oid(&set[0].1).unwrap()).unwrap();
        assert!(repo.entry_at(&first.tree, "libs/new/Cargo.toml").unwrap().is_none());
        assert!(repo.entry_at(&first.tree, "swap/inner.txt").unwrap().is_some());
        assert!(repo.entry_at(&first.tree, "gone/only.txt").unwrap().is_some());

        // who uses what: a public user clears, a private one flags, also through a public crate only
        // private ones reach; a crate with no user is no flag
        let private: BTreeMap<String, TomlDocument> = [("apps/commercial/x".to_string(), toml("[package]\nname = \"x\"\n[dependencies]\nbehind = { path = \"../../../libs/behind\" }\nnew = { path = \"../../../libs/new\" }\n"))].into();
        let crates = groups.new_crates(&private);
        let row = |dir: &str| crates.iter().find(|c| c.dir == dir).unwrap();
        assert_eq!(crates.iter().map(|c| c.dir.as_str()).collect::<Vec<_>>(), vec!["libs/alone", "libs/behind", "libs/new", "libs/only"]);
        assert!(!row("libs/new").only_private);
        assert_eq!(row("libs/new").users, vec!["apps/commercial/x", "libs/a"]);
        assert!(row("libs/behind").only_private);
        assert_eq!(row("libs/behind").users, vec!["apps/commercial/x"]);
        assert!(row("libs/only").only_private, "its one user is a public crate only the private one uses");
        assert!(!row("libs/alone").only_private && row("libs/alone").users.is_empty());
        // without the private checkout's manifests nothing can be said to be only theirs
        assert!(groups.new_crates(&BTreeMap::new()).iter().all(|c| !c.only_private));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Non-test users decide: a public test-only user doesn't hide a crate only private crates use.
    #[test]
    fn only_non_test_public_users_clear_a_crate() {
        let package = |name: &str, section: &str, dep: &str| toml(&format!("[package]\nname = \"{name}\"\n[{section}]\n{dep} = {{ path = \"../{dep}\" }}\n"));
        let docs: BTreeMap<String, TomlDocument> = [
            ("libs/hidden".to_string(), toml("[package]\nname = \"hidden\"\n")),
            ("libs/kept".to_string(), toml("[package]\nname = \"kept\"\n")),
            ("libs/built".to_string(), toml("[package]\nname = \"built\"\n")),
            ("libs/testkit".to_string(), toml("[package]\nname = \"testkit\"\n")),
            ("libs/ptestkit".to_string(), toml("[package]\nname = \"ptestkit\"\n")),
            ("libs/tester".to_string(), package("tester", "dev-dependencies", "hidden")),
            ("libs/unix_tester".to_string(), toml("[package]\nname = \"unix_tester\"\n[target.'cfg(unix)'.dev-dependencies]\nhidden = { path = \"../hidden\" }\ntestkit = { path = \"../testkit\" }\n")),
            ("libs/user".to_string(), package("user", "dependencies", "kept")),
            ("libs/builder".to_string(), package("builder", "build-dependencies", "built")),
            ("apps/commercial/x".to_string(), toml("[package]\nname = \"x\"\n[dependencies]\nhidden = { path = \"../../../libs/hidden\" }\nkept = { path = \"../../../libs/kept\" }\nbuilt = { path = \"../../../libs/built\" }\n[dev-dependencies]\nptestkit = { path = \"../../../libs/ptestkit\" }\n")),
        ]
        .into();
        let all = crates_of(&docs);
        let only = only_private(&all, &|dir| dir.starts_with("apps/commercial/"));
        assert!(only.contains("libs/hidden"), "public users only in their tests: still only the private crate's");
        assert!(!only.contains("libs/kept"), "a public [dependencies] user clears it");
        assert!(!only.contains("libs/built"), "a public [build-dependencies] user clears it");
        assert!(!only.contains("libs/testkit"), "only public tests use it: no private crate does");
        assert!(only.contains("libs/ptestkit"), "only a private crate's tests use it");
        assert!(!only.contains("libs/tester") && !only.contains("apps/commercial/x"), "no users, or private itself: no flag");
    }

    #[test]
    fn a_workspace_dependency_resolves_in_the_nearest_workspace() {
        let docs: BTreeMap<String, TomlDocument> = [
            ("".to_string(), toml("[workspace]\n[workspace.dependencies]\nshared = { path = \"libs/shared\" }\n")),
            ("libs/user".to_string(), toml("[package]\nname = \"user\"\n[dependencies]\nshared.workspace = true\n")),
            ("apps/commercial/book".to_string(), toml("[workspace]\n[workspace.dependencies]\nshared = { path = \"../../../libs/other\" }\n")),
            ("apps/commercial/book/page".to_string(), toml("[package]\nname = \"page\"\n[target.'cfg(unix)'.dev-dependencies]\nshared = { workspace = true }\n")),
        ]
        .into();
        let crates = crates_of(&docs);
        assert_eq!(crates["libs/user"].deps.iter().collect::<Vec<_>>(), vec!["libs/shared"]);
        assert_eq!(crates["apps/commercial/book/page"].deps.iter().collect::<Vec<_>>(), vec!["libs/other"]);
        assert_eq!(crates.len(), 2, "a workspace manifest is no crate");
    }
}
