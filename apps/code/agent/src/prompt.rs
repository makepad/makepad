//! The system prompt. Short on purpose: the model is a ~27B local one, and
//! every token here is prefilled on each new session.

pub fn system_prompt(root: &str, project_notes: &str) -> String {
    let mut out = String::with_capacity(1024 + project_notes.len());
    out.push_str(
        "You are a coding agent working in the project at ");
    out.push_str(root);
    out.push_str(
        ". Do the task with the tools; never guess file contents.\n\
         Rules:\n\
         - Find with grep/glob/list_dir, then read_file. A read shows up to 300 lines; do not read lines you already have.\n\
         - Call independent tools together in one turn (several reads, several edits).\n\
         - Change files with edit_file (old_string copied exactly, unique in the file) or write_file for new files. If an edit fails, read the lines the error names and retry.\n\
         - Never end a turn by saying what you will do next: make the tool call. Text without a call ends the task.\n\
         - After changing code, build or test with run_command and fix what fails. Run builds plain, not piped into tail/head/grep; you get the errors, failing tests and summary.\n\
         - run_command returns as soon as a failure shows, or after yield_ms: the command is then still running as run N in the background. Fix what you see; run_output N reads more, run_stop N ends it (stop a stale run before starting it again). Prefer fail-fast: pytest -x, jest --bail, go test -failfast; cargo test already stops at the first failing test binary (no --no-fail-fast).\n\
         - Three or more steps: keep a short plan with todo. Ask_user only when the request is unclear in a way that matters.\n\
         - Paths are relative to the project root; never touch files outside it.\n\
         - Finish with a few plain sentences: what changed, what you ran, what is left. Be brief; no preamble.\n\
         - Running makepad apps (here and on paired hosts) are yours to use: apps lists them, app_tools shows an app's tools, app_call calls one, ui drives its window (dump, snap, click, text, grab), launch starts one. Apps inside a window manager: tools through the WM target (host:wm, instance from apps), ui through their own pid (host:pid).\n\
         - The apps of this machine: list_apps lists them (\"what apps are there\"); start_app opens one by name (\"start stage\") the way the dock does, in the window manager when one runs; build_app (errors as file:line), run_app (native or makepad_run, from source; gives the pid for ui), stop_app, app_status, app_logs.\n\
         - After changing an app's UI or Splash code, run it (run_app) and fix every error its result lists: Splash mistakes compile fine and only show there.\n");
    if !project_notes.is_empty() {
        out.push_str("\n# Project instructions\n");
        out.push_str(project_notes);
        out.push('\n');
    }
    out
}

/// The sub-agent's prompt: one task, its own context, a summary back.
pub fn sub_agent_prompt(root: &str, read_only: bool, project_notes: &str) -> String {
    let mut out = String::new();
    out.push_str("You are a helper agent for the project at ");
    out.push_str(root);
    out.push_str(". You get one task. Do it with the tools");
    if read_only {
        out.push_str(" (read-only: you cannot change files)");
    }
    out.push_str(
        ", then reply with a compact summary of what you found or did: file paths, line numbers, \
         and anything the caller must act on. No preamble.\n");
    if !project_notes.is_empty() {
        out.push_str("\n# Project instructions\n");
        out.push_str(project_notes);
        out.push('\n');
    }
    out
}

/// Budget for the machine notes (`~/MACHINE.md`): about 1000 tokens.
const MACHINE_CHARS: usize = 3600;

/// What this machine is, in a few lines for the system prompt: its name and OS, the makepad
/// checkout its apps come from, whether a window manager runs here, and `~/MACHINE.md` (written by
/// whoever set the machine up: rules, which apps exist, how they start, where the logs are).
pub fn machine_notes() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| {
            std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "this machine".into());
    let mut out = format!("## This machine\n{host} ({} {})", std::env::consts::OS, std::env::consts::ARCH);
    if let Some(root) = makepad_app_registry::repo_root() {
        out.push_str(&format!("; makepad checkout {}", root.display()));
    }
    if std::env::var_os("MAKEPAD_WM_SOCKET").is_some() {
        out.push_str("; a makepad window manager runs here (start_app opens apps in it)");
    }
    out.push('\n');
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(path) = home.map(|h| h.join("MACHINE.md")) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push_str(&machine_md(&text, &path.display().to_string(), MACHINE_CHARS));
        }
    }
    out
}

/// `MACHINE.md` cut to `budget`: its head, then the sections an agent needs first (rules, apps,
/// logs, the checkout), then the others while they fit; the sections left out are named with
/// where to read them.
fn machine_md(text: &str, path: &str, budget: usize) -> String {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut head = String::new();
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            sections.push((title.trim().to_string(), format!("{line}\n")));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        } else {
            head.push_str(line);
            head.push('\n');
        }
    }
    let first = |t: &str| {
        let t = t.to_ascii_lowercase();
        ["rule", "app", "log", "checkout"].iter().any(|k| t.contains(k))
    };
    let mut order: Vec<usize> = (0..sections.len()).filter(|&i| first(&sections[i].0)).collect();
    order.extend((0..sections.len()).filter(|&i| !first(&sections[i].0)));
    let mut out: String = head.chars().take(budget).collect();
    // choose in priority order, then write the chosen ones in file order
    let mut kept = vec![false; sections.len()];
    let mut used = out.chars().count();
    for i in order {
        let n = sections[i].1.chars().count();
        if used + n <= budget {
            kept[i] = true;
            used += n;
        }
    }
    for (i, (_, body)) in sections.iter().enumerate() {
        if kept[i] {
            out.push_str(body);
        }
    }
    let left: Vec<&str> = sections.iter().zip(&kept).filter(|(_, k)| !**k).map(|((t, _), _)| t.as_str()).collect();
    if !left.is_empty() {
        out.push_str(&format!("[more in {path}: {}]\n", left.join("; ")));
    }
    out
}

/// Names of the project instruction files, in order of preference per folder.
const NOTE_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
/// Budget for all project instructions together (~600 tokens of a 16k window).
const NOTES_CHARS: usize = 2000;

/// Project instruction files from `root` up to the filesystem root, the
/// nearest first, cut to a budget. A cut file says where to read the rest.
pub fn load_project_notes(root: &std::path::Path) -> String {
    let mut out = String::new();
    let mut dir = Some(root);
    while let Some(d) = dir {
        for name in NOTE_FILES {
            let path = d.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let left = NOTES_CHARS.saturating_sub(out.chars().count());
            if left < 200 {
                return out;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            out.push_str(&format!("## {rel}\n"));
            if text.chars().count() > left {
                out.extend(text.chars().take(left));
                out.push_str(&format!("\n[cut: read_file {rel} for the rest]\n"));
            } else {
                out.push_str(&text);
                out.push('\n');
            }
            break;
        }
        dir = d.parent();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_notes_keep_the_rules_and_apps_first() {
        let text = "# MACHINE.md -- node 9\nintro\n## Hardware\nhw hw hw hw hw hw hw hw\n## Rules for an AI\n- be careful\n## Apps and how to start them\nStage=stage\n";
        let cut = machine_md(text, "/h/MACHINE.md", 110);
        assert!(cut.contains("be careful") && cut.contains("Stage=stage"), "{cut}");
        assert!(!cut.contains("hw hw"), "{cut}");
        assert!(cut.contains("[more in /h/MACHINE.md: Hardware]"), "{cut}");
        let all = machine_md(text, "/h/MACHINE.md", 4000);
        assert!(all.contains("hw hw") && !all.contains("[more in"), "{all}");
        // file order is kept for what fits
        assert!(all.find("Hardware").unwrap() < all.find("Rules").unwrap());
    }

    #[test]
    fn notes_walk_up_and_cut() {
        let base = std::env::temp_dir().join(format!("code-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        std::fs::write(base.join("AGENTS.md"), "outer rules").unwrap();
        std::fs::write(base.join("a/AGENTS.md"), "x".repeat(5000)).unwrap();
        let notes = load_project_notes(&base.join("a/b"));
        assert!(notes.contains("## ../AGENTS.md") || notes.contains("AGENTS.md"));
        assert!(notes.contains("[cut:"));
        assert!(notes.chars().count() < NOTES_CHARS + 200);
        let _ = std::fs::remove_dir_all(&base);
    }
}
