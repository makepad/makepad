//! Reusable Codex worktrees, owned by the master, separate from the lane (11-15).

use super::Ctx;
use crate::lease::wt;
use crate::state::LaneRec;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{atomic::{AtomicU64, Ordering}, Mutex};

static SLOTS: Mutex<()> = Mutex::new(());
const COMMERCIAL: &str = "apps/commercial";
const TRAILERS: &str = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\nCo-Authored-By: Codex <267193182+codex@users.noreply.github.com>";
pub(crate) const USAGE: &str = "lane slot <lane> <n> [--job J] | lane slots <lane> | lane slot diff|take|drop <lane> <n> [--commit <msg>] (n = 1..4; --commit is for take)";

#[derive(Clone, Debug)]
struct Slot {
    lane: String,
    n: u8,
    path: PathBuf,
    base: [String; 2],
    job: String,
    made_ms: u64,
}

impl Slot {
    /// One slot's record, as slots.json and a `lane offers` row hold it ([`offered`]).
    fn parse(v: &Value) -> Result<Slot, String> {
        let lane = string(v, "lane")?;
        let n = number(&v.get("n").and_then(Value::as_u64).unwrap_or(0).to_string())?;
        let path = PathBuf::from(string(v, "path")?);
        let b = v.get("base").ok_or("slot: missing base")?;
        let base = [string(b, "makepad")?, string(b, "commercial")?];
        if !super::lane::valid_name(&lane) || !path.is_absolute() || base[0].is_empty() || base.iter().any(|s| !s.is_empty() && !(matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))) {
            return Err("slots.json: invalid or duplicate slot".into());
        }
        Ok(Slot { lane, n, path, base, job: string(v, "job")?, made_ms: v.get("made_ms").and_then(Value::as_u64).ok_or("slot: missing made_ms")? })
    }

    fn json(&self) -> String {
        format!("{{\"lane\":{},\"n\":{},\"path\":{},\"base\":{{\"makepad\":{},\"commercial\":{}}},\"job\":{},\"made_ms\":{}}}", json_str(&self.lane), self.n, json_str(&self.path.to_string_lossy()), json_str(&self.base[0]), json_str(&self.base[1]), json_str(&self.job), self.made_ms)
    }
}

struct Records {
    file: PathBuf,
    slots: Vec<Slot>,
}

fn string(v: &Value, key: &str) -> Result<String, String> {
    v.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("slot: missing {key}"))
}

fn number(n: &str) -> Result<u8, String> {
    match n { "1" => Ok(1), "2" => Ok(2), "3" => Ok(3), "4" => Ok(4), _ => Err(USAGE.into()) }
}

impl Records {
    fn load(home: &Path) -> Result<Self, String> {
        let file = home.join("slots.json");
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self { file, slots: Vec::new() }),
            Err(e) => return Err(format!("{}: {e}", file.display())),
        };
        let v = makepad_strict_json::parse(&bytes).map_err(|e| format!("{}: {e:?}", file.display()))?;
        let mut slots = Vec::new();
        for v in v.as_arr().ok_or("slots.json: expected an array")? {
            let s = Slot::parse(v)?;
            if slots.iter().any(|o: &Slot| o.lane == s.lane && o.n == s.n) {
                return Err("slots.json: invalid or duplicate slot".into());
            }
            slots.push(s);
        }
        Ok(Self { file, slots })
    }

    fn save(&self) -> Result<(), String> {
        let dir = self.file.parent().ok_or("slots.json: no parent")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = self.file.with_extension("json.tmp");
        std::fs::write(&tmp, format!("[{}]\n", self.slots.iter().map(Slot::json).collect::<Vec<_>>().join(","))).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| format!("{}: {e}", self.file.display()))
    }
}

/// Everything an operation needs, including when no master is running.
struct Lane {
    name: String,
    repo: PathBuf,
    tree: PathBuf,
    root: PathBuf,
    branch: String,
    cbranch: String,
}

impl Lane {
    fn new(repo: &Path, l: &LaneRec) -> Self {
        Self { name: l.name.clone(), repo: repo.to_path_buf(), tree: PathBuf::from(&l.path), root: wt::root_of(repo), branch: l.branch.clone(), cbranch: l.cbranch.clone() }
    }

    fn dir(&self) -> PathBuf { self.root.join(format!("{}.slots", self.name)) }
    fn saved(&self) -> PathBuf { self.repo.join("local/agent_state").join(&self.name) }
    fn heads(&self) -> Result<[String; 2], String> {
        Ok([head(&self.tree)?, if self.cbranch.is_empty() { String::new() } else { head(&self.tree.join(COMMERCIAL))? }])
    }
}

fn head(tree: &Path) -> Result<String, String> {
    wt::git(Some(tree), None, &["rev-parse", "--verify", "HEAD"]).map(|s| s.trim().to_string())
}

fn half(path: &Path, n: usize) -> PathBuf { if n == 0 { path.to_path_buf() } else { path.join(COMMERCIAL) } }
fn shown(n: usize, file: &str) -> String { if n == 0 { file.into() } else { format!("{COMMERCIAL}/{file}") } }

/// Check every half before any index or worktree write (11-15).
fn verify(l: &Lane, s: &Slot) -> Result<(), String> {
    verify_in(&l.dir(), s)
}

/// [`verify`] against its lane's slots dir `dir` ([`Lane::dir`]).
fn verify_in(dir: &Path, s: &Slot) -> Result<(), String> {
    let fail = |i, why: String| format!("slot {} of {}: {} half: {why}", s.n, s.lane, if i == 0 { "makepad" } else { "commercial" });
    let expected = dir.join(s.n.to_string());
    let root = s.path.canonicalize().map_err(|e| fail(0, format!("{}: {e}", s.path.display())))?;
    let expected = expected.canonicalize().map_err(|e| fail(0, format!("{}: {e}", expected.display())))?;
    if root != expected { return Err(fail(0, format!("record names {}, expected {}", root.display(), expected.display()))) }
    for i in 0..2 {
        if s.base[i].is_empty() { continue }
        let tree = half(&s.path, i);
        let meta = std::fs::symlink_metadata(&tree).map_err(|e| fail(i, format!("{}: {e}", tree.display())))?;
        if !meta.is_dir() { return Err(fail(i, format!("{} is {}", tree.display(), if meta.file_type().is_symlink() { "a symlink" } else { "not a directory" }))) }
        let canonical = tree.canonicalize().map_err(|e| fail(i, format!("{}: {e}", tree.display())))?;
        if canonical != half(&root, i) { return Err(fail(i, format!("{} resolves outside its half to {}", tree.display(), canonical.display()))) }
        let top = wt::git(Some(&tree), None, &["rev-parse", "--show-toplevel"]).map_err(|e| fail(i, e))?;
        let top = Path::new(top.trim_end_matches('\n')).canonicalize().map_err(|e| fail(i, format!("git toplevel {top:?}: {e}")))?;
        if top != canonical { return Err(fail(i, format!("git toplevel is {}, expected {}", top.display(), canonical.display()))) }
    }
    Ok(())
}

/// A slot whose folder is gone holds nothing to lose: removal forgets it, a listing says so,
/// and nothing else runs on it (verify refuses) (11-15).
fn gone(s: &Slot) -> bool {
    std::fs::symlink_metadata(&s.path).is_err()
}

fn busy_reason(s: &Slot, cwds: &[(u32, PathBuf)]) -> Result<Option<String>, String> {
    let path = s.path.canonicalize().map_err(|e| format!("slot {} of {}: {e}", s.n, s.lane))?;
    let live = wt::live_in(cwds, &path);
    if live.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "slot {} of {}: pid {} runs in it; wait for it to end",
        s.n, s.lane, live.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
    )))
}

fn idle(s: &Slot, cwds: &[(u32, PathBuf)]) -> Result<(), String> {
    match busy_reason(s, cwds)? {
        None => Ok(()),
        Some(why) => Err(why),
    }
}

/// The bot only try-locks, so outer then commercial cannot deadlock (11-15).
fn lane_write<T>(l: &Lane, f: impl FnOnce() -> T) -> T {
    crate::sync::lanes::lane_write(&l.tree, || {
        if l.cbranch.is_empty() { f() } else { crate::sync::lanes::lane_write(&l.tree.join(COMMERCIAL), f) }
    })
}

fn names(files: &[String]) -> String {
    let more = if files.len() > 8 { format!(" and {} more", files.len() - 8) } else { String::new() };
    format!("{}{more}", files.iter().take(8).map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", "))
}

/// Porcelain's NUL paths preserve spaces, newlines and both sides of a rename.
fn dirty(tree: &Path, outer: bool) -> Result<Vec<String>, String> {
    let status = wt::git(Some(tree), None, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignore-submodules=all", "--no-renames"])?;
    let mut files: Vec<String> = status.split('\0').filter(|s| !s.is_empty()).map(|s| s.get(3..).ok_or("slot: invalid git status").map(str::to_string)).collect::<Result<_, _>>()?;
    if outer { files.retain(|p| p != COMMERCIAL && !p.starts_with("apps/commercial/")); }
    files.sort();
    files.dedup();
    Ok(files)
}

fn changed(s: &Slot) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for i in 0..2 {
        let tree = half(&s.path, i);
        if s.base[i].is_empty() { continue }
        files.extend(dirty(&tree, i == 0)?.iter().map(|p| shown(i, p)));
        let committed = wt::git(Some(&tree), None, &["diff", "--name-only", "--no-renames", "-z", &s.base[i], "HEAD", "--"])?;
        files.extend(committed.split('\0').filter(|p| !p.is_empty()).map(|p| shown(i, p)));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// A working-file patch cannot carry an index-only version (11-15).
fn staged_files(s: &Slot) -> Result<(), String> {
    let mut files = Vec::new();
    for i in 0..2 {
        if s.base[i].is_empty() { continue }
        let tree = half(&s.path, i);
        let unstaged = wt::git(Some(&tree), None, &["diff", "--name-only", "-z"])?;
        let staged = wt::git(Some(&tree), None, &["diff", "--cached", "--name-only", "-z", "HEAD"])?;
        let unstaged: BTreeSet<&str> = unstaged.split('\0').filter(|p| !p.is_empty()).collect();
        files.extend(staged.split('\0').filter(|p| !p.is_empty() && unstaged.contains(p)).map(|p| shown(i, p)));
    }
    if files.is_empty() { return Ok(()) }
    Err(format!("slot {} of {}: {} are staged with content their files no longer have; stage or restore them, then take or drop", s.n, s.lane, names(&files)))
}

fn dirty_reason(s: &Slot) -> Result<Option<String>, String> {
    let files = changed(s)?;
    if files.is_empty() {
        return Ok(None);
    }
    let job = s.job.trim();
    let from = if job.is_empty() { String::new() } else { format!(" from job {job}") };
    Ok(Some(format!(
        "slot {} of {} holds changes{from}: {}; take them (`devhub lane slot take {} {}`) or drop them (`devhub lane slot drop {} {}`)",
        s.n, s.lane, names(&files), s.lane, s.n, s.lane, s.n,
    )))
}

fn clean(s: &Slot) -> Result<(), String> {
    match dirty_reason(s)? {
        None => Ok(()),
        Some(why) => Err(why),
    }
}

fn reset(s: &mut Slot, base: [String; 2], discard: bool) -> Result<(), String> {
    for (i, sha) in base.iter().enumerate().filter(|(_, sha)| !sha.is_empty()) {
        let tree = half(&s.path, i);
        if discard {
            wt::git(Some(&tree), None, &["reset", "-q", "HEAD", "--", "."])?;
            wt::git(Some(&tree), None, &["checkout", "--", "."])?;
            wt::git(Some(&tree), None, &["clean", "-fd"])?;
        }
        wt::git(Some(&tree), None, &["checkout", "-q", "--detach", sha])?;
    }
    s.base = base;
    s.job.clear();
    Ok(())
}

fn make(l: &Lane, records: &mut Records, n: u8, job: &str, cwds: &[(u32, PathBuf)]) -> Result<Slot, String> {
    lane_write(l, || make_locked(l, records, n, job, cwds))
}

fn make_locked(l: &Lane, records: &mut Records, n: u8, job: &str, cwds: &[(u32, PathBuf)]) -> Result<Slot, String> {
    let base = l.heads()?;
    if let Some(s) = records.slots.iter_mut().find(|s| s.lane == l.name && s.n == n) {
        verify(l, s)?;
        idle(s, cwds)?;
        clean(s)?;
        reset(s, base, false)?;
        s.job = job.into();
        let out = s.clone();
        records.save()?;
        return Ok(out);
    }
    let path = l.dir().join(n.to_string());
    if path.exists() { return Err(format!("{} exists without a slot record; kept", path.display())) }
    std::fs::create_dir_all(l.dir()).map_err(|e| e.to_string())?;
    wt::git(Some(&l.repo), None, &["worktree", "add", "-q", "--detach", &path.to_string_lossy(), &base[0]])?;
    if !base[1].is_empty() {
        if let Err(e) = wt::git(Some(&l.repo.join(COMMERCIAL)), None, &["worktree", "add", "-q", "--detach", &path.join(COMMERCIAL).to_string_lossy(), &base[1]]) {
            let undo = wt::git(Some(&l.repo), None, &["worktree", "remove", "--force", &path.to_string_lossy()]);
            return Err(match undo { Ok(_) => e, Err(u) => format!("{e}; undo: {u}") });
        }
    }
    let s = Slot { lane: l.name.clone(), n, path, base, job: job.into(), made_ms: crate::proto::wall_ms() };
    records.slots.push(s.clone());
    if let Err(e) = records.save() {
        let undo = remove_tree(l, &s, false);
        records.slots.pop();
        return Err(match undo { Ok(()) => e, Err(u) => format!("{e}; undo: {u}") });
    }
    Ok(s)
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!("devhub-slot-{}-{}-{}", std::process::id(), crate::proto::wall_ms(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// Plumbing with a private index; never changes the slot's real index.
fn git(tree: &Path, index: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut c = crate::git_exec::command();
    c.current_dir(tree).args(args).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    if let Some(index) = index { c.env("GIT_INDEX_FILE", index); }
    let out = c.output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() { return Err(format!("git {}: {}{}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))) }
    String::from_utf8(out.stdout).map_err(|_| "slot: git returned a non-UTF-8 path or patch".into())
}

struct Patch {
    text: String,
    files: Vec<String>,
}

fn diff(l: &Lane, s: &Slot) -> Result<Vec<Patch>, String> {
    verify(l, s)?;
    let tmp = Temp::new()?;
    let mut patches = Vec::new();
    for i in 0..2 {
        if s.base[i].is_empty() { continue }
        let tree = half(&s.path, i);
        let index = tmp.0.join(format!("index-{i}"));
        git(&tree, Some(&index), &["read-tree", "HEAD"])?;
        git(&tree, Some(&index), &["add", "-u", "--", "."])?;
        // untracked files, never the nested commercial checkout: `add -A` with an exclude of an
        // ignored path is an error to git ("paths are ignored"), so they are listed and added
        let others = git(&tree, Some(&index), &["ls-files", "--others", "--exclude-standard", "-z"])?;
        let new: Vec<String> = others.split('\0').filter(|p| !p.is_empty() && (i == 1 || !(*p == COMMERCIAL || p.starts_with("apps/commercial/")))).map(str::to_string).collect();
        if !new.is_empty() {
            let paths = paths_file(&tmp.0, i, &new)?;
            git(&tree, Some(&index), &["--literal-pathspecs", "add", &paths, "--pathspec-file-nul"])?;
        }
        let prefix = if i == 0 { "" } else { "apps/commercial/" };
        let text = git(&tree, Some(&index), &["diff", "--cached", "--binary", "--full-index", "--no-color", "--no-renames", "--no-ext-diff", "--no-textconv", &format!("--src-prefix=a/{prefix}"), &format!("--dst-prefix=b/{prefix}"), &s.base[i], "--"])?;
        let files = git(&tree, Some(&index), &["diff", "--cached", "--name-only", "--no-renames", "-z", &s.base[i], "--"])?;
        patches.push(Patch { text, files: files.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect() });
    }
    Ok(patches)
}

fn patch_file(dir: &Path, i: usize, p: &Patch) -> Result<PathBuf, String> {
    let file = dir.join(format!("{i}.diff"));
    std::fs::write(&file, &p.text).map_err(|e| e.to_string())?;
    Ok(file)
}

fn paths_file(dir: &Path, i: usize, files: &[String]) -> Result<String, String> {
    let file = dir.join(format!("{i}.paths"));
    let mut bytes = files.join("\0").into_bytes();
    bytes.push(0);
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    Ok(format!("--pathspec-from-file={}", file.display()))
}

fn applied_files(tree: &Path, head: &str, files: &[String]) -> Result<Vec<String>, String> {
    let staged = git(tree, None, &["diff", "--cached", "--name-only", "--no-renames", "-z", head, "--"])?;
    Ok(staged.split('\0').filter(|p| !p.is_empty() && files.iter().any(|f| f.as_str() == *p)).map(str::to_string).collect::<BTreeSet<_>>().into_iter().collect())
}

/// A conflict, including in the second half, touches neither real index nor worktree.
fn take(l: &Lane, s: &mut Slot, commit: Option<&str>, cwds: &[(u32, PathBuf)]) -> Result<(), String> {
    lane_write(l, || take_locked(l, s, commit, cwds, apply))
}

fn apply(i: usize, tree: &Path, file: &Path) -> Result<(), String> {
    wt::git(Some(tree), None, &["apply", "--3way", if i == 0 { "-p1" } else { "-p3" }, &file.to_string_lossy()]).map(|_| ())
}

fn take_locked(l: &Lane, s: &mut Slot, commit: Option<&str>, cwds: &[(u32, PathBuf)], mut apply: impl FnMut(usize, &Path, &Path) -> Result<(), String>) -> Result<(), String> {
    verify(l, s)?;
    idle(s, cwds)?;
    staged_files(s)?;
    let patches = diff(l, s)?;
    let heads = l.heads()?;
    let tmp = Temp::new()?;
    let mut conflicts = Vec::new();
    for (i, p) in patches.iter().enumerate().filter(|(_, p)| !p.text.is_empty()) {
        let tree = half(&l.tree, i);
        let dirty = dirty(&tree, i == 0)?;
        let overlap: Vec<String> = dirty.iter().filter(|d| p.files.iter().any(|p| Path::new(d).starts_with(p) || Path::new(p).starts_with(d))).map(|p| shown(i, p)).collect();
        if !overlap.is_empty() { return Err(format!("lane {} has uncommitted changes to slot files: {}", l.name, names(&overlap))) }
        let index = tmp.0.join(format!("index-{i}"));
        git(&tree, Some(&index), &["read-tree", &heads[i]])?;
        let file = patch_file(&tmp.0, i, p)?;
        let strip = if i == 0 { "-p1" } else { "-p3" };
        let check = git(&tree, Some(&index), &["apply", "--cached", "--3way", "--check", strip, &file.to_string_lossy()]);
        // --cached also computes the merge in the disposable index; --check alone need not
        // populate its unmerged entries on every git version.
        let merge = git(&tree, Some(&index), &["apply", "--cached", "--3way", strip, &file.to_string_lossy()]);
        if check.is_err() || merge.is_err() {
            let unmerged = git(&tree, Some(&index), &["diff", "--name-only", "--diff-filter=U", "-z"])?;
            let files: Vec<String> = unmerged.split('\0').filter(|p| !p.is_empty()).map(|p| shown(i, p)).collect();
            let files = if files.is_empty() { p.files.iter().map(|p| shown(i, p)).collect() } else { files };
            conflicts.push(format!("{}: {}", names(&files), merge.err().or_else(|| check.err()).unwrap_or_default()));
        }
    }
    if !conflicts.is_empty() { return Err(format!("slot take conflicts (nothing applied): {}", conflicts.join("; "))) }
    let mut applied = Vec::new();
    for (i, _) in patches.iter().enumerate().filter(|(_, p)| !p.text.is_empty()) {
        let tree = half(&l.tree, i);
        let file = tmp.0.join(format!("{i}.diff"));
        let result = apply(i, &tree, &file);
        if let Err(e) = result {
            let mut failures = Vec::new();
            for &j in applied.iter().rev() {
                let tree = half(&l.tree, j);
                let file = tmp.0.join(format!("{j}.diff"));
                // Reverse only what this take applied; HEAD may have moved meanwhile (11-15).
                if let Err(e) = wt::git(Some(&tree), None, &["apply", "-R", "--index", if j == 0 { "-p1" } else { "-p3" }, &file.to_string_lossy()]) {
                    failures.push(format!("reverse apply failed in lane {} half {}: {e}; that lane half and slot {} of {} both hold the change", l.name, tree.display(), s.n, s.lane));
                }
            }
            let undo = if failures.is_empty() { "applied halves rolled back".into() } else { format!("rollback: {}", failures.join("; ")) };
            return Err(format!("{e}; {undo}; check {} for a partial apply; slot {} of {} kept", tree.display(), s.n, s.lane));
        }
        applied.push(i);
    }
    if let Some(msg) = commit {
        let msg = format!("{}\n\n{TRAILERS}", msg.trim_end());
        let mut committed: Vec<(usize, String)> = Vec::new();
        for &i in &applied {
            let tree = half(&l.tree, i);
            let commit = || -> Result<Option<String>, String> {
                let files = applied_files(&tree, &heads[i], &patches[i].files)?;
                if files.is_empty() { return Ok(None) }
                let paths = paths_file(&tmp.0, i, &files)?;
                // a plain commit, not wt::git (which turns hooks off): the repository's own hooks
                // run as for any commit, its private-check scan among them; never --no-verify.
                // --only keeps unrelated staged files out
                git(&tree, None, &["--literal-pathspecs", "commit", "--only", "-m", &msg, &paths, "--pathspec-file-nul"])?;
                head(&tree).map(Some)
            };
            match commit() {
                Ok(Some(after)) => committed.push((i, after)),
                Ok(None) => {}
                Err(e) => {
                    let mut failures = Vec::new();
                    for (j, after) in committed.iter().rev() {
                        if let Err(e) = wt::git(Some(&half(&l.tree, *j)), None, &["update-ref", "HEAD", &heads[*j], after]) { failures.push(e); }
                    }
                    return Err(if failures.is_empty() { e } else { format!("{e}; undo commits: {}", failures.join("; ")) });
                }
            }
        }
    }
    reset(s, l.heads()?, true)
}

fn save_diff(l: &Lane, s: &Slot) -> Result<PathBuf, String> {
    use std::io::Write;
    let text: String = diff(l, s)?.into_iter().map(|p| p.text).collect();
    std::fs::create_dir_all(l.saved()).map_err(|e| e.to_string())?;
    let mut ms = crate::proto::wall_ms();
    loop {
        let path = l.saved().join(format!("slot-{}-{ms}.diff", s.n));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => { f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?; return Ok(path) }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn drop_slot(l: &Lane, s: &mut Slot, cwds: &[(u32, PathBuf)]) -> Result<PathBuf, String> {
    verify(l, s)?;
    idle(s, cwds)?;
    staged_files(s)?;
    let saved = save_diff(l, s)?;
    reset(s, l.heads()?, true)?;
    Ok(saved)
}

fn remove_tree(l: &Lane, s: &Slot, force: bool) -> Result<(), String> {
    verify(l, s)?;
    for i in (0..2).rev() {
        let path = half(&s.path, i);
        if s.base[i].is_empty() || !path.exists() { continue }
        let p = path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force { args.push("--force"); }
        args.push(&p);
        wt::git(Some(&half(&l.repo, i)), None, &args)?;
    }
    Ok(())
}

/// A combined patch separates at git headers, including git's quoted path spelling.
fn split_diff(text: &str) -> [String; 2] {
    let mut parts = [String::new(), String::new()];
    let mut i = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            i = usize::from(line.starts_with("diff --git a/apps/commercial/") || line.starts_with("diff --git \"a/apps/commercial/"));
        }
        parts[i].push_str(line);
    }
    parts
}

fn prune_saved(l: &Lane) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(l.saved()) { Ok(e) => e, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()), Err(e) => return Err(e.to_string()) };
    let tmp = Temp::new()?;
    let mut kept = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("slot-") || !name.ends_with(".diff") || !entry.file_type().map_err(|e| e.to_string())?.is_file() { continue }
        let parts = split_diff(&std::fs::read_to_string(entry.path()).map_err(|e| e.to_string())?);
        let mut missing = Vec::new();
        for (i, part) in parts.iter().enumerate().filter(|(_, p)| !p.is_empty()) {
            let branch = if i == 0 { &l.branch } else { &l.cbranch };
            let tree = half(&l.repo, i);
            let index = tmp.0.join(format!("check-{i}"));
            let patch = tmp.0.join(format!("saved-{i}.diff"));
            std::fs::write(&patch, part).map_err(|e| e.to_string())?;
            let check = git(&tree, Some(&index), &["read-tree", &format!("refs/heads/{branch}")]).and_then(|_| git(&tree, Some(&index), &["apply", "--cached", "--reverse", "--check", if i == 0 { "-p1" } else { "-p3" }, &patch.to_string_lossy()]));
            if check.is_err() { missing.push(if branch.is_empty() { "(no commercial branch)".into() } else { branch.clone() }); }
        }
        if missing.is_empty() {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        } else {
            missing.dedup();
            kept.push(format!("kept {}: its change is not in {}", entry.path().display(), missing.join(", ")));
        }
    }
    kept.sort();
    Ok(kept)
}

/// Ignored local work belongs to the job; only the spool's own folder is disposable (11-15).
fn ignored_work(s: &Slot) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for i in 0..2 {
        if s.base[i].is_empty() { continue }
        let ignored = wt::git(Some(&half(&s.path, i)), None, &["ls-files", "--others", "--ignored", "--exclude-standard", "-z", "--", "local"])?;
        files.extend(ignored.split('\0').filter(|p| !p.is_empty() && !p.starts_with("local/devhub-spool/")).map(|p| shown(i, p)));
    }
    Ok(files)
}

fn save_local(l: &Lane, s: &Slot, files: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() { return Ok(Vec::new()) }
    std::fs::create_dir_all(l.saved()).map_err(|e| e.to_string())?;
    let mut ms = crate::proto::wall_ms();
    let dir = loop {
        let dir = l.saved().join(format!("slot-{}-local-{ms}", s.n));
        match std::fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
            Err(e) => return Err(e.to_string()),
        }
    };
    let mut notes = vec![format!("saved slot {} of {} local work in {}", s.n, s.lane, dir.display())];
    for file in files {
        let src = s.path.join(file);
        let meta = std::fs::symlink_metadata(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        if !meta.is_file() {
            notes.push(format!("skipped {}: {} (not followed)", src.display(), if meta.file_type().is_symlink() { "symlink" } else { "not a regular file" }));
            continue;
        }
        let dst = dir.join(file);
        std::fs::create_dir_all(dst.parent().ok_or("slot local file has no parent")?).map_err(|e| e.to_string())?;
        std::fs::copy(&src, &dst).map_err(|e| format!("{} -> {}: {e}", src.display(), dst.display()))?;
    }
    Ok(notes)
}

fn ignored_work_reason(s: &Slot, files: &[String]) -> String {
    format!("slot {} of {} holds ignored local work: {}; move or delete them first", s.n, s.lane, names(files))
}

fn remove_slots(l: &Lane, records: &mut Records, force: bool, cwds: &[(u32, PathBuf)]) -> Result<Vec<String>, String> {
    let slots: Vec<Slot> = records.slots.iter().filter(|s| s.lane == l.name).cloned().collect();
    // Check every slot before removing any of them.
    let mut locals = Vec::new();
    for s in &slots {
        if gone(s) { locals.push(Vec::new()); continue }
        verify(l, s)?;
        idle(s, cwds)?;
        let files = ignored_work(s)?;
        if !force {
            clean(s)?;
            if !files.is_empty() { return Err(ignored_work_reason(s, &files)) }
        }
        locals.push(files);
    }
    let mut notes = Vec::new();
    for (s, files) in slots.into_iter().zip(locals) {
        if gone(&s) {
            // nothing left to save or check: its record goes, and git forgets the worktree
            for i in 0..2 { if !s.base[i].is_empty() { wt::git(Some(&half(&l.repo, i)), None, &["worktree", "prune"])?; } }
            records.slots.retain(|r| r.lane != s.lane || r.n != s.n);
            records.save()?;
            notes.push(format!("slot {} of {}: its folder {} was gone; its record removed", s.n, s.lane, s.path.display()));
            continue;
        }
        if force {
            save_diff(l, &s)?;
            notes.extend(save_local(l, &s, &files)?);
        }
        remove_tree(l, &s, force)?;
        records.slots.retain(|r| r.lane != s.lane || r.n != s.n);
        records.save()?;
        notes.push(format!("removed slot {} of {}", s.n, s.lane));
    }
    if l.dir().exists() { std::fs::remove_dir(l.dir()).map_err(|e| format!("{}: {e}", l.dir().display()))?; }
    notes.extend(prune_saved(l)?);
    Ok(notes)
}

pub(crate) fn remove(ctx: &Ctx, l: &LaneRec, force: bool) -> Result<Vec<String>, String> {
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    remove_slots(&Lane::new(&ctx.repo, l), &mut Records::load(&ctx.home)?, force, &wt::live_cwds())
}

pub(crate) fn check_clean(ctx: &Ctx, l: &LaneRec) -> Result<(), String> {
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    for s in Records::load(&ctx.home)?.slots.iter().filter(|s| s.lane == l.name && !gone(s)) { verify(&Lane::new(&ctx.repo, l), s)?; clean(s)?; }
    Ok(())
}

/// Result of checking a lane's slots before automatic landed cleanup removes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CleanupCheck {
    /// Every slot is clean and idle (or its folder is already gone); removal may proceed.
    Clean,
    /// A slot holds owner work (tracked/untracked/committed changes or ignored `local/` work)
    /// waiting for the owner to take, drop, move or delete it.
    OwnerWork(String),
    /// A process currently runs inside a slot; wait for it to end (no owner note).
    Busy(String),
}

/// Checks a lane's slots before automatic cleanup removes it: structural/git errors stay `Err`,
/// a running process returns `Busy`, and uncollected changes or ignored `local/` work return
/// `OwnerWork` so cleanup can keep the lane and notify the owner once instead of warning every tick.
pub(crate) fn check_for_cleanup(ctx: &Ctx, l: &LaneRec, cwds: &[(u32, PathBuf)]) -> Result<CleanupCheck, String> {
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let records = Records::load(&ctx.home)?;
    let mut slots: Vec<Slot> = records.slots.into_iter().filter(|s| s.lane == l.name && !gone(s)).collect();
    if slots.is_empty() {
        return Ok(CleanupCheck::Clean);
    }
    slots.sort_by_key(|s| s.n);
    let lane = Lane::new(&ctx.repo, l);
    for s in &slots {
        verify(&lane, s)?;
    }
    for s in &slots {
        if let Some(why) = busy_reason(s, cwds)? {
            return Ok(CleanupCheck::Busy(why));
        }
    }
    for s in &slots {
        if let Some(why) = dirty_reason(s)? {
            return Ok(CleanupCheck::OwnerWork(why));
        }
    }
    for s in &slots {
        let files = ignored_work(s)?;
        if !files.is_empty() {
            return Ok(CleanupCheck::OwnerWork(ignored_work_reason(s, &files)));
        }
    }
    Ok(CleanupCheck::Clean)
}

/// A `lane offers` row's slot fields: the lane's slots dir and its slots' records, read from
/// slots.json (their worktrees are read by the asking CLI, [`offer_refusal`]).
pub(crate) fn offered(ctx: &Ctx, l: &LaneRec) -> String {
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    match Records::load(&ctx.home) {
        Ok(r) => {
            let slots: Vec<String> = r.slots.iter().filter(|s| s.lane == l.name).map(Slot::json).collect();
            format!(",\"slots_dir\":{},\"slots\":[{}]", json_str(&Lane::new(&ctx.repo, l).dir().to_string_lossy()), slots.join(","))
        }
        Err(e) => format!(",\"slots_error\":{}", json_str(&e)),
    }
}

/// Why `lane retire` would refuse an offered lane for its slots, as [`check_clean`] checks them,
/// from the row [`offered`] wrote: read in the asking CLI. None for a row without slots (an
/// older master's).
pub(crate) fn offer_refusal(offer: &Value) -> Option<String> {
    if let Some(e) = offer.get("slots_error").and_then(Value::as_str) { return Some(e.to_string()); }
    let dir = PathBuf::from(offer.get("slots_dir").and_then(Value::as_str)?);
    for v in offer.get("slots").and_then(Value::as_arr).unwrap_or(&[]) {
        let s = match Slot::parse(v) { Ok(s) => s, Err(e) => return Some(e) };
        if gone(&s) { continue; }
        if let Err(why) = verify_in(&dir, &s).and_then(|_| clean(&s)) { return Some(why); }
    }
    None
}

/// Prune runs outside the master too; records remain protected if their lane is live.
pub(crate) fn protected(home: &Path) -> Result<Vec<PathBuf>, String> {
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let records = Records::load(home)?;
    if records.slots.is_empty() { return Ok(Vec::new()) }
    let store = crate::state::Store::open_quiet(home, "")?;
    let live: BTreeSet<String> = store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| l.name.clone()).collect());
    Ok(records.slots.into_iter().filter(|s| live.contains(&s.lane)).map(|s| std::fs::canonicalize(&s.path).unwrap_or(s.path)).collect())
}

/// CLI arguments become one JSON request, so a commit message retains spaces and newlines.
pub(crate) fn request(words: &[&str]) -> Result<String, String> {
    let (verb, lane, n, job, commit) = match words {
        ["slots", lane] => ("slots", *lane, 0, "", None),
        ["slot", verb @ ("diff" | "take" | "drop"), lane, n] => (*verb, *lane, number(n)?, "", None),
        ["slot", "take", lane, n, "--commit", msg] => ("take", *lane, number(n)?, "", Some(*msg)),
        ["slot", lane, n] => ("make", *lane, number(n)?, "", None),
        ["slot", lane, n, "--job", job] => ("make", *lane, number(n)?, *job, None),
        _ => return Err(USAGE.into()),
    };
    if !super::lane::valid_name(lane) { return Err(USAGE.into()) }
    Ok(format!("{{\"verb\":{},\"lane\":{},\"n\":{n},\"job\":{},\"commit\":{}}}", json_str(verb), json_str(lane), json_str(job), commit.map(json_str).unwrap_or_else(|| "null".into())))
}

pub(crate) fn serve(ctx: &Ctx, rest: &str) -> Result<String, String> {
    let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| USAGE.to_string())?;
    let verb = string(&v, "verb")?;
    let name = string(&v, "lane")?;
    let n = if verb == "slots" { 0 } else { number(&v.get("n").and_then(Value::as_u64).unwrap_or(0).to_string())? };
    let _lock = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let l = ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == name && l.state == "live").cloned()).ok_or_else(|| format!("no live lane {name}"))?;
    let l = Lane::new(&ctx.repo, &l);
    let mut records = Records::load(&ctx.home)?;
    if verb == "make" {
        let s = make(&l, &mut records, n, v.get("job").and_then(Value::as_str).unwrap_or(""), &wt::live_cwds())?;
        return Ok(format!("{{\"path\":{},\"text\":{}}}", json_str(&s.path.to_string_lossy()), json_str(&format!("{}\n", s.path.display()))));
    }
    if verb == "slots" {
        let mut slots: Vec<&Slot> = records.slots.iter().filter(|s| s.lane == name).collect();
        slots.sort_by_key(|s| s.n);
        let mut text = String::new();
        let mut rows = Vec::new();
        for s in slots {
            let count = if gone(s) { None } else { verify(&l, s)?; Some(changed(s)?.len()) };
            text.push_str(&format!("{} {} base={}{} job={} changed={}\n", s.n, s.path.display(), s.base[0], if s.base[1].is_empty() { String::new() } else { format!("/{}", s.base[1]) }, s.job, count.map_or("gone".to_string(), |c| c.to_string())));
            rows.push(format!("{{\"slot\":{},\"changed\":{}}}", s.json(), count.map_or("null".to_string(), |c| c.to_string())));
        }
        return Ok(format!("{{\"slots\":[{}],\"text\":{}}}", rows.join(","), json_str(&text)));
    }
    let s = records.slots.iter_mut().find(|s| s.lane == name && s.n == n).ok_or_else(|| format!("no slot {n} of {name}"))?;
    let text = match verb.as_str() {
        "diff" => diff(&l, s)?.into_iter().map(|p| p.text).collect(),
        "take" => { take(&l, s, v.get("commit").and_then(Value::as_str), &wt::live_cwds())?; format!("took slot {n} of {name}\n") }
        "drop" => { let file = crate::sync::lanes::lane_write(&l.tree, || drop_slot(&l, s, &wt::live_cwds()))?; format!("dropped slot {n} of {name}; saved {}\n", file.display()) }
        _ => return Err(USAGE.into()),
    };
    if verb != "diff" { records.save()?; }
    Ok(format!("{{\"text\":{}}}", json_str(&text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Pair {
        dir: PathBuf,
        lane: Lane,
        records: Records,
    }

    impl Pair {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("devhub-slot-test-{name}-{}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            let dir = dir.canonicalize().unwrap();
            let repo = dir.join("repo");
            for tree in [repo.clone(), repo.join(COMMERCIAL)] {
                std::fs::create_dir_all(&tree).unwrap();
                wt::git(Some(&tree), None, &["init", "-q"]).unwrap();
                let mut config = std::fs::OpenOptions::new().append(true).open(tree.join(".git/config")).unwrap();
                writeln!(config, "\n[user]\n name = Test\n email = test@example.invalid\n[commit]\n gpgsign = false").unwrap();
                wt::git(Some(&tree), None, &["config", "core.hooksPath", &tree.join(".git/hooks").to_string_lossy()]).unwrap();
                std::fs::write(tree.join(".gitignore"), "apps/commercial/\nlocal/\nignored/\ntarget/\n").unwrap();
                std::fs::write(tree.join("one"), "base\n").unwrap();
                std::fs::write(tree.join("other"), "other\n").unwrap();
                Self::commit(&tree, "base");
            }
            let root = wt::root_of(&repo);
            let tree = root.join("lane");
            for i in 0..2 {
                wt::git(Some(&half(&repo, i)), None, &["worktree", "add", "-q", "-b", "rc-lane", &half(&tree, i).to_string_lossy(), "HEAD"]).unwrap();
            }
            let lane = Lane { name: "lane".into(), repo, tree, root, branch: "rc-lane".into(), cbranch: "rc-lane".into() };
            let records = Records::load(&dir.join("home")).unwrap();
            Self { dir, lane, records }
        }

        fn commit(tree: &Path, msg: &str) {
            wt::git(Some(tree), None, &["add", "-A"]).unwrap();
            wt::git(Some(tree), None, &["commit", "-q", "-m", msg]).unwrap();
        }

        fn slot(&mut self, n: u8) -> Slot { make(&self.lane, &mut self.records, n, "J1", &[]).unwrap() }

        fn context(&self) -> (Ctx, LaneRec) {
            use std::sync::Arc;
            let home = self.dir.join("home");
            let rec = LaneRec {
                name: self.lane.name.clone(), path: self.lane.tree.to_string_lossy().into_owned(),
                branch: self.lane.branch.clone(), cbranch: self.lane.cbranch.clone(), state: "live".into(),
                ..Default::default()
            };
            let store = crate::state::Store::open_quiet(&home, "").unwrap();
            store.update(|d| d.lanes.push(rec.clone())).unwrap();
            let leases = Arc::new(crate::lease::master::LeaseMaster::new("", home.clone(), Arc::new(|_, _, _| Err("no nodes".into())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
            (Ctx { store, leases, machine: String::new(), repo: self.lane.repo.clone(), home, monitor: crate::monitor::Monitor::start(&[]) }, rec)
        }
    }

    impl Drop for Pair {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    #[test]
    fn make_reuse_and_dirty_refusal() {
        let mut p = Pair::new("reuse");
        let s = p.slot(1);
        let dotgit = std::fs::metadata(s.path.join(".git")).unwrap().modified().unwrap();
        for i in 0..2 {
            let tree = half(&p.lane.tree, i);
            std::fs::write(tree.join("one"), "moved\n").unwrap();
            Pair::commit(&tree, "move lane");
        }
        let reused = make(&p.lane, &mut p.records, 1, "J2", &[]).unwrap();
        assert_eq!(s.path, reused.path, "reuse keeps the same slot path");
        assert_eq!(s.made_ms, reused.made_ms, "reuse keeps its creation time");
        assert_eq!(dotgit, std::fs::metadata(s.path.join(".git")).unwrap().modified().unwrap(), "reuse does not remake the worktree");
        assert_eq!(reused.base, p.lane.heads().unwrap(), "both halves follow the lane's moved HEAD");
        let loaded = Records::load(&p.dir.join("home")).unwrap();
        assert_eq!(loaded.slots[0].job, "J2", "the assignment survives loading slots.json");
        assert_eq!(loaded.slots[0].base, reused.base, "the base pair survives loading slots.json");
        std::fs::write(s.path.join(COMMERCIAL).join("draft"), "unfinished\n").unwrap();
        let err = make(&p.lane, &mut p.records, 1, "J3", &[]).unwrap_err();
        assert!(err.contains("holds changes from job J2:") && err.contains("apps/commercial/draft") && !err.contains("(none recorded)"), "dirty refusal names its job and file: {err}");
        assert!(err.contains("devhub lane slot take lane 1") && err.contains("devhub lane slot drop lane 1"), "dirty refusal gives both commands: {err}");
        for n in ["0", "5", "-1", "01", "x"] { assert!(request(&["slot", "lane", n]).is_err(), "{n} is not a slot number"); }
        assert!(request(&["slot", "lane", "4", "--job", "J4"]).is_ok(), "slot four is allowed");
    }

    #[test]
    fn dirty_refusal_omits_job_clause_when_unlabeled_and_check_for_cleanup_classifies_blockers() {
        let mut p = Pair::new("cleanup");
        let mut first = make(&p.lane, &mut p.records, 1, "", &[]).unwrap();
        let second = p.slot(2);
        let (ctx, rec) = p.context();
        let check = |cwds: &[(u32, PathBuf)]| check_for_cleanup(&ctx, &rec, cwds);
        let refusal = "slot 1 of lane holds changes: \"one\"; take them (`devhub lane slot take lane 1`) or drop them (`devhub lane slot drop lane 1`)";
        std::fs::write(first.path.join("one"), "work\n").unwrap();
        for job in ["", "   "] {
            first.job = job.into();
            assert_eq!(clean(&first).unwrap_err(), refusal, "an unlabeled slot names its work and remedies");
        }
        first.job = "J9".into();
        assert!(clean(&first).unwrap_err().contains("holds changes from job J9:"), "a labeled slot names its job");
        assert_eq!(check(&[]), Ok(CleanupCheck::OwnerWork(refusal.into())), "uncollected work needs an owner");
        assert_eq!(check(&[(999, second.path.canonicalize().unwrap())]), Ok(CleanupCheck::Busy(
            "slot 2 of lane: pid 999 runs in it; wait for it to end".into()
        )), "a process in the second slot takes precedence over work in the first");

        std::fs::write(first.path.join("one"), "base\n").unwrap();
        std::fs::remove_dir_all(&second.path).unwrap();
        assert_eq!(check(&[]), Ok(CleanupCheck::Clean), "a clean slot and a missing slot allow cleanup");
        for file in ["local/notes/wip.txt", "local/devhub-spool/x"] {
            let path = first.path.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "work\n").unwrap();
        }
        assert_eq!(check(&[]), Ok(CleanupCheck::OwnerWork(
            "slot 1 of lane holds ignored local work: \"local/notes/wip.txt\"; move or delete them first".into()
        )), "only the owner's ignored work blocks cleanup");
        p.records.slots[0].path = p.lane.tree.clone();
        p.records.save().unwrap();
        assert!(check(&[]).unwrap_err().contains("slot 1 of lane: makepad half: record names"), "a misplaced record is a structural error");
    }

    #[test]
    fn committed_slot_work_is_dirty_and_taken() {
        let mut p = Pair::new("committed");
        let mut s = p.slot(1);
        for i in 0..2 {
            let tree = half(&s.path, i);
            std::fs::write(tree.join("one"), "committed slot\n").unwrap();
            Pair::commit(&tree, "slot work");
            assert!(dirty(&tree, i == 0).unwrap().is_empty(), "half {i}'s work is committed, not a dirty worktree");
        }
        assert_eq!(changed(&s).unwrap(), ["apps/commercial/one", "one"], "committed files count once with their half prefixes");
        let heads = [head(&s.path).unwrap(), head(&s.path.join(COMMERCIAL)).unwrap()];
        for err in [make(&p.lane, &mut p.records, 1, "J2", &[]).unwrap_err(), remove_slots(&p.lane, &mut p.records, false, &[]).unwrap_err()] {
            assert!(err.contains("apps/commercial/one") && err.contains("holds changes"), "reuse and removal refuse committed work: {err}");
        }
        for i in 0..2 { assert_eq!(head(&half(&s.path, i)).unwrap(), heads[i], "refusals keep half {i}'s committed HEAD"); }
        take(&p.lane, &mut s, None, &[]).unwrap();
        for i in 0..2 { assert_eq!(std::fs::read_to_string(half(&p.lane.tree, i).join("one")).unwrap(), "committed slot\n", "take carries half {i}'s committed content"); }
        assert!(changed(&s).unwrap().is_empty(), "taking committed work leaves a reusable slot");
    }

    #[test]
    fn index_only_versions_refuse_take_and_drop() {
        let mut p = Pair::new("staged");
        let mut s = p.slot(1);
        for i in 0..2 {
            let tree = half(&s.path, i);
            std::fs::write(tree.join("one"), "index only\n").unwrap();
            wt::git(Some(&tree), None, &["add", "one"]).unwrap();
            std::fs::write(tree.join("one"), "working file\n").unwrap();
        }
        let expected = "slot 1 of lane: \"one\", \"apps/commercial/one\" are staged with content their files no longer have; stage or restore them, then take or drop";
        assert_eq!(take(&p.lane, &mut s, None, &[]).unwrap_err(), expected, "take refuses both index-only versions");
        assert_eq!(drop_slot(&p.lane, &mut s, &[]).unwrap_err(), expected, "drop refuses both index-only versions");
        assert!(!p.lane.saved().exists(), "refusal happens before saving a misleading working-file patch");
        for i in 0..2 {
            let tree = half(&s.path, i);
            assert_eq!(git(&tree, None, &["show", ":one"]).unwrap(), "index only\n", "half {i}'s index-only content survives");
            assert_eq!(std::fs::read_to_string(tree.join("one")).unwrap(), "working file\n", "half {i}'s working content survives");
            assert_eq!(std::fs::read_to_string(half(&p.lane.tree, i).join("one")).unwrap(), "base\n", "half {i}'s lane is untouched");
            wt::git(Some(&tree), None, &["add", "one"]).unwrap();
        }
        take(&p.lane, &mut s, None, &[]).unwrap();
        assert!(changed(&s).unwrap().is_empty(), "staging the working versions permits take");
    }

    #[test]
    fn failed_second_apply_reverses_only_its_patches() {
        let mut p = Pair::new("rollback");
        let mut s = p.slot(1);
        for i in 0..2 { std::fs::write(half(&s.path, i).join("one"), "slot\n").unwrap(); }
        let err = take_locked(&p.lane, &mut s, None, &[], |i, tree, file| {
            if i == 0 { return apply(i, tree, file) }
            std::fs::write(p.lane.tree.join("other"), "lead staged\n").unwrap();
            wt::git(Some(&p.lane.tree), None, &["add", "other"]).unwrap();
            std::fs::write(p.lane.tree.join("other"), "lead working\n").unwrap();
            Err("forced second-half failure".into())
        }).unwrap_err();
        assert!(err.contains("forced second-half failure") && err.contains("applied halves rolled back"), "the second apply failure reverses the first: {err}");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("one")).unwrap(), "base\n", "reverse apply undoes only the first patch");
        assert_eq!(git(&p.lane.tree, None, &["show", ":one"]).unwrap(), "base\n", "reverse apply undoes the first patch in the index");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("other")).unwrap(), "lead working\n", "rollback keeps the lead's later working edit");
        assert_eq!(git(&p.lane.tree, None, &["show", ":other"]).unwrap(), "lead staged\n", "rollback keeps the lead's later staged edit");
        for i in 0..2 { assert_eq!(std::fs::read_to_string(half(&s.path, i).join("one")).unwrap(), "slot\n", "half {i}'s slot survives the failed take"); }
        let err = take_locked(&p.lane, &mut s, None, &[], |i, tree, file| {
            if i == 0 { return apply(i, tree, file) }
            // A later edit on a patched file makes --index refuse the reverse; never restore HEAD.
            std::fs::write(p.lane.tree.join("one"), "slot\nlead later\n").unwrap();
            Err("forced second-half failure".into())
        }).unwrap_err();
        assert!(err.contains("reverse apply failed") && err.contains("both hold the change") && err.contains(&p.lane.tree.to_string_lossy().to_string()), "failed rollback names the lane half and retained slot: {err}");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("one")).unwrap(), "slot\nlead later\n", "failed reverse never restores the patched file to HEAD");
        assert_eq!(git(&p.lane.tree, None, &["show", ":one"]).unwrap(), "slot\n", "failed reverse keeps the applied index content");
        assert_eq!(s.job, "J1", "both failures retain the slot assignment");
        for i in 0..2 { assert_eq!(std::fs::read_to_string(half(&s.path, i).join("one")).unwrap(), "slot\n", "failed reverse keeps half {i}'s slot copy"); }
    }

    #[test]
    fn live_slot_refuses_every_destructive_operation() {
        let mut p = Pair::new("live");
        let mut s = p.slot(1);
        let first = p.slot(2);
        let cwds = vec![(41, s.path.canonicalize().unwrap()), (42, s.path.join(COMMERCIAL).canonicalize().unwrap()), (43, s.path.with_file_name("10"))];
        let expected = "slot 1 of lane: pid 41, 42 runs in it; wait for it to end";
        assert_eq!(take(&p.lane, &mut s, None, &cwds).unwrap_err(), expected, "a running slot cannot be taken");
        assert_eq!(drop_slot(&p.lane, &mut s, &cwds).unwrap_err(), expected, "a running slot cannot be dropped");
        assert_eq!(make(&p.lane, &mut p.records, 1, "J2", &cwds).unwrap_err(), expected, "a running clean slot cannot be reused");
        p.records.slots.swap(0, 1);
        for force in [false, true] {
            assert_eq!(remove_slots(&p.lane, &mut p.records, force, &cwds).unwrap_err(), expected, "removal refuses a running slot even with force={force}");
            assert!(first.path.exists() && s.path.exists() && p.records.slots.len() == 2, "all slots are checked before either is removed");
        }
        assert_eq!(s.job, "J1", "the refused operations keep the job assignment");
        assert!(!p.lane.saved().exists(), "the refused operations do not snapshot a running job");
        assert!(idle(&s, &cwds[2..]).is_ok(), "a sibling whose name starts with the slot number is outside it");
    }

    #[test]
    fn ignored_local_work_is_kept_before_removal() {
        let mut p = Pair::new("local");
        let s = p.slot(1);
        for i in 0..2 {
            let tree = half(&s.path, i);
            std::fs::create_dir_all(tree.join("local/notes")).unwrap();
            std::fs::write(tree.join("local/notes/draft"), format!("half {i}\n")).unwrap();
            std::fs::create_dir_all(tree.join("local/devhub-spool")).unwrap();
            std::fs::write(tree.join("local/devhub-spool/result"), "spool\n").unwrap();
            std::fs::create_dir_all(tree.join("ignored")).unwrap();
            std::fs::write(tree.join("ignored/cache"), "build output\n").unwrap();
        }
        let outside = p.dir.join("outside");
        std::fs::write(&outside, "outside\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, s.path.join("local/link")).unwrap();
        let err = remove_slots(&p.lane, &mut p.records, false, &[]).unwrap_err();
        assert!(err.contains("local/notes/draft") && err.contains("apps/commercial/local/notes/draft") && err.contains("move or delete them first"), "removal names ignored work in both halves: {err}");
        assert!(!err.contains("devhub-spool") && !err.contains("ignored/cache"), "spool and build output do not block removal");
        assert!(s.path.exists() && p.records.slots.len() == 1, "refused removal keeps the worktrees and record");
        let notes = remove_slots(&p.lane, &mut p.records, true, &[]).unwrap();
        let saved = std::fs::read_dir(p.lane.saved()).unwrap().map(|e| e.unwrap().path()).find(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().starts_with("slot-1-local-")).expect("forced removal saves a local folder");
        assert!(notes.iter().any(|n| n.contains(&saved.to_string_lossy().to_string())), "forced removal reports the saved local folder");
        for i in 0..2 {
            assert_eq!(std::fs::read_to_string(half(&saved, i).join("local/notes/draft")).unwrap(), format!("half {i}\n"), "half {i}'s local files keep their relative paths and content");
            assert!(!half(&saved, i).join("local/devhub-spool").exists() && !half(&saved, i).join("ignored").exists(), "half {i}'s spool and build output are disposable");
        }
        #[cfg(unix)]
        assert!(!saved.join("local/link").exists() && notes.iter().any(|n| n.contains("local/link") && n.contains("symlink")), "symlinks are skipped with a note, never followed");
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside\n", "removal leaves the link target alone");
        assert!(!s.path.exists() && p.records.slots.is_empty(), "forced removal finishes after preserving local work");
        let s = p.slot(1);
        for i in 0..2 {
            for dir in ["local/devhub-spool", "target", "ignored"] {
                let dir = half(&s.path, i).join(dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("cache"), "disposable\n").unwrap();
            }
        }
        remove_slots(&p.lane, &mut p.records, false, &[]).unwrap();
        assert!(!s.path.exists(), "spool and ignored build output alone permit non-force removal");
        assert!(saved.is_dir(), "later removal leaves the saved local folder intact");
    }

    #[cfg(unix)]
    #[test]
    fn invalid_slot_paths_refuse_before_writing() {
        let mut p = Pair::new("paths");
        let mut s = p.slot(1);
        let indices: Vec<PathBuf> = (0..2).map(|i| PathBuf::from(git(&half(&p.lane.tree, i), None, &["rev-parse", "--path-format=absolute", "--git-path", "index"]).unwrap().trim())).collect();
        let before: Vec<Vec<u8>> = indices.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let refuse = |p: &mut Pair, s: &mut Slot, found: &str| {
            p.records.slots[0] = s.clone();
            let errors = [
                diff(&p.lane, s).err().expect("diff refuses the invalid path"),
                take(&p.lane, s, None, &[]).unwrap_err(),
                drop_slot(&p.lane, s, &[]).unwrap_err(),
                make(&p.lane, &mut p.records, 1, "J2", &[]).unwrap_err(),
                remove_slots(&p.lane, &mut p.records, false, &[]).unwrap_err(),
                remove_slots(&p.lane, &mut p.records, true, &[]).unwrap_err(),
            ];
            for err in errors { assert!(err.contains(found), "every operation names the invalid half and finding: {err}"); }
            assert!(!p.lane.saved().exists(), "invalid paths are refused before saving patches");
            assert_eq!(s.job, "J1", "invalid paths do not reset the assignment");
        };
        let slot_path = s.path.clone();
        s.path = p.lane.tree.clone();
        refuse(&mut p, &mut s, "makepad half: record names");
        s.path = slot_path;
        let commercial = s.path.join(COMMERCIAL);
        let moved = p.dir.join("commercial-kept");
        std::fs::rename(&commercial, &moved).unwrap();
        std::os::unix::fs::symlink(p.lane.tree.join(COMMERCIAL), &commercial).unwrap();
        refuse(&mut p, &mut s, "is a symlink");
        std::fs::remove_file(&commercial).unwrap();
        std::fs::rename(&moved, &commercial).unwrap();
        std::fs::rename(commercial.join(".git"), commercial.join(".git.kept")).unwrap();
        refuse(&mut p, &mut s, "commercial half: git toplevel is");
        std::fs::rename(commercial.join(".git.kept"), commercial.join(".git")).unwrap();
        for i in 0..2 {
            assert_eq!(std::fs::read(&indices[i]).unwrap(), before[i], "half {i}'s lane index is byte-for-byte unchanged");
            assert_eq!(std::fs::read_to_string(half(&p.lane.tree, i).join("one")).unwrap(), "base\n", "half {i}'s lane working file is unchanged");
            assert_eq!(std::fs::read_to_string(half(&s.path, i).join("one")).unwrap(), "base\n", "half {i}'s slot working file is unchanged");
        }
    }

    #[test]
    fn a_slot_whose_folder_is_gone_is_forgotten_not_a_refusal() {
        let mut p = Pair::new("gone");
        let s = p.slot(1);
        std::fs::remove_dir_all(&s.path).unwrap();
        assert!(gone(&s), "the folder is gone");
        let notes = remove_slots(&p.lane, &mut p.records, false, &[]).unwrap();
        assert!(notes.iter().any(|n| n.contains("was gone") && n.contains("record removed")), "removal says it forgot the slot: {notes:?}");
        assert!(p.records.slots.is_empty() && Records::load(&p.dir.join("home")).unwrap().slots.is_empty(), "the record is gone, saved too");
        let listed = git(&p.lane.repo, None, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(!listed.contains(&s.path.to_string_lossy().to_string()), "git no longer lists the gone worktree: {listed}");
    }

    #[test]
    fn lane_drop_saves_slots_before_its_first_commit() {
        let mut p = Pair::new("drop-order");
        let s = p.slot(1);
        std::fs::write(s.path.join("one"), "slot work\n").unwrap();
        std::fs::write(p.lane.tree.join("one"), "lane work\n").unwrap();
        let before = p.lane.heads().unwrap();
        let (ctx, _) = p.context();
        let home = &ctx.home;
        // Stop the first lane write, after slot removal but before any commit or bundle (11-15).
        let index = git(&p.lane.tree, None, &["rev-parse", "--path-format=absolute", "--git-path", "index.lock"]).unwrap();
        std::fs::write(index.trim(), "held\n").unwrap();
        let err = super::super::lane::serve(&ctx, "lane drop lane").unwrap_err();
        assert!(err.contains("index.lock"), "lane drop reaches its first commit preparation: {err}");
        assert!(!s.path.exists() && Records::load(home).unwrap().slots.is_empty(), "lane drop removed the slot before its first lane write");
        let saved: Vec<PathBuf> = std::fs::read_dir(p.lane.saved()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "diff")).collect();
        assert_eq!(saved.len(), 1, "lane drop saved the slot diff before its first lane write");
        assert!(std::fs::read_to_string(&saved[0]).unwrap().contains("+slot work"), "the saved diff holds the unbundled slot work");
        assert_eq!(p.lane.heads().unwrap(), before, "both lane heads are still uncommitted when the slot diff exists");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("one")).unwrap(), "lane work\n", "a failed drop keeps the lane working content");
        assert!(std::fs::read_dir(home.join("dropped")).unwrap().next().is_none(), "no bundle preceded the slot snapshot");
    }

    #[test]
    fn diff_and_take_preserve_unrelated_changes() {
        let mut p = Pair::new("take");
        let mut s = p.slot(1);
        let index = git(&s.path, None, &["rev-parse", "--path-format=absolute", "--git-path", "index"]).unwrap();
        let before = std::fs::read(index.trim()).unwrap();
        std::fs::write(s.path.join("one"), "slot\n").unwrap();
        std::fs::write(s.path.join("new file"), "untracked\n").unwrap();
        std::fs::write(s.path.join(COMMERCIAL).join("new file"), "nested\n").unwrap();
        let patches = diff(&p.lane, &s).unwrap();
        assert!(patches[0].text.contains("+untracked") && patches[1].text.contains("b/apps/commercial/new file"), "diff includes untracked files and prefixes the commercial half");
        assert_eq!(before, std::fs::read(index.trim()).unwrap(), "diff leaves the real slot index byte-for-byte alone");
        std::fs::write(p.lane.tree.join("one"), "lane dirty\n").unwrap();
        let err = take(&p.lane, &mut s, None, &[]).unwrap_err();
        assert!(err.contains("one") && err.contains("uncommitted"), "overlap refusal names the dirty file: {err}");
        assert!(!p.lane.tree.join(COMMERCIAL).join("new file").exists(), "overlap applies neither half");
        std::fs::write(p.lane.tree.join("one"), "base\n").unwrap();
        std::fs::write(p.lane.tree.join("other"), "unrelated staged\n").unwrap();
        wt::git(Some(&p.lane.tree), None, &["add", "other"]).unwrap();
        std::fs::write(p.lane.tree.join("other"), "unrelated unstaged\n").unwrap();
        take(&p.lane, &mut s, None, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("one")).unwrap(), "slot\n", "take applies the tracked edit");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("new file")).unwrap(), "untracked\n", "take applies the outer untracked file");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join(COMMERCIAL).join("new file")).unwrap(), "nested\n", "take applies the nested untracked file");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("other")).unwrap(), "unrelated unstaged\n", "take preserves unrelated worktree edits");
        assert_eq!(git(&p.lane.tree, None, &["show", ":other"]).unwrap(), "unrelated staged\n", "take preserves unrelated staged content");
        assert!(changed(&s).unwrap().is_empty() && s.job.is_empty(), "success cleans both slot halves and clears its job");
    }

    #[test]
    fn conflict_in_second_half_applies_nothing() {
        let mut p = Pair::new("conflict");
        let mut s = p.slot(1);
        for i in 0..2 { std::fs::write(half(&s.path, i).join("one"), "slot\n").unwrap(); }
        let commercial = p.lane.tree.join(COMMERCIAL);
        std::fs::write(commercial.join("one"), "lane commit\n").unwrap();
        Pair::commit(&commercial, "conflict");
        let err = take(&p.lane, &mut s, None, &[]).unwrap_err();
        assert!(err.contains("apps/commercial/one") && err.contains("nothing applied"), "the conflict names the nested file: {err}");
        assert_eq!(std::fs::read_to_string(p.lane.tree.join("one")).unwrap(), "base\n", "a second-half conflict leaves the first half unchanged");
        assert_eq!(std::fs::read_to_string(commercial.join("one")).unwrap(), "lane commit\n", "a conflict leaves the second half unchanged");
        for i in 0..2 { assert!(dirty(&half(&p.lane.tree, i), i == 0).unwrap().is_empty(), "a conflict leaves half {i}'s real index clean"); }
        assert_eq!(s.job, "J1", "a conflict retains the slot's job");
    }

    #[test]
    fn commit_only_taken_files_and_credit_in_order() {
        let mut p = Pair::new("commit");
        let mut s = p.slot(1);
        for i in 0..2 {
            std::fs::write(half(&s.path, i).join("one"), "taken\n").unwrap();
            let tree = half(&p.lane.tree, i);
            std::fs::write(tree.join("other"), "unrelated staged\n").unwrap();
            wt::git(Some(&tree), None, &["add", "other"]).unwrap();
        }
        take(&p.lane, &mut s, Some("take both"), &[]).unwrap();
        for i in 0..2 {
            let tree = half(&p.lane.tree, i);
            let msg = git(&tree, None, &["log", "-1", "--format=%B"]).unwrap();
            assert_eq!(msg.trim_end(), format!("take both\n\n{TRAILERS}"), "half {i} credits Claude first, then Codex");
            assert_eq!(git(&tree, None, &["show", "HEAD:other"]).unwrap(), "other\n", "half {i}'s commit excludes unrelated staged work");
            assert_eq!(git(&tree, None, &["show", ":other"]).unwrap(), "unrelated staged\n", "half {i} keeps unrelated staged work");
        }
        assert_eq!(s.base, p.lane.heads().unwrap(), "a committed take follows the lane's new heads");
    }

    #[cfg(unix)]
    #[test]
    fn refusing_hook_keeps_both_changes_uncommitted_and_the_slot() {
        use std::os::unix::fs::PermissionsExt;
        let mut p = Pair::new("hook");
        let mut s = p.slot(1);
        let heads = p.lane.heads().unwrap();
        for i in 0..2 { std::fs::write(half(&s.path, i).join("one"), "REFUSE_SLOT_MARKER\n").unwrap(); }
        let hook = p.lane.repo.join(COMMERCIAL).join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nif git show :one | grep -q REFUSE_SLOT_MARKER; then\n echo 'slot marker refused by hook' >&2\n exit 1\nfi\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = take(&p.lane, &mut s, Some("hook must run"), &[]).unwrap_err();
        assert!(err.contains("slot marker refused by hook"), "the configured hook runs and its refusal returns: {err}");
        assert_eq!(heads, p.lane.heads().unwrap(), "a second-half hook refusal undoes the first half's commit");
        for i in 0..2 {
            assert_eq!(std::fs::read_to_string(half(&p.lane.tree, i).join("one")).unwrap(), "REFUSE_SLOT_MARKER\n", "half {i}'s applied change remains uncommitted");
            assert_eq!(std::fs::read_to_string(half(&s.path, i).join("one")).unwrap(), "REFUSE_SLOT_MARKER\n", "half {i}'s slot remains intact");
        }
        assert_eq!(s.job, "J1", "hook refusal retains the job");
    }

    #[test]
    fn drop_and_retire_keep_only_unlanded_diffs() {
        let mut p = Pair::new("retire");
        let mut s = p.slot(1);
        std::fs::create_dir(s.path.join("ignored")).unwrap();
        std::fs::write(s.path.join("ignored/cache"), "keep on drop\n").unwrap();
        for i in 0..2 { std::fs::write(half(&s.path, i).join("one"), "landed\n").unwrap(); }
        let err = remove_slots(&p.lane, &mut p.records, false, &[]).unwrap_err();
        assert!(err.contains("one") && s.path.exists(), "retirement refuses a dirty slot without removing it: {err}");
        let landed = drop_slot(&p.lane, &mut s, &[]).unwrap();
        assert!(landed.is_file() && changed(&s).unwrap().is_empty(), "drop saves the combined diff and cleans the slot");
        assert!(s.path.join("ignored/cache").exists(), "drop never cleans ignored files");
        for i in 0..2 {
            let tree = half(&p.lane.tree, i);
            std::fs::write(tree.join("one"), "landed\n").unwrap();
            Pair::commit(&tree, "take saved change");
        }
        reset(&mut s, p.lane.heads().unwrap(), false).unwrap();
        std::fs::write(s.path.join("one"), "unlanded\n").unwrap();
        let kept = drop_slot(&p.lane, &mut s, &[]).unwrap();
        p.records.slots[0] = s.clone();
        let notes = remove_slots(&p.lane, &mut p.records, false, &[]).unwrap();
        assert!(!s.path.exists() && !p.lane.dir().exists() && p.records.slots.is_empty(), "retire removes both worktrees, their empty parent and records");
        assert!(!landed.exists(), "a saved change found in both branch tips is deleted");
        assert!(kept.exists() && notes.iter().any(|n| n == &format!("kept {}: its change is not in rc-lane", kept.display())), "an unlanded diff is kept and named: {notes:?}");
        for i in 0..2 {
            let worktrees = wt::git(Some(&half(&p.lane.repo, i)), None, &["worktree", "list", "--porcelain"]).unwrap();
            assert!(!worktrees.contains(&s.path.to_string_lossy().to_string()), "half {i}'s worktree registration is removed");
        }
    }
}
