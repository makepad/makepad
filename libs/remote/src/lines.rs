//! A request/answer service of JSON lines for the services that are not `cmd` (the relay's
//! `list` and `session`, the hub's `hosts`): one request line per DATA message, the answer lines
//! sent as one DATA message ending with `{"end":"<first word>","ms":N}`, like `cmd`. A handler that
//! waits may say so first ([`say_waiting`]), as `{"say":"<line>","waiting":true}` lines ahead of them.

use crate::cmd::json_str;
use crate::conn::{ChannelTx, Conn, Event, CLOSE_REFUSED};
use crate::endpoint::Factory;
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    /// The connection this thread answers requests on.
    static ASKED_ON: RefCell<Option<Conn>> = const { RefCell::new(None) };
    /// Where this thread answers its requests.
    static ANSWERS: RefCell<Option<ChannelTx>> = const { RefCell::new(None) };
}

/// Tells the client of the request this thread answers what it waits on, ahead of the answer, which
/// may be minutes away. The request has done nothing yet, so a client cut off now can ask again
/// (nothing outside a request).
pub fn say_waiting(line: &str) {
    answer_early(format!("{{\"say\":{},\"waiting\":true}}\n", json_str(line)));
}

/// The wait [`say_waiting`] told of is over: the request goes on to do what it asks.
pub fn wait_over() {
    answer_early("{\"say\":\"\",\"waiting\":false}\n".to_string());
}

fn answer_early(line: String) {
    ANSWERS.with(|tx| {
        if let Some(tx) = tx.borrow().as_ref() {
            let _ = tx.send(line.into_bytes());
        }
    });
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
                ANSWERS.with(|tx| *tx.borrow_mut() = Some(ch.tx()));
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
                        crate::served::tick(true);
                    }
                }
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the {name} thread: {e}")))
    })
}
