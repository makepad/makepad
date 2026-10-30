//! The user's "it's done" signal: after a sync moved the checkout, each dock app that runs from
//! it says in a toast what landed and how it reached the app ("Fab updated: hinge placement,
//! Rebuilding status (2 changes) — applied live"). The WM draws it in its notification stack
//! (`makepad-wm notify`, no OS popup); the text is the landed commits' subjects. Each toast is
//! timed against the landing that caused it (the order's `landed=` wall ms) and logged in
//! `~/.makepad/devhub/sync/visible.log`.

#[cfg(unix)]
use crate::proto;

/// What a sync brought, for the toasts it causes.
#[derive(Clone, Debug, Default)]
pub struct News {
    /// The landed commits' subjects, newest first (commercial before makepad), merges left out.
    pub subjects: Vec<String>,
    /// The landing's wall-clock ms (the master's kick), when the sync followed one.
    pub landed_ms: Option<u64>,
}

/// How a sync's change reached a running app.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Patched into the running code.
    Live,
    /// The app restarted in place with its state (document, view, window).
    Restarted,
    /// makepad rebuilds itself first; the app restarts into it by itself.
    Rebuilding,
    /// A restart waits for the person: the app said why (unsaved work).
    Postponed(String),
    /// A restart applies it, offered to the person: clicking the toast restarts the app in
    /// place (its window, document and view kept).
    Offered(String),
    /// A restart is needed and nothing will do it by itself.
    NeedsRestart(String),
    /// Not applied: the app keeps the previous code.
    NotApplied(String),
}

/// A subject as a person reads it: without a leading `area: ` tag.
fn plain(subject: &str) -> &str {
    match subject.split_once(": ") {
        Some((tag, rest)) if !rest.is_empty() && tag.len() <= 24 && !tag.contains(' ') => rest,
        _ => subject,
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}...", s.chars().take(max.saturating_sub(3)).collect::<String>())
}

/// The toast: ("Fab updated", "hinge placement, Rebuilding status (2 changes) — applied live").
pub fn text(app: &str, news: &News, outcome: &Outcome) -> (String, String) {
    let what = match news.subjects.len() {
        0 => String::new(),
        1 => cut(plain(&news.subjects[0]), 72),
        n => format!("{}, {} ({n} changes)", cut(plain(&news.subjects[0]), 48), cut(plain(&news.subjects[1]), 48)),
    };
    let how = match outcome {
        Outcome::Live => "applied live".to_string(),
        Outcome::Restarted => "applied: restarted in place, your document and view kept".to_string(),
        Outcome::Rebuilding => "makepad rebuilds first; the app restarts into it by itself, keeping your work".to_string(),
        Outcome::Postponed(why) => format!("restart to apply ({}): Restart is in the window's caption", cut(why, 80)),
        Outcome::Offered(_) => "restart to apply: click here to restart (same window, your document and view kept)".to_string(),
        Outcome::NeedsRestart(why) => format!("restart to apply ({})", cut(why, 120)),
        Outcome::NotApplied(why) => format!("not applied, the app runs the previous code ({})", cut(why, 120)),
    };
    let body = if what.is_empty() { how } else { format!("{what} — {how}") };
    (format!("{} updated", label(app)), body)
}

/// The name a person knows the app by (the registry's label), from a JIT's announced name.
pub fn label(app: &str) -> String {
    let a = makepad_app_registry::apps().iter().find(|e| e.id == app || e.package == app || e.bin == app || e.label == app);
    a.map_or_else(|| app.to_string(), |e| e.label.to_string())
}

/// Shows the toast in this machine's WM; the ms from the landing to now when known.
#[cfg(unix)]
pub fn post(app: &str, pid: u32, news: &News, outcome: &Outcome) -> Result<(String, Option<u64>), String> {
    let (title, body) = text(app, news, outcome);
    let path = makepad_app_registry::machine::wm_socket().ok_or("no WM runs in this session (no makepad-wm socket)")?;
    // an offered restart: the toast's click is the JIT's `session relaunch`
    let action = matches!(outcome, Outcome::Offered(_)).then(|| format!("relaunch={pid}"));
    makepad_app_registry::machine::request_notify(&path, &title, &body, action.as_deref())?;
    let now = proto::wall_ms();
    let after = news.landed_ms.map(|l| now.saturating_sub(l));
    let dir = proto::home().join("sync");
    let line = format!("{now} {app} {pid} landed={} visible_after_ms={} {:?}\n", news.landed_ms.map_or("-".into(), |l| l.to_string()), after.map_or("-".into(), |a| a.to_string()), outcome);
    let _ = std::fs::create_dir_all(&dir).and_then(|_| {
        use std::io::Write;
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("visible.log"))?.write_all(line.as_bytes())
    });
    Ok((format!("{title}: {body}"), after))
}

/// No makepad WM on this platform: nothing to show.
#[cfg(not(unix))]
pub fn post(app: &str, _pid: u32, news: &News, outcome: &Outcome) -> Result<(String, Option<u64>), String> {
    let (title, body) = text(app, news, outcome);
    Err(format!("no WM toasts on this platform ({title}: {body})"))
}
