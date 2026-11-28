//! One ask of a devhub lines service, the master's or a box's through it, and its answer: what a
//! crate that only asks devhub needs. Its own crate (with [`crate::proto`]) so that such a crate
//! does not depend on all of devhub: every devhub landing changed an app's inputs through one ask,
//! and its checks and tests ran again in the buckets after each (11-27).

use makepad_remote::hub;
use makepad_remote::link::Client;
use std::time::Duration;

/// At most once a minute per process, including while a request waits for its answer.
/// Only the agent's own CLI has `DEVHUB_AGENT`; the master's runners do not speak for it.
pub fn agent_seen(c: &Client) {
    static SAID: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let Some(agent) = std::env::var("DEVHUB_AGENT").ok().filter(|a| makepad_devhub_conf::valid_name(a)) else { return };
    let now = std::time::Instant::now();
    let mut said = SAID.lock().unwrap();
    if !agent_seen_due(*said, now) || !has(c, "agent-seen") { return; }
    *said = Some(now);
    drop(said);
    let _ = ask_lines(c, None, "agent-seen", &format!("agent-seen {agent}"));
}

fn agent_seen_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(60))
}

/// Whether an ask's error says its link went while it was out: a master that restarts cuts what
/// it was answering ([`retry_master`]). By how it starts: an answer that only quotes one is an
/// answer. A session ended on a lost link says `W1 ended: box: link lost: ..`, and `live act` on it
/// asked again for good (11-09).
pub fn link_cut(e: &str) -> bool {
    e.starts_with("link lost") || e.starts_with("the hub connection ended")
}

/// What a cut ask's error adds when the service had said it still waited: nothing of the request
/// was done, so it can be asked again whatever it does.
pub const RAN_NOTHING: &str = "it had done nothing yet";

/// A `{"say":..}` line a service sends while it waits (`lines::say_waiting`): what it says, and
/// whether the request still waits. An answer's lines are not one.
pub fn said_line(l: &str) -> Option<(String, bool)> {
    if !l.starts_with("{\"say\":") {
        return None;
    }
    let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
    Some((v.get("say")?.as_str()?.to_string(), v.get("waiting")?.as_bool()?))
}

/// Sends one line to a lines service and collects the answer.
pub fn ask(c: &Client, target: Option<&str>, service: &str, line: &str) -> Result<Vec<String>, String> {
    if service != "agent-seen" { agent_seen(c); }
    ask_lines(c, target, service, line).map_err(|e| older(target, service, box_link(target, e)))
}

/// A routed ask's error, saying whose link it was: an ask of another box goes over the master's
/// socket and the master dials the box, never this process. A lane read the master's failed dial
/// (`Network is unreachable`) as macOS's Local Network block on its own CLI (11-17).
pub fn box_link(target: Option<&str>, e: String) -> String {
    match target {
        Some(t) if e.contains("cannot reach fleet hub") => format!("the master cannot reach {t}'s hub now (this command opens no connection of its own; `devhub nodes` shows the box): {e}"),
        _ => e,
    }
}

/// A refusal of a service this client knows means the other side runs older code: say so, and
/// what brings it up to date (a bare "unsupported service native" failed whole buckets, 11-02).
pub fn older(target: Option<&str>, service: &str, e: String) -> String {
    if !e.contains("unsupported service") {
        return e;
    }
    match target {
        None => format!("the master is older than this client (it has no `{service}` service): run `devhub master restart`"),
        Some(t) => format!("{t}'s hub is older than this client (it has no `{service}` service): it updates itself once its deployment has the landing (`devhub nodes` says when)"),
    }
}

fn ask_lines(c: &Client, target: Option<&str>, service: &str, line: &str) -> Result<Vec<String>, String> {
    let mut got = Vec::new();
    match target {
        Some(t) => {
            hub::ask(c, t, service, "", line, &mut |l| {
                got.push(l.to_string());
                true
            })?;
        }
        None => {
            let ch = c.open(service, "").map_err(|e| e.to_string())?;
            ch.send(line.as_bytes().to_vec()).map_err(|e| format!("link lost: {e}"))?;
            got = answer_of(|| loop {
                if service != "agent-seen" { agent_seen(c); }
                match ch.recv_timeout(Duration::from_secs(60)) {
                    Ok(event) => break Some(event),
                    Err(true) => continue,
                    Err(false) => break None,
                }
            })?;
        }
    }
    Ok(got)
}

/// The answer lines of one request, as its channel delivers them: what a service says while it
/// waits goes to stderr (`lines::say_waiting`), and a cut while it still waited says it did nothing.
fn answer_of(mut recv: impl FnMut() -> Option<makepad_remote::Event>) -> Result<Vec<String>, String> {
    let mut got = Vec::new();
    let mut waiting = false;
    let cut = |e: String, waiting: bool| if waiting { format!("{e}: {RAN_NOTHING}") } else { e };
    loop {
        match recv() {
            Some(makepad_remote::Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    if l.starts_with("{\"end\":") {
                        return Ok(got);
                    }
                    if let Some((said, still)) = said_line(l) {
                        if !said.is_empty() { eprintln!("{said}"); }
                        waiting = still;
                        continue;
                    }
                    got.push(l.to_string());
                }
            }
            Some(makepad_remote::Event::Opened(_)) => {}
            Some(makepad_remote::Event::Closed(makepad_remote::conn::CLOSE_LINK_LOST, why)) => return Err(cut(format!("link lost: {why}"), waiting)),
            Some(makepad_remote::Event::Closed(_, why)) => return Err(why),
            None => return Err(cut("the hub connection ended".into(), waiting)),
        }
    }
}

pub fn has(c: &Client, service: &str) -> bool {
    c.caps().iter().any(|s| s.split('/').next() == Some(service))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_calls_say_it_is_there_once_a_minute() {
        let now = std::time::Instant::now();
        assert!(agent_seen_due(None, now));
        assert!(!agent_seen_due(Some(now), now + Duration::from_millis(59_999)));
        assert!(agent_seen_due(Some(now), now + Duration::from_secs(60)));
        assert!(!agent_seen_due(Some(now + Duration::from_secs(60)), now + Duration::from_secs(61)));
    }

    /// What a service says while it waits is not its answer, and an answer is not said.
    #[test]
    fn a_said_line_is_told_apart_from_the_answer() {
        assert_eq!(said_line("{\"say\":\"live call W1 waits for room\",\"waiting\":true}"), Some(("live call W1 waits for room".to_string(), true)));
        assert_eq!(said_line("{\"say\":\"\",\"waiting\":false}"), Some((String::new(), false)));
        assert_eq!(said_line("{\"lines\":[],\"say\":\"x\"}"), None);
        assert_eq!(said_line("{\"say\":"), None);
    }

    /// An ask cut while its service still waited says it did nothing, which a caller may ask
    /// again; one cut after the wait, or with no wait said, doesn't.
    #[test]
    fn an_ask_cut_while_it_waited_says_it_did_nothing() {
        use makepad_remote::{conn::CLOSE_LINK_LOST, Event};
        let waits = "{\"say\":\"no room\",\"waiting\":true}\n";
        let over = "{\"say\":\"\",\"waiting\":false}\n";
        let answer = |lines: &[&str], end: Option<Event>| {
            let mut events: Vec<Option<Event>> = lines.iter().map(|l| Some(Event::Data(l.as_bytes().to_vec()))).collect();
            events.push(end);
            let mut events = events.into_iter();
            answer_of(|| events.next().flatten())
        };
        let lost = || Some(Event::Closed(CLOSE_LINK_LOST, "peer closed".to_string()));
        for end in [lost(), None] {
            let e = answer(&[waits], end).unwrap_err();
            assert!(link_cut(&e) && e.contains(RAN_NOTHING), "{e}");
        }
        for (lines, end) in [(&[waits, over][..], lost()), (&[waits, over], None), (&[], lost()), (&[], None)] {
            let e = answer(lines, end).unwrap_err();
            assert!(link_cut(&e) && !e.contains(RAN_NOTHING), "{lines:?}: {e}");
        }
        // an answer that quotes a cut is an answer: asked again, it would say the same for good
        assert!(!link_cut("W1 ended: boxa: link lost: cannot reach fleet hub `boxa`"));
        // what it said is not its answer
        let done = answer(&[waits, over, "{\"lines\":[]}\n{\"end\":\"live\",\"ms\":1}\n"], None);
        assert_eq!(done, Ok(vec!["{\"lines\":[]}".to_string()]));
    }
}
