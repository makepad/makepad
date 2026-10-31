//! What a network runtime has in flight, for `makepad inspect <pid> net`: every HTTP request from
//! its start until the app has READ its last event, and the runtime's event queue (events the
//! backend emitted that the app has not read yet, and when the app last read the queue). It
//! answers the two questions of a request that never completes: did the server answer, and did
//! the answer reach the app.
//!
//! Every runtime registers its ledger here; `report()` reads them from any thread without the UI
//! thread's help (a stuck UI loop is what it has to show).

use crate::types::{HttpRequest, NetworkResponse};
use makepad_live_id::LiveId;
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

/// Finished requests kept after the app read their answer.
const KEEP_DONE: usize = 12;

struct Entry {
    id: LiveId,
    request: String,
    started: Instant,
    /// another request with this id was still in flight when this one started
    reused_id: bool,
    /// bytes the backend reported so far (progress or stream chunks)
    bytes: u64,
    last_event: Option<Instant>,
    /// the last event was emitted: the answer (status) or the error
    finished: Option<(Instant, String)>,
    /// the app read the last event
    delivered: Option<Instant>,
}

#[derive(Default)]
struct State {
    live: Vec<Entry>,
    done: VecDeque<Entry>,
    /// emitted and not read yet: when, and what
    queue: VecDeque<(Instant, LiveId, &'static str)>,
    emitted: u64,
    read: u64,
    /// when the app last asked the queue for an event (found one or not)
    last_poll: Option<Instant>,
}

pub struct Ledger {
    created: Instant,
    state: Mutex<State>,
}

fn registry() -> &'static Mutex<Vec<Weak<Ledger>>> {
    static R: Mutex<Vec<Weak<Ledger>>> = Mutex::new(Vec::new());
    &R
}

/// The request as a short line: method and URL without its query.
fn describe(request: &HttpRequest) -> String {
    let url = request.url.split(['?', '#']).next().unwrap_or("");
    format!("{} {}", request.method.as_str(), url)
}

fn event_id(event: &NetworkResponse) -> (LiveId, bool) {
    match event {
        NetworkResponse::HttpResponse { request_id, .. }
        | NetworkResponse::HttpStreamComplete { request_id, .. }
        | NetworkResponse::HttpError { request_id, .. } => (*request_id, true),
        NetworkResponse::HttpStreamChunk { request_id, .. } | NetworkResponse::HttpProgress { request_id, .. } => (*request_id, false),
        NetworkResponse::WsOpened { socket_id }
        | NetworkResponse::WsMessage { socket_id, .. }
        | NetworkResponse::WsClosed { socket_id }
        | NetworkResponse::WsError { socket_id, .. } => (*socket_id, false),
    }
}

fn event_kind(event: &NetworkResponse) -> &'static str {
    match event {
        NetworkResponse::HttpResponse { .. } => "answer",
        NetworkResponse::HttpStreamChunk { .. } => "stream chunk",
        NetworkResponse::HttpStreamComplete { .. } => "stream end",
        NetworkResponse::HttpError { .. } => "error",
        NetworkResponse::HttpProgress { .. } => "progress",
        NetworkResponse::WsOpened { .. } => "ws opened",
        NetworkResponse::WsMessage { .. } => "ws message",
        NetworkResponse::WsClosed { .. } => "ws closed",
        NetworkResponse::WsError { .. } => "ws error",
    }
}

impl Ledger {
    pub(crate) fn new() -> Arc<Self> {
        let ledger = Arc::new(Self { created: Instant::now(), state: Mutex::new(State::default()) });
        if let Ok(mut r) = registry().lock() {
            r.retain(|w| w.strong_count() > 0);
            r.push(Arc::downgrade(&ledger));
        }
        ledger
    }

    pub(crate) fn http_started(&self, id: LiveId, request: &HttpRequest) {
        let Ok(mut s) = self.state.lock() else { return };
        let reused_id = s.live.iter().any(|e| e.id == id && e.delivered.is_none());
        if reused_id {
            for e in s.live.iter_mut().filter(|e| e.id == id) {
                e.reused_id = true;
            }
        }
        s.live.push(Entry { id, request: describe(request), started: Instant::now(), reused_id, bytes: 0, last_event: None, finished: None, delivered: None });
    }

    /// The backend emitted `event` (before it is queued).
    pub(crate) fn emitted(&self, event: &NetworkResponse) {
        let now = Instant::now();
        let Ok(mut s) = self.state.lock() else { return };
        s.emitted += 1;
        let (id, last) = event_id(event);
        s.queue.push_back((now, id, event_kind(event)));
        let Some(e) = s.live.iter_mut().find(|e| e.id == id && e.finished.is_none()) else { return };
        e.last_event = Some(now);
        match event {
            NetworkResponse::HttpProgress { progress, .. } => e.bytes = e.bytes.max(progress.loaded),
            NetworkResponse::HttpStreamChunk { response, .. } => e.bytes += response.body.as_ref().map_or(0, |b| b.len() as u64),
            _ => {}
        }
        if last {
            let what = match event {
                NetworkResponse::HttpResponse { response, .. } => format!("answered {}", response.status_code),
                NetworkResponse::HttpError { error, .. } => format!("error: {}", error.message),
                _ => "stream ended".to_string(),
            };
            e.finished = Some((now, what));
        }
    }

    /// The request ended without an event for the app (cancelled, or the backend refused it).
    pub(crate) fn ended(&self, id: LiveId, what: &str) {
        let now = Instant::now();
        let Ok(mut s) = self.state.lock() else { return };
        while let Some(at) = s.live.iter().position(|e| e.id == id && e.finished.is_none()) {
            let mut e = s.live.remove(at);
            e.finished = Some((now, what.to_string()));
            e.delivered = Some(now);
            if s.done.len() == KEEP_DONE {
                s.done.pop_front();
            }
            s.done.push_back(e);
        }
    }

    /// The app asked the queue for an event; `event` is what it got.
    pub(crate) fn polled(&self, event: Option<&NetworkResponse>) {
        let now = Instant::now();
        let Ok(mut s) = self.state.lock() else { return };
        s.last_poll = Some(now);
        let Some(event) = event else { return };
        s.read += 1;
        s.queue.pop_front();
        let (id, last) = event_id(event);
        if !last {
            return;
        }
        if let Some(at) = s.live.iter().position(|e| e.id == id && e.finished.is_some() && e.delivered.is_none()) {
            let mut e = s.live.remove(at);
            e.delivered = Some(now);
            if s.done.len() == KEEP_DONE {
                s.done.pop_front();
            }
            s.done.push_back(e);
        }
    }

    fn report(&self, n: usize, out: &mut String) -> Option<String> {
        let now = Instant::now();
        let s = self.state.lock().ok()?;
        let ago = |t: Instant| now.duration_since(t).as_secs_f64();
        if s.live.is_empty() && s.done.is_empty() && s.queue.is_empty() {
            return None;
        }
        let polled = s.last_poll.map_or("never".to_string(), |t| format!("{:.1} s ago", ago(t)));
        let verdict = if let Some((oldest, id, what)) = s.queue.front().filter(|(t, _, _)| ago(*t) >= 1.0) {
            format!(
                "STUCK: {} event(s) wait unread in the app's queue, the oldest ({id}: {what}) for {:.1} s; the app last read the queue {polled}",
                s.queue.len(),
                ago(*oldest)
            )
        } else if let Some(e) = s.live.iter().filter(|e| e.finished.is_none()).min_by_key(|e| e.started) {
            let silent = e.last_event.map_or(ago(e.started), ago);
            format!(
                "WAITING: {} request(s) in flight; the oldest ({} {}) started {:.1} s ago, nothing from the backend for {silent:.1} s",
                s.live.iter().filter(|e| e.finished.is_none()).count(),
                e.id,
                e.request,
                ago(e.started)
            )
        } else {
            "OK: nothing in flight, nothing unread".to_string()
        };
        let _ = writeln!(out, "## runtime {n} (made {:.1} s ago): {verdict}", ago(self.created));
        let _ = writeln!(out, "queue: {} unread; {} events emitted, {} read; last read {polled}", s.queue.len(), s.emitted, s.read);
        let _ = writeln!(out, "| id | request | started | backend | app read it |");
        let _ = writeln!(out, "|---|---|---|---|---|");
        for e in s.live.iter().chain(s.done.iter().rev()) {
            let backend = match &e.finished {
                Some((t, what)) => format!("{what} after {:.2} s", t.duration_since(e.started).as_secs_f64()),
                None if e.bytes > 0 => format!("in flight, {} bytes so far", e.bytes),
                None => "in flight, nothing yet".to_string(),
            };
            let read = match (e.delivered, &e.finished) {
                (Some(d), Some((f, _))) => format!("yes, {:.3} s after the backend", d.duration_since(*f).as_secs_f64()),
                (None, Some((f, _))) => format!("NO, unread for {:.1} s", ago(*f)),
                _ => "-".to_string(),
            };
            let reused = if e.reused_id { " (id reused while in flight)" } else { "" };
            let _ = writeln!(out, "| {}{reused} | {} | {:.1} s ago | {backend} | {read} |", e.id, e.request, ago(e.started));
        }
        Some(verdict)
    }
}

/// Every runtime of this process (of this build of the crate) with anything to show: a verdict
/// line per runtime first, then each runtime's requests (newest finished last).
pub fn report() -> String {
    let ledgers: Vec<Arc<Ledger>> = registry().lock().map(|r| r.iter().filter_map(Weak::upgrade).collect()).unwrap_or_default();
    let mut body = String::new();
    let mut verdicts = Vec::new();
    for (n, ledger) in ledgers.iter().enumerate() {
        if let Some(v) = ledger.report(n, &mut body) {
            verdicts.push(format!("runtime {n}: {v}"));
        }
    }
    if verdicts.is_empty() {
        return format!("net: {} runtime(s), no HTTP request made and no event queued\n", ledgers.len());
    }
    let mut out = String::new();
    for v in verdicts {
        let _ = writeln!(out, "{v}");
    }
    out.push('\n');
    out.push_str(&body);
    out
}
