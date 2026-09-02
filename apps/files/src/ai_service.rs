//! The file browser on the desktop's AI bus.
//!
//! Hosted by the window manager, the app opens one [`AiServicePort`] with
//! the manifest from `chat_tools::service_manifest` and answers the calls
//! that come back through it. The tools are the same four the app's own
//! panel has; what is new is how they run: every call carries the
//! engine's `call_id`, the answer carries it back, and the person (or the
//! router) can give up on a call mid-walk. The old panel's runner is
//! order-only and cannot do either, so this one sits beside it rather
//! than inside it, and the two never share a job.
//!
//! One worker thread, one job at a time. A job in flight is cancelled by
//! a flag the walk checks on every entry; a job still queued behind it is
//! cancelled before it starts. Progress from the walk and the finished
//! results come back on channels the UI drains on its signal.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
};

use makepad_ai_services::wire::{ServiceCall, ToolResult};
use makepad_strict_json as json;
use makepad_widgets::makepad_platform::thread::SignalToUI;

use crate::chat_tools::{run_with, ToolJob, ToolOutcome};

/// One call on its way to the worker.
struct ServiceJob {
    call_id: String,
    job: ToolJob,
    cancel: Arc<AtomicBool>,
}

/// What the worker sends back, in the order it happened.
pub enum ServiceReply {
    Result(ToolResult),
    Progress { call_id: String, note: String, permille: u16 },
}

/// The bus's tool worker: correlated by call id, cancellable.
pub struct ServiceRunner {
    jobs: Sender<ServiceJob>,
    replies: Receiver<ServiceReply>,
    /// The cancel flag of every call not yet answered.
    live: HashMap<String, Arc<AtomicBool>>,
}

impl Default for ServiceRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRunner {
    pub fn new() -> Self {
        let (jobs, job_rx) = channel::<ServiceJob>();
        let (reply_tx, replies) = channel::<ServiceReply>();
        thread::spawn(move || {
            while let Ok(ServiceJob { call_id, job, cancel }) = job_rx.recv() {
                let result = if cancel.load(Ordering::Relaxed) {
                    // Given up on while it waited its turn: never started.
                    ToolResult::cancelled(&call_id)
                } else {
                    let progress_tx = reply_tx.clone();
                    let progress_id = call_id.clone();
                    let progress = move |permille: u16| {
                        let _ = progress_tx.send(ServiceReply::Progress {
                            call_id: progress_id.clone(),
                            note: "measuring…".to_string(),
                            permille,
                        });
                        SignalToUI::set_ui_signal();
                    };
                    let outcome = run_with(&job, &cancel, &progress);
                    if cancel.load(Ordering::Relaxed) {
                        // The walk stopped early on the flag: what it has
                        // is a floor nobody asked for any more.
                        ToolResult::cancelled(&call_id)
                    } else {
                        result_for(&call_id, outcome)
                    }
                };
                if reply_tx.send(ServiceReply::Result(result)).is_err() {
                    return;
                }
                SignalToUI::set_ui_signal();
            }
        });
        Self { jobs, replies, live: HashMap::new() }
    }

    /// Queue one call. `cwd` is the folder the person is looking at — what
    /// a relative path is read from — and `home` the jail.
    pub fn submit(&mut self, call: &ServiceCall, cwd: PathBuf, home: PathBuf) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.live.insert(call.call_id.clone(), cancel.clone());
        let job = ToolJob {
            name: call.tool.clone(),
            args: flat_args(&call.args),
            cwd,
            home,
        };
        let _ = self.jobs.send(ServiceJob { call_id: call.call_id.clone(), job, cancel });
    }

    /// Give up on one call, running or queued. Unknown ids are nothing.
    pub fn cancel(&mut self, call_id: &str) {
        if let Some(flag) = self.live.get(call_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Everything the worker sent since the last drain. A result retires
    /// its call's flag.
    pub fn drain(&mut self) -> Vec<ServiceReply> {
        let replies: Vec<ServiceReply> = self.replies.try_iter().collect();
        for reply in &replies {
            if let ServiceReply::Result(result) = reply {
                self.live.remove(&result.call_id);
            }
        }
        replies
    }
}

/// A tool's outcome as the wire says it. The tools already sort their own
/// failures into "refused" (the jail, an unknown name) and "could not"
/// (the disk said no); the wire keeps that distinction.
fn result_for(call_id: &str, outcome: ToolOutcome) -> ToolResult {
    if !outcome.is_error {
        return ToolResult::ok(call_id, outcome.text, outcome.note);
    }
    if outcome.note.starts_with("refused") || outcome.note.starts_with("unknown tool") {
        ToolResult::refused(call_id, outcome.text)
    } else {
        ToolResult::failed(call_id, outcome.text)
    }
}

/// The call's JSON argument object as the tools read it: one string per
/// key. Numbers and booleans become their text; nested values are dropped,
/// since no tool here takes one.
pub fn flat_args(args: &str) -> Vec<(String, String)> {
    let Ok(json::Value::Obj(fields)) = json::parse(args.as_bytes()) else {
        return Vec::new();
    };
    fields
        .into_iter()
        .filter_map(|(key, value)| {
            let text = match value {
                json::Value::Str(s) => s,
                json::Value::Int(i) => i.to_string(),
                json::Value::F64(f) => f.to_string(),
                json::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((key, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::ToolOutcome as Outcome;
    use std::time::{Duration, Instant};

    fn call(id: &str, tool: &str, args: &str) -> ServiceCall {
        ServiceCall { call_id: id.into(), tool: tool.into(), args: args.into() }
    }

    fn wait_for(runner: &mut ServiceRunner, n: usize) -> Vec<ToolResult> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < deadline {
            for reply in runner.drain() {
                if let ServiceReply::Result(r) = reply {
                    out.push(r);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        out
    }

    #[test]
    fn json_args_become_the_tools_flat_pairs() {
        let args = flat_args(r#"{"path":"~/Downloads","top":3,"deep":{"x":1},"quick":true}"#);
        assert_eq!(
            args,
            vec![
                ("path".to_string(), "~/Downloads".to_string()),
                ("top".to_string(), "3".to_string()),
                ("quick".to_string(), "true".to_string()),
            ]
        );
        assert!(flat_args("not json").is_empty());
        assert!(flat_args("[1,2]").is_empty());
    }

    #[test]
    fn results_carry_their_own_call_ids_and_a_queued_cancel_never_runs() {
        let home = crate::model::home_dir();
        let mut runner = ServiceRunner::new();
        // An unknown tool is refused; a jail escape is refused; both keep
        // their ids whatever order the worker answers in.
        runner.submit(&call("a", "stat", r#"{"path":"/etc/passwd"}"#), home.clone(), home.clone());
        runner.submit(&call("b", "rm_rf", r#"{"path":"~"}"#), home.clone(), home.clone());
        runner.submit(&call("c", "stat", r#"{"path":"~"}"#), home.clone(), home.clone());
        // Cancelled while it is still queued: it must come back cancelled
        // without the tool ever running.
        runner.cancel("c");
        assert_eq!(runner.live.len(), 3);
        let results = wait_for(&mut runner, 3);
        assert_eq!(results.len(), 3);
        let by_id = |id: &str| results.iter().find(|r| r.call_id == id).unwrap();
        assert_eq!(by_id("a").outcome, Outcome::Refused);
        assert!(by_id("a").text.contains("refused"));
        assert_eq!(by_id("b").outcome, Outcome::Refused);
        assert!(by_id("b").text.contains("no tool called"));
        assert_eq!(by_id("c").outcome, Outcome::Cancelled);
        assert_eq!(runner.live.len(), 0);
        // An unknown id is nothing.
        runner.cancel("zzz");
    }

    #[test]
    fn a_folder_walk_stops_on_the_flag() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let (bytes, files, complete) = crate::chat_tools::measure_for_test(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
            &cancel,
        );
        assert_eq!((bytes, files, complete), (0, 0, false));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
