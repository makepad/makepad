//! `makepad devhub promote`: squash `work` onto `main` as one commit per logical block, then re-point
//! `work` at main's tip. `--dry` prints the block list and the scan and runs the private-address
//! check on the result; `--go` validates every commit, pushes main and moves work.
//!
//! Runs in the user's checkout and moves only refs there, so the working tree stays as it is: the
//! last commit's tree is work's own tree, and a promotion stops if it would not be. The old chain
//! stays local as `work-old-<date>`, and lanes off it carry over on their own
//! ([`crate::land::retired_base`]).
//!
//! Each commit but the last has a generated root manifest that lists only the crates whose own files
//! and path dependencies are already at work's state, so `cargo check --workspace` holds at each.

use super::{git, is_ancestor};
use makepad_toml_parser::{parse_toml, Toml, TomlTable};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BLOCKS_FILE: &str = "tools/devhub/promote_blocks.txt";
const EXCLUDE_FILE: &str = "tools/devhub/promote_exclude.txt";
const PREP_BRANCH: &str = "promote-prep";
const CLEAN_BUILD: &str = "tools/devhub/promote_clean_build.sh";
/// Paths that are never meant for main: agent state, editor and packaging leftovers.
const STRAY_PREFIXES: [&str; 2] = ["local/", ".claude/"];
const STRAY_NAMES: [&str; 2] = ["trial.out", ".DS_Store"];
const STRAY_SUFFIXES: [&str; 2] = [".orig", ".rej"];
const TRAILER: &str = super::push::TRAILER;
/// A changed file at least this big is named in the plan.
const LARGE: u64 = 256 * 1024;

pub struct Block {
    title: String,
    patterns: Vec<String>,
}

#[derive(Default, Clone)]
struct Stat {
    files: Vec<String>,
    added: u64,
    removed: u64,
}

struct Plan {
    repo: PathBuf,
    base: String,
    tip: String,
    tree: String,
    blocks: Vec<Block>,
    stats: Vec<Stat>,
    /// each crate of the tip: the block at whose commit it and its path dependencies are ready
    ready: BTreeMap<String, usize>,
    manifest: String,
    blockers: Vec<String>,
    notes: Vec<String>,
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

pub fn block_of(blocks: &[Block], path: &str) -> usize {
    blocks.iter().position(|b| b.patterns.iter().any(|p| matches(p, path))).unwrap_or(blocks.len() - 1)
}

/// The directories of a crate's path dependencies, as written in its manifest.
fn path_deps(dir: &str, doc: &TomlTable, workspace: &TomlTable) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut tables: Vec<&TomlTable> = Vec::new();
    let names = ["dependencies", "build-dependencies", "dev-dependencies"];
    tables.extend(names.iter().filter_map(|n| doc.get(*n).and_then(Toml::as_table)));
    if let Some(targets) = doc.get("target").and_then(Toml::as_table) {
        for target in targets.values().filter_map(Toml::as_table) {
            tables.extend(names.iter().filter_map(|n| target.get(*n).and_then(Toml::as_table)));
        }
    }
    for table in tables {
        for (name, dep) in table {
            let Some(dep) = dep.as_table() else { continue };
            if dep.get("workspace").and_then(Toml::as_bool) == Some(true) {
                if let Some(p) = workspace.get(name).and_then(Toml::as_table).and_then(|d| d.get("path")).and_then(Toml::as_str) {
                    out.insert(normal(p));
                }
            } else if let Some(p) = dep.get("path").and_then(Toml::as_str) {
                out.insert(normal(&format!("{dir}/{p}")));
            }
        }
    }
    out
}

/// `a/b/../c` as `a/c`.
fn normal(path: &str) -> String {
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

/// The root manifest at block `at`: members and patches limited to what is at work's state there.
pub fn manifest_at(manifest: &str, ready: &BTreeMap<String, usize>, at: usize) -> String {
    let is_ready = |dir: &str| ready.get(dir).is_some_and(|r| *r <= at);
    let (mut out, mut in_members, mut in_patch) = (String::new(), false, false);
    for line in manifest.lines() {
        let s = line.trim();
        if s.starts_with("workspace.members") {
            in_members = true;
            out.push_str(line);
            out.push('\n');
            // members a glob named at the tip, listed by hand while only some exist in this state
            for dir in ready.iter().filter(|(_, r)| **r <= at).map(|(d, _)| d) {
                let in_glob = dir.split('/').count() == 2 && dir.starts_with("apps/")
                    || dir.split('/').count() == 4 && dir.starts_with("libs/ai/models/");
                if in_glob {
                    out.push_str(&format!("    \"{dir}\",\n"));
                }
            }
            continue;
        }
        if in_members && s == "]" {
            in_members = false;
        } else if in_members {
            if let Some(member) = s.strip_prefix('"').and_then(|r| r.split('"').next()) {
                if !is_ready(member) {
                    continue;
                }
            }
        }
        if s.starts_with('[') {
            in_patch = s.starts_with("[patch.");
        }
        if in_patch {
            if let Some(path) = s.split("path").nth(1).and_then(|r| r.split('"').nth(1)) {
                if !is_ready(&normal(path)) {
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn plan(repo: &Path) -> Result<Plan, String> {
    if git(repo, &["branch", "--show-current"])? != "work" {
        return Err("the checkout is not on work".into());
    }
    git(repo, &["fetch", "-q", "origin", "main", "work"])?;
    let base = git(repo, &["rev-parse", "refs/remotes/origin/main"])?;
    let tip = git(repo, &["rev-parse", "refs/heads/work"])?;
    let tree = git(repo, &["rev-parse", &format!("{tip}^{{tree}}")])?;
    let mut blockers = Vec::new();
    if !is_ancestor(repo, &base, &tip) {
        blockers.push("origin/main has commits work doesn't: merge main into work first".to_string());
    }
    let blocks = parse_blocks(&git(repo, &["show", &format!("{tip}:{BLOCKS_FILE}")])?);
    if blocks.is_empty() {
        return Err(format!("{BLOCKS_FILE} names no blocks"));
    }
    let mut stats = vec![Stat::default(); blocks.len()];
    let mut notes = Vec::new();
    let numstat = git(repo, &["diff", "--numstat", "--no-renames", "--diff-filter=ACDMRT", &base, &tip])?;
    let mut binaries = 0;
    for row in numstat.lines() {
        let mut parts = row.splitn(3, '\t');
        let (a, d, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let stat = &mut stats[block_of(&blocks, path)];
        stat.files.push(path.to_string());
        stat.added += a.parse().unwrap_or(0);
        stat.removed += d.parse().unwrap_or(0);
        binaries += u32::from(a == "-");
    }
    notes.push(format!("{binaries} binary files changed"));

    // the tree itself: names that must not be there
    let listing = git(repo, &["ls-tree", "-r", "-l", &tree])?;
    let mut files: Vec<(u64, &str)> = Vec::new();
    for row in listing.lines() {
        let Some((head, path)) = row.split_once('\t') else { continue };
        files.push((head.split_whitespace().nth(3).and_then(|n| n.parse().ok()).unwrap_or(0), path));
    }
    let changed: BTreeSet<&str> = stats.iter().flat_map(|s| s.files.iter().map(String::as_str)).collect();
    let name = |p: &str| p.rsplit('/').next().unwrap_or("").to_string();
    for (_, path) in files.iter().filter(|(_, p)| changed.contains(p)) {
        let stray = STRAY_PREFIXES.iter().any(|s| path.starts_with(s))
            || STRAY_NAMES.contains(&name(path).as_str())
            || STRAY_SUFFIXES.iter().any(|s| path.ends_with(s));
        if stray {
            blockers.push(format!("stray file: {path}"));
        }
    }
    if let Ok(list) = git(repo, &["show", &format!("{tip}:{EXCLUDE_FILE}")]) {
        for excluded in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if files.iter().any(|(_, p)| *p == excluded) {
                blockers.push(format!("excluded file still in the tree: {excluded}"));
            }
        }
    }
    let mut largest: Vec<&(u64, &str)> = files.iter().filter(|(size, p)| *size >= LARGE && changed.contains(p)).collect();
    largest.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    for (size, path) in largest.iter().take(10) {
        notes.push(format!("large: {:.1} MB {path}", *size as f64 / 1_048_576.0));
    }

    // text that looks like a credential
    let diff = git(repo, &["diff", "--unified=0", "--no-color", "--no-ext-diff", "--no-renames", &base, &tip])?;
    let stem = git(repo, &["config", "user.email"]).ok().and_then(|e| super::push::email_stem(&e));
    for hit in super::push::scan(&diff, stem.as_ref()) {
        notes.push(format!("credential-like: {}:{} ({})", hit.file, hit.line, hit.what));
    }

    // which crates hold together at which block
    let manifest = git(repo, &["show", &format!("{tip}:Cargo.toml")])?;
    let root = parse_toml(&manifest).map_err(|e| format!("root manifest: {}", e.msg))?;
    let empty = TomlTable::new();
    let workspace = root.get_path(&["workspace", "dependencies"]).and_then(Toml::as_table).unwrap_or(&empty);
    let mut crates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, path) in files.iter().filter(|(_, p)| p.ends_with("/Cargo.toml")) {
        let Ok(text) = git(repo, &["show", &format!("{tip}:{path}")]) else { continue };
        let Ok(doc) = parse_toml(&text) else { continue };
        if doc.root.contains_key("package") {
            let dir = path.trim_end_matches("/Cargo.toml").to_string();
            crates.insert(dir.clone(), path_deps(&dir, &doc.root, workspace));
        }
    }
    let mut ready: BTreeMap<String, usize> = crates.keys().map(|d| (d.clone(), 0)).collect();
    for (at, stat) in stats.iter().enumerate() {
        for file in &stat.files {
            let mut dir = file.as_str();
            while let Some((up, _)) = dir.rsplit_once('/') {
                dir = up;
                if let Some(r) = ready.get_mut(dir) {
                    *r = (*r).max(at);
                    break;
                }
            }
        }
    }
    loop {
        let mut moved = false;
        for (dir, deps) in &crates {
            let needed = deps.iter().filter_map(|d| ready.get(d)).max().copied().unwrap_or(0);
            if needed > ready[dir] {
                ready.insert(dir.clone(), needed);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    Ok(Plan { repo: repo.to_path_buf(), base, tip, tree, blocks, stats, ready, manifest, blockers, notes })
}

/// `git` with an extra environment, feeding `input` when there is one.
#[track_caller]
fn git_env(repo: &Path, env: &[(&str, &str)], args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    use std::io::Write;
    let mut cmd = crate::git_exec::command();
    cmd.args(args).current_dir(repo).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("git: {e}"))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(bytes).map_err(|e| format!("git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn message(plan: &Plan, at: usize) -> String {
    let (block, stat) = (&plan.blocks[at], &plan.stats[at]);
    let mut areas: BTreeMap<String, u32> = BTreeMap::new();
    for file in &stat.files {
        *areas.entry(file.split('/').take(2).collect::<Vec<_>>().join("/")).or_default() += 1;
    }
    let mut top: Vec<(String, u32)> = areas.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let names: Vec<String> = top.into_iter().take(12).map(|(a, _)| a).collect();
    format!("{}\n\n{} files, +{} -{}. Areas: {}.\n\n{TRAILER}\n", block.title, stat.files.len(), stat.added, stat.removed, names.join(", "))
}

/// The commits on `plan.base`, one per block with changes: their shas, the last with work's tree.
fn build(plan: &Plan) -> Result<Vec<(usize, String)>, String> {
    let index = std::env::temp_dir().join(format!("makepad-promote-index-{}", std::process::id()));
    let index = index.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", index.as_str()), ("GIT_LITERAL_PATHSPECS", "1")];
    let repo = &plan.repo;
    git_env(repo, &env, &["read-tree", &plan.base], None)?;
    let last = plan.stats.iter().rposition(|s| !s.files.is_empty()).ok_or("nothing to promote")?;
    let (mut parent, mut out) = (plan.base.clone(), Vec::new());
    for at in (0..=last).filter(|at| !plan.stats[*at].files.is_empty()) {
        let list = std::env::temp_dir().join(format!("makepad-promote-paths-{}", std::process::id()));
        std::fs::write(&list, plan.stats[at].files.join("\n") + "\n").map_err(|e| format!("{e}"))?;
        let from = format!("--pathspec-from-file={}", list.to_string_lossy());
        let source = format!("--source={}", plan.tree);
        git_env(repo, &env, &["restore", &source, "--staged", &from], None)?;
        let _ = std::fs::remove_file(&list);
        if at != last {
            let text = manifest_at(&plan.manifest, &plan.ready, at);
            let blob = git_env(repo, &env, &["hash-object", "-w", "--stdin"], Some(text.as_bytes()))?;
            git_env(repo, &env, &["update-index", "--add", "--cacheinfo", &format!("100644,{blob},Cargo.toml")], None)?;
        } else {
            git_env(repo, &env, &["restore", &source, "--staged", "Cargo.toml"], None)?;
        }
        let tree = git_env(repo, &env, &["write-tree"], None)?;
        parent = git_env(repo, &[], &["commit-tree", &tree, "-p", &parent], Some(message(plan, at).as_bytes()))?;
        if at == last && tree != plan.tree {
            let _ = std::fs::remove_file(&index);
            return Err(format!("the last tree {tree:.10} is not work's {:.10}", plan.tree));
        }
        out.push((at, parent.clone()));
    }
    let _ = std::fs::remove_file(&index);
    git(repo, &["update-ref", &format!("refs/heads/{PREP_BRANCH}"), &parent])?;
    Ok(out)
}

fn print_plan(plan: &Plan, set: &[(usize, String)]) {
    println!("promote {:.10} (work) onto {:.10} (origin/main): {} commits", plan.tip, plan.base, set.len());
    for (n, (at, sha)) in set.iter().enumerate() {
        let stat = &plan.stats[*at];
        let ready = plan.ready.values().filter(|r| **r <= *at).count();
        println!("{:2} {:.10} {} [{} files, +{} -{}, {ready}/{} crates]", n + 1, sha, plan.blocks[*at].title, stat.files.len(), stat.added, stat.removed, plan.ready.len());
    }
    plan.notes.iter().for_each(|n| println!("note: {n}"));
    plan.blockers.iter().for_each(|b| println!("BLOCKER: {b}"));
}

/// This devhub's own words, run as a child: its stdout, or why it failed.
fn devhub(args: &[&str]) -> Result<String, String> {
    let own = crate::cli::own_argv(&crate::lease::wt::default_repo());
    let out = Command::new(&own[0]).args(&own[1..]).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("devhub {}: {}{}", args.first().unwrap_or(&""), text.trim(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Submits the jobs and waits for them: Err(the ones that didn't pass).
fn run_jobs(legs: &[(String, Vec<String>)]) -> Result<(), String> {
    let mut ids: Vec<(String, String)> = Vec::new();
    for (label, args) in legs {
        let mut submit = vec!["job", "submit", "--kind", "test", "--lane", "promote"];
        submit.extend(args.iter().map(String::as_str));
        let line = devhub(&submit)?;
        let id = line.split_whitespace().next().ok_or("job submit: no id")?.to_string();
        ids.push((id, label.clone()));
    }
    let mut wait = vec!["job", "wait"];
    wait.extend(ids.iter().map(|(id, _)| id.as_str()));
    let answer = devhub(&wait).unwrap_or_else(|e| e);
    let failed: Vec<String> = ids
        .iter()
        .filter(|(id, _)| !answer.lines().any(|l| l.starts_with(id.as_str()) && l.contains(" ok ")))
        .map(|(id, label)| format!("{label} ({id})"))
        .collect();
    if failed.is_empty() { Ok(()) } else { Err(format!("not passed: {}", failed.join(", "))) }
}

fn check_leg(label: String, needs: &str, rev: &str, cmd: &[&str]) -> (String, Vec<String>) {
    let mut args = vec!["--needs".to_string(), needs.to_string(), "--rev".to_string(), rev.to_string(), "--".to_string()];
    args.extend(cmd.iter().map(|s| s.to_string()));
    (label, args)
}

fn private_check(tip: &str) -> Result<(), String> {
    let rev = format!("{PREP_BRANCH}@{tip}");
    run_jobs(&[check_leg("private check".into(), "os=linux", &rev, &["cargo", "run", "-q", "--release", "-p", "makepad-private-check"])])
}

fn validate(set: &[(usize, String)], plan: &Plan) -> Result<(), String> {
    let mut legs = Vec::new();
    for (at, sha) in set {
        let rev = format!("{PREP_BRANCH}@{sha}");
        let cmd = ["cargo", "check", "--release", "--workspace"];
        legs.push(check_leg(format!("{} (linux)", plan.blocks[*at].title), "os=linux", &rev, &cmd));
        legs.push(check_leg(format!("{} (macos)", plan.blocks[*at].title), "os=macos", &rev, &cmd));
    }
    let tip = &set.last().ok_or("no commits")?.1;
    let rev = format!("{PREP_BRANCH}@{tip}");
    legs.push(check_leg("clean build".into(), "os=linux", &rev, &["bash", CLEAN_BUILD, tip]));
    run_jobs(&legs)
}

/// The cut: push main, keep the old chain, move work with the working tree as it is, push work.
fn cut(plan: &Plan, tip: &str) -> Result<(), String> {
    let repo = &plan.repo;
    devhub(&["land", "pause", "promotion to main"])?;
    let result = (|| {
        if git(repo, &["rev-parse", "refs/heads/work"])? != plan.tip {
            return Err("work moved while validating: run it again".to_string());
        }
        git(repo, &["fetch", "-q", "origin", "main", "work"])?;
        if git(repo, &["rev-parse", "refs/remotes/origin/main"])? != plan.base {
            return Err("origin/main moved while validating: run it again".to_string());
        }
        let before = git(repo, &["status", "--porcelain=v1", "--untracked-files=all"])?;
        let origin_work = git(repo, &["rev-parse", "refs/remotes/origin/work"])?;
        git_env(repo, &[("MAKEPAD_PROMOTE", "1")], &["push", "-q", "origin", &format!("{tip}:refs/heads/main")], None)?;
        let date = git_env(repo, &[], &["log", "-1", "--format=%cd", "--date=format:%Y%m%d", tip], None)?;
        let mut old = format!("refs/heads/work-old-{date}");
        for n in 2.. {
            if git(repo, &["rev-parse", "-q", "--verify", &old]).is_err() {
                break;
            }
            old = format!("refs/heads/work-old-{date}-{n}");
        }
        git(repo, &["update-ref", &old, &plan.tip])?;
        git(repo, &["update-ref", "refs/heads/work", tip, &plan.tip])?;
        if git(repo, &["status", "--porcelain=v1", "--untracked-files=all"])? != before {
            return Err("git status changed after moving work: look before anything else lands".to_string());
        }
        git_env(repo, &[], &["push", "-q", &format!("--force-with-lease=work:{origin_work}"), "origin", &format!("{tip}:refs/heads/work")], None)?;
        println!("main and work at {tip}; the old chain is {}", old.trim_start_matches("refs/heads/"));
        Ok(())
    })();
    if result.is_ok() {
        devhub(&["land", "resume"])?;
        let _ = devhub(&["verify", "kick"]);
    } else {
        println!("landing stays paused: `makepad devhub land resume` once it's understood");
    }
    result
}

pub fn main(words: &[&str]) -> i32 {
    let go = words.contains(&"--go");
    if !go && !words.contains(&"--dry") {
        eprintln!("makepad devhub promote --dry | --go   (work onto main as logical blocks, then work re-points at main)");
        return 2;
    }
    let run = || -> Result<(), String> {
        let repo = crate::lease::wt::default_repo();
        let plan = plan(&repo)?;
        let set = build(&plan)?;
        print_plan(&plan, &set);
        if !plan.blockers.is_empty() {
            return Err(format!("{} blockers: nothing moved", plan.blockers.len()));
        }
        let tip = set.last().ok_or("nothing to promote")?.1.clone();
        private_check(&tip)?;
        println!("private check: clean");
        if !go {
            println!("dry: {PREP_BRANCH} is at {tip}; nothing was pushed");
            return Ok(());
        }
        validate(&set, &plan)?;
        println!("every commit checks and the clean build has 0 warnings");
        cut(&plan, &tip)
    };
    match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("makepad devhub promote: {e}");
            1
        }
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
    fn the_manifest_lists_only_what_is_ready() {
        let text = "workspace.members = [\n    \"libs/a\",\n    \"libs/b\",\n    \"apps/*/src/..\",\n]\n[patch.crates-io]\nx = { path = \"libs/b\" }\ny = { path = \"libs/a\" }\n";
        let ready: BTreeMap<String, usize> = [("libs/a".to_string(), 0), ("libs/b".to_string(), 3), ("apps/c".to_string(), 0)].into_iter().collect();
        let at0 = manifest_at(text, &ready, 0);
        assert!(at0.contains("\"libs/a\"") && !at0.contains("\"libs/b\"") && at0.contains("\"apps/c\"") && !at0.contains("src/.."));
        assert!(at0.contains("y = ") && !at0.contains("x = "));
        assert!(manifest_at(text, &ready, 3).contains("\"libs/b\""));
    }
}
