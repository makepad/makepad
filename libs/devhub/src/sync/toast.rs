//! What a sync's change asks of the person at a dock app that runs from the checkout: a toast in
//! the WM's notification stack (`makepad-wm notify`, no OS popup), the landed commits' subjects and
//! what to do ("Notes updated: list sorting (2 changes) — restart to apply: click here"). Only
//! when a person has to act ([`Outcome::asks_a_person`]) on a box a person uses (the hub says
//! which): a change that applied by itself says nothing, and a pool, hot or bench box nobody sits
//! at shows nothing (11-08: "why do these things show on the machines? seems not useful"). Each
//! outcome, toast or none, is timed against the landing that caused it (the order's `landed=` wall
//! ms) and logged in `~/.makepad/devhub/sync/visible.log`.

#[cfg(unix)]
use crate::proto;

/// What a sync brought, for the toasts it causes.
#[derive(Clone, Debug, Default)]
pub struct News {
    /// The landed commits' subjects, newest first (commercial before makepad), merges left out.
    pub subjects: Vec<String>,
    /// The landing's wall-clock ms (the master's kick), when the sync followed one.
    #[cfg_attr(not(unix), allow(dead_code))]
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

impl Outcome {
    /// What it asks of the person, as the toast says it: a restart they start or allow, or a change
    /// that did not apply. None: it applied by itself (live, restarted in place, makepad rebuilding
    /// into it) and needs nobody.
    fn ask(&self) -> Option<String> {
        match self {
            Outcome::Live | Outcome::Restarted | Outcome::Rebuilding => None,
            Outcome::Postponed(why) => Some(format!("restart to apply ({}): Restart is in the window's caption", cut(why, 80))),
            Outcome::Offered(_) => Some("restart to apply: click here to restart (same window, your document and view kept)".to_string()),
            Outcome::NeedsRestart(why) => Some(format!("restart to apply ({})", cut(why, 120))),
            Outcome::NotApplied(why) => Some(format!("not applied, the app runs the previous code ({})", cut(why, 120))),
        }
    }

    /// Whether a person has to act on it ([`Outcome::ask`]).
    pub fn asks_a_person(&self) -> bool {
        self.ask().is_some()
    }
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

/// The toast: ("Notes updated", "list sorting, Search status (2 changes) — restart to apply
/// (...)"); None for an outcome that asks nobody.
pub fn text(app: &str, news: &News, outcome: &Outcome) -> Option<(String, String)> {
    let what = match news.subjects.len() {
        0 => String::new(),
        1 => cut(plain(&news.subjects[0]), 72),
        n => format!("{}, {} ({n} changes)", cut(plain(&news.subjects[0]), 48), cut(plain(&news.subjects[1]), 48)),
    };
    let how = outcome.ask()?;
    let body = if what.is_empty() { how } else { format!("{what} — {how}") };
    Some((format!("{} updated", label(app)), body))
}

/// The name a person knows the app by (the registry's label), from a JIT's announced name.
pub fn label(app: &str) -> String {
    let a = makepad_app_registry::apps().iter().find(|e| e.id == app || e.package == app || e.bin == app || e.label == app);
    a.map_or_else(|| app.to_string(), |e| e.label.to_string())
}

/// A box a person uses, for its toasts: a user's WM, or one whose WM took a person's input in this
/// window.
pub const PERSON_MS: u64 = 5 * 60 * 1000;

/// The toast an outcome shows: one a person has to act on ([`text`]), on a box a person uses
/// (`person`); None: nothing to show.
pub fn shown(app: &str, news: &News, outcome: &Outcome, person: bool) -> Option<(String, String)> {
    text(app, news, outcome).filter(|_| person)
}

/// Logs the outcome in `visible.log`, timed from its landing (`toast=no`: none was shown), and
/// shows its toast ([`shown`]) in this machine's WM: its text then (None: none shown), and the ms
/// from the landing to now when known.
#[cfg(unix)]
pub fn post(app: &str, pid: u32, news: &News, outcome: &Outcome, person: bool) -> Result<(Option<String>, Option<u64>), String> {
    let said = if let Some((title, body)) = shown(app, news, outcome, person) {
        let path = makepad_app_registry::machine::wm_socket().ok_or("no WM runs in this session (no makepad-wm socket)")?;
        // an offered restart: the toast's click is the JIT's `session relaunch`
        let action = matches!(outcome, Outcome::Offered(_)).then(|| format!("relaunch={pid}"));
        makepad_app_registry::machine::request_notify(&path, &title, &body, action.as_deref())?;
        Some(format!("{title}: {body}"))
    } else {
        None
    };
    let now = proto::wall_ms();
    let after = news.landed_ms.map(|l| now.saturating_sub(l));
    let dir = proto::home().join("sync");
    let line = format!("{now} {app} {pid} landed={} visible_after_ms={} toast={} {:?}\n", news.landed_ms.map_or("-".into(), |l| l.to_string()), after.map_or("-".into(), |a| a.to_string()), if said.is_some() { "yes" } else { "no" }, outcome);
    let _ = std::fs::create_dir_all(&dir).and_then(|_| {
        use std::io::Write;
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("visible.log"))?.write_all(line.as_bytes())
    });
    Ok((said, after))
}

/// No makepad WM on this platform: nothing to show.
#[cfg(not(unix))]
pub fn post(app: &str, _pid: u32, news: &News, outcome: &Outcome, person: bool) -> Result<(Option<String>, Option<u64>), String> {
    match shown(app, news, outcome, person) {
        Some((title, body)) => Err(format!("no WM toasts on this platform ({title}: {body})")),
        None => Ok((None, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn news() -> News {
        News { subjects: vec!["notes: list sorting".into(), "notes: search status".into()], landed_ms: None }
    }

    /// A change that applied by itself says nothing, on any box; one a person has to act on shows
    /// only on a box a person uses (11-08: toasts on the pool's boxes "seem not useful").
    #[test]
    fn a_toast_only_asks_a_person_at_the_box() {
        let by_itself = [Outcome::Live, Outcome::Restarted, Outcome::Rebuilding];
        let asks = [Outcome::Postponed("unsaved work".into()), Outcome::Offered(String::new()), Outcome::NeedsRestart("a new type".into()), Outcome::NotApplied("a JIT error".into())];
        for o in &by_itself {
            assert!(!o.asks_a_person(), "{o:?}");
            assert_eq!(shown("notes", &news(), o, true), None, "{o:?} on a person's box");
            assert_eq!(shown("notes", &news(), o, false), None, "{o:?} on a pool box");
        }
        for o in &asks {
            assert!(o.asks_a_person(), "{o:?}");
            assert_eq!(shown("notes", &news(), o, false), None, "{o:?} on a box nobody uses");
            let (_, body) = shown("notes", &news(), o, true).unwrap_or_else(|| panic!("{o:?} on a person's box"));
            assert!(body.starts_with("list sorting, search status (2 changes) — "), "{body}");
        }
        assert!(shown("notes", &news(), &asks[0], true).unwrap().1.contains("restart to apply (unsaved work)"));
    }
}
