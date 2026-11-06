//! A request/answer service of JSON lines for the services that are not `cmd` (the relay's
//! `list` and `session`, the hub's `hosts`): one request line per DATA message, the answer lines
//! sent as one DATA message ending with `{"end":"<first word>","ms":N}`, like `cmd`.

use crate::cmd::json_str;
use crate::conn::{Conn, Event, CLOSE_REFUSED};
use crate::endpoint::Factory;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// The connection this thread answers requests on.
    static ASKED_ON: RefCell<Option<Conn>> = const { RefCell::new(None) };
}

/// Whether the client of the request this thread is answering has gone (never outside a request):
/// a handler that waits long can drop work whose answer nobody will read.
pub fn client_gone() -> bool {
    ASKED_ON.with(|c| c.borrow().as_ref().is_some_and(Conn::is_closed))
}

/// The connection of the request this thread answers (None outside a request): for a wait that
/// asks whether its client has gone from a loop or a thread of its own, where [`client_gone`]
/// sees no request.
pub fn asked_on() -> Option<Conn> {
    ASKED_ON.with(|c| c.borrow().clone())
}

/// Answers one request line by pushing answer lines (no `end` line).
pub type Handler = dyn Fn(&str, &mut Vec<String>) + Send + Sync;

pub fn service(name: &'static str, handler: Arc<Handler>) -> Factory {
    Arc::new(move |conn, open| {
        let hooks = conn.hooks();
        let handler = handler.clone();
        let asked_on = conn.clone();
        std::thread::Builder::new()
            .name(format!("makepad-remote-{name}"))
            .spawn(move || {
                (hooks.enter)();
                ASKED_ON.with(|c| *c.borrow_mut() = Some(asked_on));
                let ch = open.channel;
                'requests: while let Some(ev) = ch.recv() {
                    let data = match ev {
                        Event::Data(d) => d,
                        Event::Closed(..) => break,
                        Event::Opened(_) => continue,
                    };
                    for line in String::from_utf8_lossy(&data).lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let t = std::time::Instant::now();
                        let mut out = Vec::new();
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(line, &mut out)));
                        if r.is_err() {
                            out.push(format!("{{\"err\":{}}}", json_str("internal error in this request")));
                        }
                        out.push(format!("{{\"end\":{},\"ms\":{}}}", json_str(line.split_whitespace().next().unwrap_or("")), t.elapsed().as_millis()));
                        let mut text = out.join("\n");
                        text.push('\n');
                        if ch.send(text.into_bytes()).is_err() {
                            break 'requests;
                        }
                    }
                }
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the {name} thread: {e}")))
    })
}
