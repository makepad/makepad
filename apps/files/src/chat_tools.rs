//! What the chat panel's model is allowed to read and change.
//!
//! Four bounded read tools inspect paths; three mutation tools make a folder,
//! rename one item, or move one item to the platform Trash. On the desktop bus
//! those three wait for confirmation because their manifest risk is
//! `Destructive`. There is
//! no permanent delete or shell, and [`run`] is a closed match over the seven
//! names.
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
use std::sync::mpsc::{channel, Receiver, Sender};

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
const TOOL_TABLE: [(&str, &str, &str, Risk); 7] = [
    (
        "list_dir",
        "List what is directly inside a folder: each entry's name, whether it is a folder, its kind and its size. Bounded to the first 200 entries. Use this before saying anything about what a folder contains.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"folder path; ~ means the home folder, and a relative path is read from the folder the user is in"}},"required":["path"]}"#,
        Risk::Read,
    ),
    (
        "read_file",
        "Read the beginning of a text file (at most 16 kB). Binary files are refused with a note of what they are instead. Use this to answer questions about what a file actually says.",
        r#"{"type":"object","properties":{"path":{"type":"string"},"max_bytes":{"type":"integer","description":"how much to read, up to 16384"}},"required":["path"]}"#,
        Risk::Read,
    ),
    (
        "stat",
        "One path's kind, size and modification time. Cheap — use it when you only need to know what something is.",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        Risk::Read,
    ),
    (
        "treemap_summary",
        "Where a folder's bytes actually are: its heaviest direct children with their recursive sizes and file counts. This is what the treemap draws. Use it for 'what is taking up the space' questions.",
        r#"{"type":"object","properties":{"path":{"type":"string"},"top":{"type":"integer","description":"how many children to list, up to 12"}},"required":["path"]}"#,
        Risk::Read,
    ),
    (
        "mkdir",
        "Create a folder inside the home-folder jail. Refuses an existing path.",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"new folder path"}},"required":["path"]}"#,
        Risk::Destructive,
    ),
    (
        "rename",
        "Rename one file or folder inside the home-folder jail. The new name must be a bare name and must not already exist.",
        r#"{"type":"object","properties":{"path":{"type":"string"},"new_name":{"type":"string","description":"bare new name, with no path separators"}},"required":["path","new_name"]}"#,
        Risk::Destructive,
    ),
    (
        "trash",
        "Move one file or folder inside the home-folder jail to the platform Trash. It is never permanently deleted.",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
        Risk::Destructive,
    ),
];

/// The tools, exactly as the panel's own model is told about them.
#[cfg(feature = "chat")]
pub fn tools() -> Vec<ToolSpec> {
    TOOL_TABLE
        .iter()
        .map(|(name, description, schema, _)| ToolSpec::new(*name, *description, *schema))
        .collect()
}

/// The same seven tools as the desktop assistant learns them over the bus
/// (`ai_service.rs`). Mutations are `Destructive`, so the router confirms
/// them before a call reaches this app.
pub fn service_manifest() -> ServiceManifest {
    let mut manifest = ServiceManifest::new(
        "files",
        "Files",
        "The file browser. Its tools list folders, read text, inspect metadata, measure folder sizes, create folders, rename items, and move items to Trash. Paths may be absolute, `~` for the home folder, or relative to the folder the person is looking at; anything outside the home is refused. Mutations require confirmation.",
    );
    for (name, description, schema, risk) in TOOL_TABLE {
        manifest = manifest.with_tool(ToolDef::new(name, description, schema, risk));
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
    /// A successful filesystem mutation asks the UI to relist its folder.
    pub mutated: bool,
}

/// The tool worker: one thread, one job at a time, results in call order.
#[cfg(feature = "chat")]
pub struct ToolRunner {
    jobs: Sender<ToolJob>,
    results: Receiver<ToolOutcome>,
}

#[cfg(feature = "chat")]
impl ToolRunner {
    pub fn new(spawner: &makepad_widgets::makepad_platform::thread::ThreadSpawner) -> Self {
        let (jobs, job_rx) = channel::<ToolJob>();
        let (result_tx, results) = channel();
        if let Ok(handle) = spawner.spawn_worker(
            makepad_widgets::makepad_platform::thread::ThreadOptions {
                name: Some("files-chat-tools".into()),
                ..Default::default()
            },
            move || {
                while let Ok(job) = job_rx.recv() {
                    if result_tx.send(run(&job)).is_err() {
                        return;
                    }
                    makepad_widgets::makepad_platform::thread::SignalToUI::set_ui_signal();
                }
            },
        ) {
            handle.detach();
        }
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
    run_with_vfs(job, vfs().as_ref(), cancel, progress)
}

fn run_with_vfs(
    job: &ToolJob,
    fs: &dyn crate::vfs::Vfs,
    cancel: &AtomicBool,
    progress: &dyn Fn(u16),
) -> ToolOutcome {
    let raw = arg(&job.args, "path");
    let resolved = resolve_with(raw, &job.home, &job.cwd, fs);
    let path = match resolved {
        Ok(path) => path,
        Err(error) => {
            return ToolOutcome {
                note: format!("refused {}", short(Path::new(raw), &job.home)),
                text: error,
                is_error: true,
                mutated: false,
            }
        }
    };
    let shown = short(&path, &job.home);
    match job.name.as_str() {
        "list_dir" => finish(list_dir(fs, &path), format!("looked at {shown}"), shown),
        "read_file" => {
            let max = number(arg(&job.args, "max_bytes")).unwrap_or(READ_LIMIT);
            finish(read_file(fs, &path, max), format!("read {shown}"), shown)
        }
        "stat" => finish(stat(fs, &path), format!("checked {shown}"), shown),
        "treemap_summary" => {
            let top = number(arg(&job.args, "top"))
                .unwrap_or(SUMMARY_TOP)
                .clamp(1, SUMMARY_TOP);
            finish(summary(fs, &path, top, cancel, progress), format!("measured {shown}"), shown)
        }
        "mkdir" => mkdir(fs, &path, &shown),
        "rename" => rename(fs, &path, arg(&job.args, "new_name"), &job.home, &shown),
        "trash" => trash(fs, &path, &job.home, &shown),
        other => ToolOutcome {
            note: format!("unknown tool {other}"),
            text: format!("there is no tool called {other}"),
            is_error: true,
            mutated: false,
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
            mutated: false,
        },
        Err(error) => ToolOutcome {
            note: format!("could not read {shown}"),
            text: error,
            is_error: true,
            mutated: false,
        },
    }
}

fn refused(shown: &str, text: impl Into<String>) -> ToolOutcome {
    ToolOutcome {
        note: format!("refused {shown}"),
        text: text.into(),
        is_error: true,
        mutated: false,
    }
}

fn mutation(result: Result<String, String>, note: String, failed_note: String) -> ToolOutcome {
    match result {
        Ok(text) => ToolOutcome { note, text, is_error: false, mutated: true },
        Err(text) => ToolOutcome {
            note: failed_note,
            text,
            is_error: true,
            mutated: false,
        },
    }
}

// ------------------------------------------------------------- the sandbox

/// The path the model named, as a real path inside the user's home — or an
/// explanation of why it is not going to get one.
pub fn resolve(raw: &str, home: &Path, cwd: &Path) -> Result<PathBuf, String> {
    resolve_with(raw, home, cwd, vfs().as_ref())
}

fn resolve_with(raw: &str, home: &Path, cwd: &Path, fs: &dyn crate::vfs::Vfs) -> Result<PathBuf, String> {
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
    let real = match fs.canonicalize(&wanted) {
        Ok(real) => real,
        // A mutation may name a leaf that does not exist yet. Resolve the
        // nearest existing ancestor so a symlink cannot smuggle that leaf
        // outside the jail, then put the missing suffix back.
        Err(_) if fs.is_demo() => wanted.clone(),
        Err(first_error) => {
            let mut ancestor = wanted.clone();
            let mut suffix = Vec::new();
            let canonical = loop {
                let Some(name) = ancestor.file_name().map(|name| name.to_os_string()) else {
                    return Err(format!("{}: {first_error}", wanted.display()));
                };
                suffix.push(name);
                if !ancestor.pop() {
                    return Err(format!("{}: {first_error}", wanted.display()));
                }
                if let Ok(real) = fs.canonicalize(&ancestor) {
                    break real;
                }
            };
            suffix.into_iter().rev().fold(canonical, |path, name| path.join(name))
        }
    };
    let real_home = fs.canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
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
        vfs().as_ref(),
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

fn mkdir(fs: &dyn crate::vfs::Vfs, path: &Path, shown: &str) -> ToolOutcome {
    if fs.exists(path) {
        return refused(shown, format!("refused: {shown} already exists"));
    }
    mutation(
        fs.mkdir(path).map(|()| format!("created folder {shown}")),
        format!("created {shown}"),
        format!("could not create {shown}"),
    )
}

fn rename(
    fs: &dyn crate::vfs::Vfs,
    path: &Path,
    new_name: &str,
    home: &Path,
    shown: &str,
) -> ToolOutcome {
    if let Some(problem) = crate::rename::name_error(new_name) {
        return refused(shown, format!("refused: {problem}"));
    }
    if new_name.contains('\\') {
        return refused(shown, "refused: a name cannot contain a path separator");
    }
    if path == home {
        return refused(shown, "refused: the home folder itself cannot be renamed");
    }
    if !fs.exists(path) {
        return refused(shown, format!("refused: there is nothing at {shown}"));
    }
    let Some(parent) = path.parent() else {
        return refused(shown, format!("refused: {shown} has no parent folder"));
    };
    let target = parent.join(new_name);
    if !within(&target, home) {
        return refused(shown, "refused: the renamed path would leave the home folder");
    }
    if fs.exists(&target) {
        return refused(
            shown,
            format!("refused: {} already exists", short(&target, home)),
        );
    }
    let target_shown = short(&target, home);
    mutation(
        fs.rename(path, &target)
            .map(|()| format!("renamed {shown} to {target_shown}")),
        format!("renamed {shown} to {new_name}"),
        format!("could not rename {shown}"),
    )
}

fn trash(fs: &dyn crate::vfs::Vfs, path: &Path, home: &Path, shown: &str) -> ToolOutcome {
    if path == home {
        return refused(shown, "refused: the home folder itself cannot be trashed");
    }
    if !fs.exists(path) {
        return refused(shown, format!("refused: there is nothing at {shown}"));
    }
    let trash_dir = crate::ops::trash_dir(home);
    let trash_dir = match resolve_with(&trash_dir.display().to_string(), home, home, fs) {
        Ok(path) => path,
        Err(error) => return refused(shown, error),
    };
    if path == trash_dir {
        return refused(shown, "refused: the Trash folder itself cannot be trashed");
    }
    if !fs.exists(&trash_dir) {
        if let Err(error) = fs.mkdir(&trash_dir) {
            return mutation(
                Err(error),
                String::new(),
                format!("could not reach Trash for {shown}"),
            );
        }
    } else if !fs.is_dir(&trash_dir) {
        return refused(shown, "refused: the Trash path is not a folder");
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return refused(shown, format!("refused: {shown} has no movable name"));
    };
    let target = unique_destination(fs, &trash_dir, name);
    mutation(
        fs.rename(path, &target)
            .map(|()| format!("moved {shown} to Trash as {}", target.file_name().unwrap().to_string_lossy())),
        format!("trashed {shown}"),
        format!("could not trash {shown}"),
    )
}

/// Collision handling identical to the operations engine: suffix before an
/// extension, or at the end for an extensionless name and a dotfile.
fn unique_destination(fs: &dyn crate::vfs::Vfs, dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !fs.exists(&candidate) {
        return candidate;
    }
    let path = Path::new(name);
    let (stem, ext) = match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => (
            stem.to_string_lossy().into_owned(),
            ext.to_string_lossy().into_owned(),
        ),
        _ => (name.to_string(), String::new()),
    };
    let mut n = 2u64;
    loop {
        let candidate_name = if ext.is_empty() {
            format!("{name} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate = dir.join(candidate_name);
        if !fs.exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn list_dir(fs: &dyn crate::vfs::Vfs, path: &Path) -> Result<(String, String), String> {
    if !fs.is_dir(path) {
        return Err(format!("{} is not a folder", path.display()));
    }
    let entries = fs.read_dir(path, false)?;
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

fn read_file(fs: &dyn crate::vfs::Vfs, path: &Path, max_bytes: usize) -> Result<(String, String), String> {
    if fs.is_dir(path) {
        return Err(format!(
            "{} is a folder — use list_dir on it",
            path.display()
        ));
    }
    let size = fs.stat(path)?.size;
    let data = fs.read_bytes(path, max_bytes.clamp(1, READ_LIMIT))?;
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

fn stat(fs: &dyn crate::vfs::Vfs, path: &Path) -> Result<(String, String), String> {
    let entry = entry_for(fs, path)?;
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
fn entry_for(fs: &dyn crate::vfs::Vfs, path: &Path) -> Result<FileEntry, String> {
    if let Ok(entry) = fs.stat(path) {
        return Ok(entry);
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent to look in", path.display()))?;
    fs
        .read_dir(parent, true)?
        .into_iter()
        .find(|e| e.path == path)
        .ok_or_else(|| format!("there is nothing at {}", path.display()))
}

fn summary(
    fs: &dyn crate::vfs::Vfs,
    path: &Path,
    top: usize,
    cancel: &AtomicBool,
    progress: &dyn Fn(u16),
) -> Result<(String, String), String> {
    if !fs.is_dir(path) {
        // A file has no children; saying so beats an empty table.
        return stat(fs, path);
    }
    let entries = fs.read_dir(path, false)?;
    let deadline = makepad_widgets::Cx::monotonic_now() + MEASURE_BUDGET_SECS;
    let mut budget = MEASURE_ENTRIES;
    let mut measured: Vec<(String, u64, u32, bool)> = Vec::new();
    let mut complete = true;
    for (index, entry) in entries.iter().enumerate() {
        if entry.is_dir {
            let (bytes, files, done) = measure(fs, &entry.path, deadline, &mut budget, 0, cancel);
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
    fs: &dyn crate::vfs::Vfs,
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
    if fs.is_symlink(path).unwrap_or(false) {
        return (0, 0, true);
    }
    if model::skip_for_scan(path, &fs.home()) {
        return (0, 0, true);
    }
    let Ok(entries) = fs.read_dir(path, true) else {
        return (0, 0, true);
    };
    let mut bytes = 0u64;
    let mut files = 0u32;
    let mut complete = true;
    for entry in entries {
        *budget = budget.saturating_sub(1);
        if entry.is_dir {
            let (child_bytes, child_files, done) =
                measure(fs, &entry.path, deadline, budget, depth + 1, cancel);
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
    use crate::vfs::Vfs;

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
        assert_eq!(tools.len(), 7);
        for tool in &tools {
            assert!(tool
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(tool.parameters.starts_with('{'));
            assert!(tool.parameters.contains("\"properties\""));
            assert!(!tool.description.is_empty());
        }
        assert!(tools.iter().any(|tool| tool.name == "mkdir"));
        assert!(tools.iter().any(|tool| tool.name == "rename"));
        assert!(tools.iter().any(|tool| tool.name == "trash"));
    }

    #[test]
    fn the_bus_manifest_has_the_table_and_validates() {
        let manifest = service_manifest();
        assert_eq!(manifest.id, "files");
        manifest.validate().expect("a manifest the wire accepts");
        assert_eq!(manifest.tools.len(), TOOL_TABLE.len());
        for ((name, description, schema, risk), tool) in TOOL_TABLE.iter().zip(&manifest.tools) {
            assert_eq!(tool.name, *name);
            assert_eq!(tool.description, *description);
            assert_eq!(tool.parameters, *schema);
            assert_eq!(tool.risk, *risk);
            assert!(schema.contains(r#""type":"object""#), "{name}: an argument object");
        }
        for name in ["list_dir", "read_file", "stat", "treemap_summary"] {
            assert_eq!(manifest.tool(name).unwrap().risk, Risk::Read);
        }
        for name in ["mkdir", "rename", "trash"] {
            assert_eq!(manifest.tool(name).unwrap().risk, Risk::Destructive);
        }
    }

    fn demo_job(home: &Path, name: &str, args: &[(&str, &str)]) -> ToolJob {
        ToolJob {
            name: name.to_string(),
            args: args
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            cwd: home.join("Documents"),
            home: home.to_path_buf(),
        }
    }

    #[test]
    fn mkdir_rename_and_trash_mutate_the_demo_vfs() {
        let fs = crate::demo::DemoVfs::new();
        let home = fs.home();
        let created = home.join("Documents/assistant-created");
        let renamed = home.join("Documents/assistant-renamed");

        let outcome = run_with_vfs(
            &demo_job(&home, "mkdir", &[("path", "assistant-created")]),
            &fs,
            &AtomicBool::new(false),
            &|_| {},
        );
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(outcome.mutated);
        assert!(fs.is_dir(&created));

        let outcome = run_with_vfs(
            &demo_job(
                &home,
                "rename",
                &[("path", "assistant-created"), ("new_name", "assistant-renamed")],
            ),
            &fs,
            &AtomicBool::new(false),
            &|_| {},
        );
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(!fs.exists(&created));
        assert!(fs.is_dir(&renamed));

        let outcome = run_with_vfs(
            &demo_job(&home, "trash", &[("path", "assistant-renamed")]),
            &fs,
            &AtomicBool::new(false),
            &|_| {},
        );
        assert!(!outcome.is_error, "{}", outcome.text);
        assert!(!fs.exists(&renamed));
        assert!(fs.is_dir(&crate::ops::trash_dir(&home).join("assistant-renamed")));
    }

    #[test]
    fn every_mutation_refuses_a_jail_escape() {
        let fs = crate::demo::DemoVfs::new();
        let home = fs.home();
        for (tool, args) in [
            ("mkdir", vec![("path", "/Outside/new")]),
            ("rename", vec![("path", "/Outside/item"), ("new_name", "new")]),
            ("trash", vec![("path", "/Outside/item")]),
        ] {
            let outcome = run_with_vfs(
                &demo_job(&home, tool, &args),
                &fs,
                &AtomicBool::new(false),
                &|_| {},
            );
            assert!(outcome.is_error, "{tool}");
            assert!(outcome.text.contains("refused"), "{tool}: {}", outcome.text);
            assert!(!outcome.mutated, "{tool}");
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
