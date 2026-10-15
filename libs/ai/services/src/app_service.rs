//! The process's `app` service (makepad-remote `app/1`): the AI tools of every in-process
//! service in [`ServiceRegistry::process`], for a caller on the process's endpoint (the code
//! agent, `makepad-remote app <pid> tools|call`), after the built-in services every app has
//! (`ui`, `edit`, `logs`, `inspect`: the platform's remote commands). One channel is one request ([`AppRequest`]);
//! the answer is [`AppLine`]s, one per message, and the channel ends with the last. A call goes
//! to the app's port like any call from its own chat and is answered in the app's
//! `handle_event` (the registry wakes the UI thread); closing the channel cancels it. The caller
//! confirms risky calls before it makes them: the port's own validation is the app's floor.

use crate::engine::core::CallActivity;
use crate::engine::{RegistryUp, ServiceRegistry};
use crate::wire::*;
use makepad_remote::program::ServiceSink;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often a waiting call drains the links (only while a call is in flight).
const POLL: Duration = Duration::from_millis(10);

/// Serves `app` on the process's endpoint (once, from [`ServiceRegistry::process`]).
pub(crate) fn serve() {
    makepad_platform::remote::register_service("app", 1, open, close);
}

fn cancels() -> &'static Mutex<HashMap<usize, Arc<AtomicBool>>> {
    static C: OnceLock<Mutex<HashMap<usize, Arc<AtomicBool>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The producer's end of one channel.
struct Sink {
    sink: ServiceSink,
    ctx: usize,
}

impl Sink {
    fn line(&self, line: &AppLine) -> bool {
        self.raw(&line.to_json())
    }
    fn raw(&self, json: &str) -> bool {
        (self.sink)(self.ctx as *mut std::ffi::c_void, 0, json.as_ptr(), json.len())
    }
    fn end(self) {
        (self.sink)(self.ctx as *mut std::ffi::c_void, 1, std::ptr::null(), 0);
    }
    fn refuse(self, why: &str) {
        (self.sink)(self.ctx as *mut std::ffi::c_void, 2, why.as_ptr(), why.len());
    }
}

extern "C" fn open(args: *const u8, len: usize, sink: ServiceSink, ctx: *mut std::ffi::c_void) -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let args = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(args, len) }).into_owned();
    let out = Sink { sink, ctx: ctx as usize };
    let request = match AppRequest::parse(&args) {
        Ok(r) => r,
        Err(why) => {
            out.refuse(&why);
            return 0;
        }
    };
    let handle = NEXT.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    cancels().lock().unwrap().insert(handle, cancelled.clone());
    let spawned = std::thread::Builder::new().name("makepad-app-service".into()).spawn(move || {
        answer(&ServiceRegistry::process(), request, &out, &cancelled);
        out.end();
        cancels().lock().unwrap().remove(&handle);
    });
    if spawned.is_err() {
        cancels().lock().unwrap().remove(&handle);
        // the closure (and the sink in it) is gone with the failed spawn: end the channel here
        let why = "cannot start the app service thread";
        sink(ctx, 2, why.as_ptr(), why.len());
        return 0;
    }
    handle
}

extern "C" fn close(handle: usize) {
    if let Some(flag) = cancels().lock().unwrap().get(&handle) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Answers one request into `out`; `cancelled` is set when the client closes the channel.
fn answer(registry: &ServiceRegistry, request: AppRequest, out: &Sink, cancelled: &AtomicBool) {
    // the frames waiting on the links (registrations, contexts) are current before anything is read
    registry.pump_remote();
    match request {
        AppRequest::List => {
            // the built-in services (ui, edit, logs, inspect) first, then the app's own
            for line in makepad_platform::remote::builtin_instances() {
                if !out.raw(&line) {
                    return;
                }
            }
            for line in list(registry) {
                if !out.line(&line) {
                    return;
                }
            }
        }
        AppRequest::Context => {
            out.line(&AppLine::Context { text: registry.dynamic_context() });
        }
        AppRequest::Call { service, tool, instance, args, caller } => {
            if let Some(line) = makepad_platform::remote::builtin_call("app0", &service, &tool, &args) {
                out.raw(&line);
                return;
            }
            let result = call(registry, &service, &tool, instance.as_deref(), args, caller, cancelled, &mut |note, permille| {
                if !out.line(&AppLine::Progress { note: note.to_string(), permille }) {
                    cancelled.store(true, Ordering::Relaxed);
                }
            });
            out.line(&AppLine::Result(result));
        }
    }
}

/// Every running instance with its manifest, then the apps the host can start.
pub fn list(registry: &ServiceRegistry) -> Vec<AppLine> {
    let mut lines = Vec::new();
    for info in registry.services() {
        if info.connected {
            let endpoint = EndpointId(info.endpoint.clone());
            if let (Some(meta), Some(manifest)) = (registry.meta(&endpoint), registry.manifest(&endpoint)) {
                lines.push(AppLine::Instance { endpoint, meta, manifest });
            }
        } else if info.launchable {
            lines.push(AppLine::Launchable { id: info.id, label: info.label });
        }
    }
    lines
}

/// One call, start to end: routed to the instance, its progress handed to `progress`, its
/// result returned (a cancel or a gone instance as such a result). Silence is
/// reported as progress state; it never cancels the work.
pub fn call(
    registry: &ServiceRegistry,
    service: &str,
    tool: &str,
    instance: Option<&str>,
    args: String,
    caller: Option<String>,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(&str, u16),
) -> ToolResult {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let call_id = format!("app{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let Some(endpoint) = registry.pick_instance(service, instance) else {
        return ToolResult::unavailable(&call_id, format!("no running instance of `{service}` here"));
    };
    // a named instance is that one or none (the registry's pick falls back to the focused one)
    if let Some(wanted) = instance {
        let named = endpoint.as_str() == wanted
            || registry.meta(&endpoint).is_some_and(|m| m.display_name.eq_ignore_ascii_case(wanted.trim()));
        if !named {
            return ToolResult::unavailable(&call_id, format!("no instance `{wanted}` of `{service}` here"));
        }
    }
    let Some(manifest) = registry.manifest(&endpoint) else {
        return ToolResult::unavailable(&call_id, "the app went away");
    };
    if manifest.tool(tool).is_none() {
        let names: Vec<&str> = manifest.tools.iter().map(|t| t.name.as_str()).collect();
        return ToolResult::refused(&call_id, format!("`{service}` has no tool `{tool}` (it has: {})", names.join(", ")));
    }
    let Some(rx) = registry.call_remote(&endpoint, ServiceCall { call_id: call_id.clone(), tool: tool.to_string(), args, caller }) else {
        return ToolResult::unavailable(&call_id, "the app went away");
    };
    let started = Instant::now();
    let mut activity = CallActivity::new(0.0);
    let mut last_permille = 0;
    loop {
        registry.pump_remote();
        match rx.recv_timeout(POLL) {
            Ok(RegistryUp::Result(_, result)) => return result,
            Ok(RegistryUp::Progress { note, permille, .. }) => {
                activity.progress(started.elapsed().as_secs_f64());
                last_permille = permille;
                progress(&note, permille);
            }
            Ok(RegistryUp::Message { .. }) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                registry.end_remote(&call_id);
                return ToolResult::unavailable(&call_id, "the app went away");
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            registry.end_remote(&call_id);
            registry.send(&endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            return ToolResult::cancelled(&call_id);
        }
        if activity.poll(started.elapsed().as_secs_f64()) {
            progress("not responding — Stop to cancel", last_permille);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{AiServicePort, PortEvent};

    fn calc() -> ServiceManifest {
        ServiceManifest::new("calc", "Calculator", "Adds.").with_tool(ToolDef::new(
            "add",
            "a+b",
            r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}}}"#,
            Risk::Read,
        ))
    }

    /// An app's UI thread: answers each call on its port, `slow` ones after a progress note and
    /// never (so the caller can cancel them).
    fn app_thread(mut port: AiServicePort, stop: Arc<AtomicBool>, seen: Arc<Mutex<Vec<PortEvent>>>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for ev in port.test_drain() {
                    if let PortEvent::Call(c) = &ev {
                        if c.args.contains("slow") {
                            port.progress(&c.call_id, "thinking", 100);
                        } else {
                            port.reply(ToolResult::ok(&c.call_id, format!("sum of {}", c.args), "added"));
                        }
                    }
                    seen.lock().unwrap().push(ev);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    }

    #[test]
    fn list_call_and_cancel_through_the_registry() {
        let registry = ServiceRegistry::new();
        let (port, link) = AiServicePort::in_process(calc()).unwrap();
        let endpoint = registry.register(link, "in this window", None).unwrap();
        registry.mark_launchable("notes", "Notes");
        let (stop, seen) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(Vec::new())));
        let app = app_thread(port, stop.clone(), seen.clone());

        let lines = list(&registry);
        assert!(matches!(&lines[0], AppLine::Instance { endpoint: e, manifest, .. } if e == &endpoint && manifest.id == "calc"), "{lines:?}");
        assert!(matches!(&lines[1], AppLine::Launchable { id, .. } if id == "notes"));
        // the lines survive the wire
        assert_eq!(AppLine::parse(&lines[0].to_json()).as_ref(), Some(&lines[0]));

        let never = AtomicBool::new(false);
        let r = call(&registry, "calc", "add", None, r#"{"a":7,"b":8}"#.into(), None, &never, &mut |_, _| {});
        assert_eq!(r.outcome, ToolOutcome::Ok, "{r:?}");
        assert!(r.text.contains("\"a\":7"));
        let r = call(&registry, "calc", "mul", None, "{}".into(), None, &never, &mut |_, _| {});
        assert_eq!(r.outcome, ToolOutcome::Refused);
        let r = call(&registry, "nope", "add", None, "{}".into(), None, &never, &mut |_, _| {});
        assert_eq!(r.outcome, ToolOutcome::Unavailable);

        // a slow call reports progress, and closing the channel cancels it at the app
        let cancel = Arc::new(AtomicBool::new(false));
        let flip = cancel.clone();
        let mut notes = Vec::new();
        let r = call(&registry, "calc", "add", Some(endpoint.as_str()), r#"{"slow":1}"#.into(), None, &cancel, &mut |note, _| {
            notes.push(note.to_string());
            flip.store(true, Ordering::Relaxed);
        });
        assert_eq!(r.outcome, ToolOutcome::Cancelled);
        assert_eq!(notes, vec!["thinking".to_string()]);
        let t = Instant::now();
        while !seen.lock().unwrap().iter().any(|e| matches!(e, PortEvent::Cancel { .. })) {
            assert!(t.elapsed() < Duration::from_secs(5), "the app never heard the cancel");
            std::thread::sleep(Duration::from_millis(5));
        }
        // the conversation's own calls are not taken by the remote caller's drains
        registry.send(&endpoint, ServiceDown::Call(ServiceCall { call_id: "core1".into(), tool: "add".into(), args: "{}".into(), caller: None }));
        let t = Instant::now();
        let mut got = Vec::new();
        while got.is_empty() {
            assert!(t.elapsed() < Duration::from_secs(5));
            registry.pump_remote();
            got = registry.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(&got[0], RegistryUp::Result(_, r) if r.call_id == "core1"));
        stop.store(true, Ordering::Relaxed);
        app.join().unwrap();
    }

    #[test]
    fn the_built_in_services_are_valid_manifests() {
        let lines = makepad_platform::remote::builtin_instances();
        assert_eq!(lines.len(), 4, "ui, edit, logs, inspect");
        let mut tools = 0;
        for l in &lines {
            match AppLine::parse(l) {
                Some(AppLine::Instance { endpoint, manifest, .. }) => {
                    assert_eq!(endpoint.as_str(), manifest.id);
                    manifest.validate().unwrap_or_else(|e| panic!("{}: {e}", manifest.id));
                    tools += manifest.tools.len();
                }
                other => panic!("not an instance line: {other:?} from {l}"),
            }
        }
        assert!(tools >= 45, "{tools}");
        // a built-in call's answer reads as a result line
        let refused = makepad_platform::remote::builtin_call("c1", "ui", "frobnicate", "{}").unwrap();
        assert!(matches!(AppLine::parse(&refused), Some(AppLine::Result(r)) if r.outcome == ToolOutcome::Failed));
        assert!(makepad_platform::remote::builtin_call("c1", "calculator", "eval", "{}").is_none());
    }

    #[test]
    fn requests_parse_and_print() {
        let r = AppRequest::parse("call calc.add e3\n{\"a\":1}").unwrap();
        assert_eq!(r, AppRequest::Call { service: "calc".into(), tool: "add".into(), instance: Some("e3".into()), args: "{\"a\":1}".into(), caller: None });
        assert_eq!(AppRequest::parse(&r.to_args()).unwrap(), r);
        let c = AppRequest::parse("call calc.add caller=tok1\n{}").unwrap();
        assert_eq!(c, AppRequest::Call { service: "calc".into(), tool: "add".into(), instance: None, args: "{}".into(), caller: Some("tok1".into()) });
        assert_eq!(AppRequest::parse(&c.to_args()).unwrap(), c);
        assert_eq!(AppRequest::parse("list").unwrap(), AppRequest::List);
        assert!(AppRequest::parse("call add").is_err());
        assert!(AppRequest::parse("drop").is_err());
    }
}
