//! What the chat panel's model is allowed to do: look, and nothing else.
//!
//! Four tools, all read-only — list a folder, read the head of a text file,
//! stat one path, and measure where a folder's bytes are. There is no write,
//! no move, no delete and no shell here, and there is no way to add one from
//! the model's side: [`run`] is a closed match over four names.
//!
//! Every path the model names goes through [`resolve`] first. It expands `~`,
//! folds `.` and `..` away *lexically* (so `~/../../etc` is refused before the
//! disk is touched at all), then canonicalises — which is what resolves any
//! symlink — and refuses anything that does not land inside the user's home.
//! A tool can therefore be handed any string at all and still only ever read
//! something the person running the app could already open in the browser.
//!
//! The tools run on a worker thread, never the UI's: measuring a folder is a
//! disk walk, and a file browser that stops painting because a chat is
//! counting bytes would be worse than one with no chat. Two callers share
//! them — the app's own panel through [`ToolRunner`] (in call order), and
//! the desktop's assistant through the bus runner in `ai_service.rs`, which
//! correlates by call id and can give up on a walk half-way ([`run_with`]).

use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "chat")]
use std::{
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

#[cfg(feature = "chat")]
use makepad_ai_hub::local_llm::ToolSpec;
use makepad_ai_services::wire::{Risk, ServiceManifest, ToolDef};

use crate::{
    model::{self, FileEntry},
    vfs::vfs,
};

/// The most entries one `list_dir` ever returns. A folder with ten thousand
/// files in it answers the question "what is in here" with the first two
/// hundred and a count, not with ten thousand lines of context.
const LIST_LIMIT: usize = 200;
/// The most bytes `read_file` will ever hand back.
const READ_LIMIT: usize = 16 * 1024;
/// The default, and the ceiling, for `treemap_summary`'s child count.
const SUMMARY_TOP: usize = 12;
/// How long one `treemap_summary` may spend walking before it answers with
/// what it has and says the numbers are a floor.
const MEASURE_BUDGET_SECS: f64 = 4.0;
/// How deep that walk goes, and how many entries it will look at.
const MEASURE_DEPTH: usize = 10;
const MEASURE_ENTRIES: usize = 400_000;

/// The one table every description of the tools is built from: the old
/// panel's `ToolSpec`s and the desktop bus's manifest read the SAME name,
/// sentence and schema, so the two can never drift apart. Every schema is
/// an argument object (`"type":"object"`), which the wire insists on.
const TOOL_TABLE: [(&str, &str, &str); 4] = [
    (
        "list_dir",
        "List what is directly inside a folder: each entry's name, whether it is a folder, its kind and its size. Bounded to the first 200 entries. Use this before saying anything about what a folder contains.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"folder path; ~ means the home folder, and a relative path is read from the folder the user is in"}},"required":["path"]}"#,
    ),
    (
        "read_file",
        "Read the beginning of a text file (at most 16 kB). Binary files are refused with a note of what they are instead. Use this to answer questions about what a file actually says.",
        r#"{"type":"object","properties":{"path":{"type":"string"},"max_bytes":{"type":"integer","description":"how much to read, up to 16384"}},"required":["path"]}"#,
    ),
    (
        "stat",
        "One path's kind, size and modification time. Cheap — use it when you only need to know what something is.",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    ),
    (
        "treemap_summary",
        "Where a folder's bytes actually are: its heaviest direct children with their recursive sizes and file counts. This is what the treemap draws. Use it for 'what is taking up the space' questions.",
        r#"{"type":"object","properties":{"path":{"type":"string"},"top":{"type":"integer","description":"how many children to list, up to 12"}},"required":["path"]}"#,
    ),
];

/// The tools, exactly as the panel's own model is told about them.
#[cfg(feature = "chat")]
pub fn tools() -> Vec<ToolSpec> {
    TOOL_TABLE
        .iter()
        .map(|(name, description, schema)| ToolSpec::new(*name, *description, *schema))
        .collect()
}

/// The same four tools as the desktop assistant learns them over the bus
/// (`ai_service.rs`). All of them only look, so all of them are `Read`.
pub fn service_manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::new(
        "files",
        "Files",
        "The file browser. Its tools only read: list a folder, read the head of \
         a text file, stat one path, and measure where a folder's bytes are \
         (the treemap). Paths may be absolute, `~` for the home folder, or \
         relative to the folder the person is looking at (its context line \
         says which); anything outside the home is refused.",
    );
    for (name, description, schema) in TOOL_TABLE {
        manifest = manifest.with_tool(ToolDef::new(name, description, schema, Risk::Read));
    }
    manifest
}

/// One tool call, as it goes to the worker.
pub struct ToolJob {
    pub name: String,
    pub args: Vec<(String, String)>,
    /// The folder the user is looking at: what a relative path is read from.
    pub cwd: PathBuf,
    pub home: PathBuf,
}

/// One tool call, as it comes back.
pub struct ToolOutcome {
    /// The dim line the transcript shows — "looked at ~/local/maps — 12 entries".
    pub note: String,
    /// What the model is told.
    pub text: String,
    pub is_error: bool,
}

/// The tool worker: one thread, one job at a time, results in call order.
#[cfg(feature = "chat")]
pub struct ToolRunner {
    jobs: Sender<ToolJob>,
    results: Receiver<ToolOutcome>,
}

#[cfg(feature = "chat")]
impl Default for ToolRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "chat")]
impl ToolRunner {
    pub fn new() -> Self {
        let (jobs, job_rx) = channel::<ToolJob>();
        let (result_tx, results) = channel();
        thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                if result_tx.send(run(&job)).is_err() {
                    return;
                }
                makepad_widgets::makepad_platform::thread::SignalToUI::set_ui_signal();
            }
        });
        Self { jobs, results }
    }

    pub fn submit(&self, job: ToolJob) {
        let _ = self.jobs.send(job);
    }

    pub fn drain(&self) -> Vec<ToolOutcome> {
        self.results.try_iter().collect()
    }
}

/// Run one tool. The whole of what the model can do to a filesystem.
#[cfg(any(test, feature = "chat"))]
pub fn run(job: &ToolJob) -> ToolOutcome {
    run_with(job, &AtomicBool::new(false), &|_| {})
}

/// [`run`] for a caller that may give up on the call: `cancel` set from any
/// thread makes the folder walk return at once (the result is then a floor,
/// and the bus runner reports the call as cancelled rather than as an
/// answer), and `progress` hears a permille as `treemap_summary` finishes
/// each direct child — the only tool slow enough to be worth watching.
pub fn run_with(job: &ToolJob, cancel: &AtomicBool, progress: &dyn Fn(u16)) -> ToolOutcome {
    let raw = arg(&job.args, "path");
    let resolved = resolve(raw, &job.home, &job.cwd);
    let path = match resolved {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome {
                note: format!("refused {}", short(Path::new(raw), &job.home)),
                text: error,
                is_error: true,
            }
        }
    };
    let shown = short(&path, &job.home);
    match job.name.as_str() {
        "list_dir" => finish(list_dir(&path), format!("looked at {shown}"), shown),
        "read_file" => {
            let max = number(arg(&job.args, "max_bytes")).unwrap_or(READ_LIMIT);
            finish(read_file(&path, max), format!("read {shown}"), shown)
        }
        "stat" => finish(stat(&path), format!("checked {shown}"), shown),
        "treemap_summary" => {
            let top = number(arg(&job.args, "top"))
                .unwrap_or(SUMMARY_TOP)
                .clamp(1, SUMMARY_TOP);
            finish(summary(&path, top, cancel, progress), format!("measured {shown}"), shown)
        }
        other => ToolOutcome {
            note: format!("unknown tool {other}"),
            text: format!("there is no tool called {other}"),
            is_error: true,
        },
    }
}

/// A tool's result plus the one-line note the transcript shows. The note gets
/// the tool's own tail ("— 12 entries") when it succeeded.
fn finish(result: Result<(String, String), String>, verb: String, shown: String) -> ToolOutcome {
    match result {
        Ok((tail, text)) => ToolOutcome {
            note: if tail.is_empty() {
                verb
            } else {
                format!("{verb} — {tail}")
            },
            text,
            is_error: false,
        },
        Err(error) => ToolOutcome {
            note: format!("could not read {shown}"),
            text: error,
            is_error: true,
        },
    }
}

// ------------------------------------------------------------- the sandbox

/// The path the model named, as a real path inside the user's home — or an
/// explanation of why it is not going to get one.
pub fn resolve(raw: &str, home: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let wanted = expand(raw, home, cwd);
    // Lexically first, so a path that walks out of the home is refused without
    // the disk being touched at all.
    if !within(&wanted, home) {
        return Err(format!(
            "refused: {} is outside {} — this assistant only looks inside the home folder",
            wanted.display(),
            home.display()
        ));
    }
    // Then for real: canonicalising is what follows a symlink, and a link out
    // of the home is exactly the case the lexical check cannot see.
    let real = match vfs().canonicalize(&wanted) {
        Ok(real) => real,
        // The demo filesystem has no paths on disk at all, and neither does a
        // path that is simply not there; both are the same answer here.
        Err(_) if crate::vfs::is_demo() => wanted.clone(),
        Err(error) => return Err(format!("{}: {error}", wanted.display())),
    };
    let real_home = vfs().canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if !within(&real, &real_home) {
        return Err(format!(
            "refused: {} leads outside {} — this assistant only looks inside the home folder",
            wanted.display(),
            home.display()
        ));
    }
    Ok(real)
}

/// `~`, relative paths and `.`/`..` folded away, without touching the disk.
pub fn expand(raw: &str, home: &Path, cwd: &Path) -> PathBuf {
    let raw = raw.trim().trim_matches('"');
    let joined = if raw.is_empty() || raw == "." {
        cwd.to_path_buf()
    } else if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        let path = Path::new(raw);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    };
    normalize(&joined)
}

/// `.` and `..` resolved textually. `..` past the root stays at the root,
/// which is what every filesystem does and what keeps the check below honest.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    // Nothing above the root: keep it, so the result stays
                    // absolute and the containment check still means something.
                    out.push(Component::RootDir.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::RootDir.as_os_str());
    }
    out
}

/// Is `path` the home folder, or something inside it?
pub fn within(path: &Path, home: &Path) -> bool {
    path == home || path.starts_with(home)
}

/// `~/rest` for anything under the home, the full path otherwise.
pub fn short(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// One argument by name; empty when the call did not give it.
fn arg<'a>(args: &'a [(String, String)], key: &str) -> &'a str {
    args.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// The bus runner's test seam: one bounded walk with a cancel flag.
#[cfg(test)]
pub fn measure_for_test(path: &Path, cancel: &AtomicBool) -> (u64, u32, bool) {
    let mut budget = MEASURE_ENTRIES;
    measure(
        path,
        makepad_widgets::Cx::monotonic_now() + MEASURE_BUDGET_SECS,
        &mut budget,
        0,
        cancel,
    )
}

fn number(text: &str) -> Option<usize> {
    text.trim().parse::<usize>().ok()
}

// ---------------------------------------------------------------- the tools

fn list_dir(path: &Path) -> Result<(String, String), String> {
    if !vfs().is_dir(path) {
        return Err(format!("{} is not a folder", path.display()));
    }
    let entries = vfs().read_dir(path, false)?;
    let total = entries.len();
    let mut out = format!("{} — {total} entries", path.display());
    if total > LIST_LIMIT {
        out.push_str(&format!(" (first {LIST_LIMIT} shown)"));
    }
    out.push('\n');
    for entry in entries.iter().take(LIST_LIMIT) {
        out.push_str(&format!(
            "{}  {:<10}  {}\n",
            if entry.is_dir { "dir " } else { "file" },
            entry.size_text(),
            entry.name,
        ));
    }
    Ok((format!("{total} entries"), out))
}

fn read_file(path: &Path, max_bytes: usize) -> Result<(String, String), String> {
    if vfs().is_dir(path) {
        return Err(format!(
            "{} is a folder — use list_dir on it",
            path.display()
        ));
    }
    let size = vfs().stat(path)?.size;
    let data = vfs().read_bytes(path, max_bytes.clamp(1, READ_LIMIT))?;
    let kind = model::kind_for(path, false);
    let looked_at = data.len().min(4096);
    if data[..looked_at].contains(&0) {
        return Ok((
            "binary".to_string(),
            format!(
                "{} is a {} of {} — not text, so there is nothing to read out of it here",
                path.display(),
                kind.label().to_lowercase(),
                model::format_size(size, false),
            ),
        ));
    }
    let cut = data.len();
    let text = match std::str::from_utf8(&data[..cut]) {
        Ok(text) => text.to_string(),
        Err(error) if error.valid_up_to() > cut / 2 => {
            String::from_utf8_lossy(&data[..error.valid_up_to()]).into_owned()
        }
        Err(_) => {
            return Ok((
                "binary".to_string(),
                format!(
                    "{} is a {} of {} — not text",
                    path.display(),
                    kind.label().to_lowercase(),
                    model::format_size(size, false),
                ),
            ))
        }
    };
    let mut out = format!(
        "{} — {}{}\n",
        path.display(),
        model::format_size(size, false),
        if (cut as u64) < size {
            format!(", first {} shown", model::format_size(cut as u64, false))
        } else {
            String::new()
        },
    );
    out.push_str(&text);
    Ok((model::format_size(cut as u64, false), out))
}

fn stat(path: &Path) -> Result<(String, String), String> {
    let entry = entry_for(path)?;
    let mut out = format!(
        "{}\nkind: {}\nsize: {}\nmodified: {}",
        path.display(),
        entry.kind_text(),
        if entry.is_dir {
            entry.size_text()
        } else {
            model::format_size(entry.size, false)
        },
        entry.modified_text(),
    );
    if !entry.permissions.is_empty() {
        out.push_str(&format!("\npermissions: {}", entry.permissions));
    }
    Ok((entry.kind_text().to_lowercase(), out))
}

/// The entry for one path: straight off the disk when there is one, out of the
/// parent's listing otherwise (which is the only way the demo can answer).
fn entry_for(path: &Path) -> Result<FileEntry, String> {
    if let Some(entry) = model::entry_at(path) {
        return Ok(entry);
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent to look in", path.display()))?;
    vfs()
        .read_dir(parent, true)?
        .into_iter()
        .find(|e| e.path == path)
        .ok_or_else(|| format!("there is nothing at {}", path.display()))
}

fn summary(
    path: &Path,
    top: usize,
    cancel: &AtomicBool,
    progress: &dyn Fn(u16),
) -> Result<(String, String), String> {
    if !vfs().is_dir(path) {
        // A file has no children; saying so beats an empty table.
        return stat(path);
    }
    let entries = vfs().read_dir(path, false)?;
    let deadline = makepad_widgets::Cx::monotonic_now() + MEASURE_BUDGET_SECS;
    let mut budget = MEASURE_ENTRIES;
    let mut measured: Vec<(String, u64, u32, bool)> = Vec::new();
    let mut complete = true;
    for (index, entry) in entries.iter().enumerate() {
        if entry.is_dir {
            let (bytes, files, done) = measure(&entry.path, deadline, &mut budget, 0, cancel);
            complete &= done;
            measured.push((entry.name.clone(), bytes, files, done));
        } else {
            measured.push((entry.name.clone(), entry.size, 1, true));
        }
        if cancel.load(Ordering::Relaxed) {
            // The caller gave up: what is measured so far is a floor, said so
            // below; the runner turns the whole answer into "cancelled".
            complete = false;
            break;
        }
        progress(((index + 1) * 1000 / entries.len().max(1)) as u16);
    }
    let total: u64 = measured.iter().map(|m| m.1).sum();
    let files: u32 = measured.iter().map(|m| m.2).sum();
    measured.sort_by(|a, b| b.1.cmp(&a.1));
    let shown = measured.len().min(top);
    let mut out = format!(
        "{} — {} in {} files across {} entries{}\n",
        path.display(),
        model::format_size(total, false),
        files,
        entries.len(),
        if complete {
            ""
        } else {
            " (the walk was cut short, so the sizes are a floor)"
        },
    );
    for (name, bytes, count, done) in measured.iter().take(shown) {
        out.push_str(&format!(
            "{:>10}{}  {:>5.1}%  {} ({} files)\n",
            model::format_size(*bytes, false),
            if *done { " " } else { "+" },
            *bytes as f64 * 100.0 / total.max(1) as f64,
            name,
            count,
        ));
    }
    if measured.len() > shown {
        out.push_str(&format!("…and {} smaller\n", measured.len() - shown));
    }
    Ok((format!("{} entries", entries.len()), out))
}

/// A folder's recursive bytes and file count, bounded by a deadline, an entry
/// budget and a depth. Returns false when it ran out of one of them — a number
/// that stopped early is a floor, and the caller says so rather than passing
/// it off as the answer.
fn measure(
    path: &Path,
    deadline: f64,
    budget: &mut usize,
    depth: usize,
    cancel: &AtomicBool,
) -> (u64, u32, bool) {
    if depth >= MEASURE_DEPTH
        || *budget == 0
        || makepad_widgets::Cx::monotonic_now() >= deadline
        || cancel.load(Ordering::Relaxed)
    {
        return (0, 0, false);
    }
    // Never walk through a link: the tree below it is somebody else's, and it
    // can lead straight back to where we started.
    if vfs().is_symlink(path).unwrap_or(false) {
        return (0, 0, true);
    }
    if model::skip_for_scan(path, &vfs().home()) {
        return (0, 0, true);
    }
    let Ok(entries) = vfs().read_dir(path, true) else {
        return (0, 0, true);
    };
    let mut bytes = 0u64;
    let mut files = 0u32;
    let mut complete = true;
    for entry in entries {
        *budget = budget.saturating_sub(1);
        if entry.is_dir {
            let (child_bytes, child_files, done) = measure(&entry.path, deadline, budget, depth + 1, cancel);
            bytes += child_bytes;
            files += child_files;
            complete &= done;
        } else {
            bytes += entry.size;
            files += 1;
        }
        if *budget == 0
            || makepad_widgets::Cx::monotonic_now() >= deadline
            || cancel.load(Ordering::Relaxed)
        {
            return (bytes, files, false);
        }
    }
    (bytes, files, complete)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/someone")
    }

    #[test]
    fn a_tilde_path_lands_in_the_home() {
        let cwd = home().join("Documents");
        assert_eq!(
            expand("~/Documents/notes", &home(), &cwd),
            home().join("Documents/notes")
        );
        assert_eq!(expand("~", &home(), &cwd), home());
        // Nothing at all means "where the user is".
        assert_eq!(expand("", &home(), &cwd), cwd);
    }

    #[test]
    fn a_relative_path_is_read_from_the_folder_the_user_is_in() {
        let cwd = home().join("Pictures");
        assert_eq!(expand("holiday", &home(), &cwd), cwd.join("holiday"));
        assert_eq!(expand("./holiday/..", &home(), &cwd), cwd);
    }

    #[test]
    fn dot_dot_is_folded_away_before_anything_is_read() {
        let cwd = home().join("Documents");
        assert_eq!(expand("~/a/../b", &home(), &cwd), home().join("b"));
        assert_eq!(expand("../Pictures", &home(), &cwd), home().join("Pictures"));
        // Past the root it stops at the root rather than going negative.
        assert_eq!(expand("/../../..", &home(), &cwd), PathBuf::from("/"));
    }

    #[test]
    fn paths_outside_the_home_are_refused() {
        let cwd = home();
        for escape in [
            "/etc/passwd",
            "~/../../etc/passwd",
            "../../../etc",
            "/Users/someone_else/Documents",
            "/",
        ] {
            let error = resolve(escape, &home(), &cwd)
                .expect_err(&format!("{escape} should have been refused"));
            assert!(
                error.contains("refused"),
                "{escape} gave the wrong reason: {error}"
            );
        }
    }

    #[test]
    fn a_sibling_whose_name_starts_with_the_home_is_not_inside_it() {
        // The string "/Users/someone-backup" starts with "/Users/someone",
        // and a prefix test on strings rather than components would let it in.
        assert!(!within(Path::new("/Users/someone-backup/x"), &home()));
        assert!(within(Path::new("/Users/someone/x"), &home()));
        assert!(within(&home(), &home()));
    }

    #[test]
    fn the_home_itself_resolves() {
        // Uses the real home, because resolve() canonicalises.
        let real_home = model::home_dir();
        let resolved = resolve("~", &real_home, &real_home);
        assert!(resolved.is_ok(), "{resolved:?}");
    }

    #[cfg(feature = "chat")]
    #[test]
    fn every_tool_has_a_schema_and_a_safe_name() {
        let tools = tools();
        assert_eq!(tools.len(), 4);
        for tool in &tools {
            assert!(tool
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(tool.parameters.starts_with('{'));
            assert!(tool.parameters.contains("\"properties\""));
            assert!(!tool.description.is_empty());
        }
        // Nothing that writes, moves, deletes or runs anything.
        for forbidden in ["write", "delete", "move", "rename", "run", "exec", "shell"] {
            assert!(
                !tools.iter().any(|t| t.name.contains(forbidden)),
                "a {forbidden} tool must never exist here"
            );
        }
    }

    #[test]
    fn the_bus_manifest_is_the_same_four_tools_and_validates() {
        let manifest = service_manifest();
        assert_eq!(manifest.id, "files");
        manifest.validate().expect("a manifest the wire accepts");
        assert_eq!(manifest.tools.len(), TOOL_TABLE.len());
        for ((name, description, schema), tool) in TOOL_TABLE.iter().zip(&manifest.tools) {
            assert_eq!(tool.name, *name);
            assert_eq!(tool.description, *description);
            assert_eq!(tool.parameters, *schema);
            assert_eq!(tool.risk, Risk::Read, "{name} only looks");
            assert!(schema.contains(r#""type":"object""#), "{name}: an argument object");
        }
        // Nothing that writes, moves, deletes or runs anything.
        for forbidden in ["write", "delete", "move", "rename", "run", "exec", "shell"] {
            assert!(
                !manifest.tools.iter().any(|t| t.name.contains(forbidden)),
                "a {forbidden} tool must never exist here"
            );
        }
    }

    #[test]
    fn a_cancelled_walk_answers_at_once_and_says_it_is_a_floor() {
        // Cancel before the first child: the walk returns immediately and
        // the summary carries the floor marker.
        let home = model::home_dir();
        let cancel = AtomicBool::new(true);
        let job = ToolJob {
            name: "treemap_summary".to_string(),
            args: vec![("path".to_string(), "~".to_string())],
            cwd: home.clone(),
            home,
        };
        let started = makepad_widgets::Cx::monotonic_now();
        let outcome = run_with(&job, &cancel, &|_| {});
        assert!(
            makepad_widgets::Cx::monotonic_now() - started < 2.0,
            "the flag must be honoured at once"
        );
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(outcome.text.contains("cut short"), "{}", outcome.text);
    }

    #[test]
    fn an_unknown_tool_is_an_error_not_a_panic() {
        let home = model::home_dir();
        let outcome = run(&ToolJob {
            name: "rm_rf".to_string(),
            args: vec![("path".to_string(), "~".to_string())],
            cwd: home.clone(),
            home,
        });
        assert!(outcome.is_error);
        assert!(outcome.text.contains("no tool called"));
    }

    #[test]
    fn a_refused_path_never_reaches_a_tool() {
        let home = model::home_dir();
        let outcome = run(&ToolJob {
            name: "read_file".to_string(),
            args: vec![("path".to_string(), "/etc/passwd".to_string())],
            cwd: home.clone(),
            home,
        });
        assert!(outcome.is_error);
        assert!(outcome.text.contains("refused"));
        assert!(!outcome.text.contains("root:"));
    }
}
