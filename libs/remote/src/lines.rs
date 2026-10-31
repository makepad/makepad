//! A request/answer service of JSON lines for the services that are not `cmd` (the relay's
//! `list` and `session`, the hub's `hosts`): one request line per DATA message, the answer lines
//! sent as one DATA message ending with `{"end":"<first word>","ms":N}`, like `cmd`.

use crate::cmd::json_str;
use crate::conn::{Event, CLOSE_REFUSED};
use crate::endpoint::Factory;
use std::sync::Arc;

/// Answers one request line by pushing answer lines (no `end` line).
pub type Handler = dyn Fn(&str, &mut Vec<String>) + Send + Sync;

pub fn service(name: &'static str, handler: Arc<Handler>) -> Factory {
    Arc::new(move |conn, open| {
        let hooks = conn.hooks();
        let handler = handler.clone();
        std::thread::Builder::new()
            .name(format!("makepad-remote-{name}"))
            .spawn(move || {
                (hooks.enter)();
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
