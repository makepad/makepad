//! The process's `app` service (makepad-remote `app/1`): the AI tools of every in-process
//! service in [`ServiceRegistry::process`], for a caller on the process's endpoint (the code
//! agent, `makepad-remote app <pid> tools|call`), after the built-in services every app has
//! (`ui`, `edit`, `logs`, `inspect`: the platform's remote commands). One channel is one request ([`AppRequest`]);
//! the answer is [`AppLine`]s, one per message, and the channel ends with the last. A call goes
//! to the app's port like any call from its own chat and is answered in the app's
//! `handle_event` (the registry wakes the UI thread); closing the channel cancels it. The caller
//! waits for the person in the addressed app before a destructive tool is dispatched.

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
    makepad_platform::remote::register_service("app", 1, open, close, None);
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
/// When no instance is connected yet, appends the platform's age note as `AppLine::Progress`.
pub fn list(registry: &ServiceRegistry) -> Vec<AppLine> {
    list_at(registry, makepad_platform::remote::app_age())
}

fn list_at(registry: &ServiceRegistry, age: Duration) -> Vec<AppLine> {
    let mut lines = Vec::new();
    let mut has_connected = false;
    for info in registry.services() {
        if info.connected {
            let endpoint = EndpointId(info.endpoint.clone());
            if let (Some(meta), Some(manifest)) = (registry.meta(&endpoint), registry.manifest(&endpoint)) {
                lines.push(AppLine::Instance { endpoint, meta, manifest });
                has_connected = true;
            }
        } else if info.launchable {
            lines.push(AppLine::Launchable { id: info.id, label: info.label });
        }
    }
    if !has_connected {
        lines.push(AppLine::Progress {
            note: makepad_platform::remote::no_app_tools_note_for(age),
            permille: 0,
        });
    }
    lines
}

fn no_instance_reason(service: &str, running: &[String], age: Duration) -> String {
    if running.is_empty() {
        format!(
            "no running instance of `{service}` here: {}",
            makepad_platform::remote::no_app_tools_note_for(age)
        )
    } else {
        format!(
            "no running instance of `{service}` here; running: {} (`tools` lists their tools)",
            running.join(", ")
        )
    }
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
    // services promised and not registered yet (a window registers its product's after its
    // first frame): the call waits for them rather than finding none
    let picked = registry.pick_instance(service, instance).or_else(|| {
        registry.wait_expected(cancelled);
        registry.pick_instance(service, instance)
    });
    let Some(endpoint) = picked else {
        // the services that do run, so a caller who named the wrong one (an action of a tool
        // taken for a service) sees where the tools are
        let mut running: Vec<String> = Vec::new();
        for info in registry.services().into_iter().filter(|s| s.connected) {
            if !running.contains(&info.id) {
                running.push(info.id);
            }
        }
        return ToolResult::unavailable(
            &call_id,
            no_instance_reason(service, &running, makepad_platform::remote::app_age()),
        );
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
    let Some(def) = manifest.tool(tool) else {
        let names: Vec<&str> = manifest.tools.iter().map(|t| t.name.as_str()).collect();
        return ToolResult::refused(&call_id, format!("`{service}` has no tool `{tool}` (it has: {})", names.join(", ")));
    };
    if cancelled.load(Ordering::Acquire) { return ToolResult::cancelled(&call_id) }
    let call = ServiceCall { call_id: call_id.clone(), tool: tool.to_string(), args, caller, origin: CallOrigin::Remote };
    let held = def.risk.max(registry.risk_floor(&manifest.id)) >= Risk::Destructive;
    let mut waiting = held.then(|| {
        let name = registry.meta(&endpoint).map_or(manifest.label, |m| m.display_name);
        format!("waiting for the person to confirm in {name}")
    });
    let (rx, mut original) = if held {
        (registry.confirm_remote(&endpoint, &call), Some(call))
    } else {
        (registry.call_remote(&endpoint, call), None)
    };
    let Some(rx) = rx else {
        return ToolResult::unavailable(&call_id, "the app went away");
    };
    let started = Instant::now();
    let mut activity = CallActivity::new(0.0);
    let mut last_permille = 0;
    let mut last_note = started;
    if let Some(note) = &waiting { progress(note, 0); }
    loop {
        if cancelled.load(Ordering::Acquire) {
            registry.end_remote(&call_id);
            registry.send(&endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            return ToolResult::cancelled(&call_id);
        }
        registry.pump_remote();
        match rx.recv_timeout(POLL) {
            Ok(RegistryUp::Result(_, result)) => return result,
            Ok(RegistryUp::Progress { note, permille, .. }) => {
                activity.progress(started.elapsed().as_secs_f64());
                last_permille = permille;
                progress(&note, permille);
            }
            Ok(RegistryUp::ConfirmAnswer { allow, .. }) => {
                if cancelled.load(Ordering::Acquire) { continue }
                let Some(call) = original.take() else { continue };
                if !allow {
                    registry.end_remote(&call_id);
                    return ToolResult::refused(&call_id, "the person declined");
                }
                if !registry.dispatch_allowed(&endpoint, call) {
                    registry.end_remote(&call_id);
                    return ToolResult::unavailable(&call_id, "the app went away");
                }
                waiting = None;
                activity.progress(started.elapsed().as_secs_f64());
            }
            Ok(RegistryUp::Message { .. }) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                registry.end_remote(&call_id);
                return ToolResult::unavailable(&call_id, "the app went away");
            }
        }
        if let Some(note) = &waiting {
            // A waiting person keeps the MCP channel active across its 15-minute silence limit.
            if last_note.elapsed() >= Duration::from_secs(60) {
                progress(note, 0);
                last_note = Instant::now();
            }
        } else if activity.poll(started.elapsed().as_secs_f64()) {
            progress("not responding — Stop to cancel", last_permille);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{AiServicePort, PortEvent};

    #[test]
    fn platform_no_app_tools_progress_line_parses_as_app_line_progress() {
        use makepad_platform::remote::{builtin_instances, no_app_tools_note_for, no_app_tools_progress_line};
        for (seconds, elapsed) in [(4, "4 s ago"), (7200, "2 h ago")] {
            let age = Duration::from_secs(seconds);
            let note = no_app_tools_note_for(age);
            assert!(note.contains(elapsed), "{note}");
            assert_eq!(
                AppLine::parse(&no_app_tools_progress_line(age)),
                Some(AppLine::Progress { note, permille: 0 }),
            );
        }
        assert_eq!(builtin_instances().len(), 4);
    }

    #[test]
    fn empty_registry_list_and_call_report_app_age_until_a_service_connects() {
        let registry = ServiceRegistry::new();
        let age = Duration::from_secs(4);
        let progress = AppLine::Progress {
            note: makepad_platform::remote::no_app_tools_note_for(age),
            permille: 0,
        };
        assert_eq!(list_at(&registry, age), [progress.clone()]);
        assert_eq!(
            no_instance_reason("town", &[], age),
            "no running instance of `town` here: no AI tools of its own open yet (app started 4 s ago); an app may open them once it has started — try again, or see ui/logs"
        );
        assert_eq!(
            no_instance_reason("town", &["calc".into()], age),
            "no running instance of `town` here; running: calc (`tools` lists their tools)"
        );
        let result = call(&registry, "town", "join", None, "{}".into(), None, &AtomicBool::new(false), &mut |_, _| {});
        assert_eq!(result.outcome, ToolOutcome::Unavailable);
        assert!(result.text.starts_with("no running instance of `town` here: no AI tools of its own open yet (app started "), "{}", result.text);
        assert!(result.text.ends_with("try again, or see ui/logs"), "{}", result.text);

        registry.mark_launchable("notes", "Notes");
        let notes = AppLine::Launchable { id: "notes".into(), label: "Notes".into() };
        assert_eq!(list_at(&registry, age), [notes.clone(), progress]);

        let (_port, link) = AiServicePort::in_process(calc()).unwrap();
        let endpoint = registry.register(link, "in this window", None).unwrap();
        let lines = list_at(&registry, age);
        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[0], AppLine::Instance { endpoint: id, manifest, .. } if id == &endpoint && manifest.id == "calc"));
        assert_eq!(lines[1], notes);
    }

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
        registry.send(&endpoint, ServiceDown::Call(ServiceCall { call_id: "core1".into(), tool: "add".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation }));
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

    struct CancelOnDrop<'a>(&'a AtomicBool);
    impl Drop for CancelOnDrop<'_> {
        fn drop(&mut self) { self.0.store(true, Ordering::Release); }
    }

    #[test]
    fn remote_confirmation_requires_the_addressed_endpoint_and_cancel_withdraws() {
        use crate::port::ServiceLink;
        let registry = ServiceRegistry::new();
        let (link, target) = ServiceLink::pair(calc());
        let endpoint = registry.register(link, "", None).unwrap();
        let (link, other) = ServiceLink::pair(calc());
        let other_id = registry.register(link, "", None).unwrap();
        registry.set_risk_floor("calc", Risk::Destructive);
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (waiting, note) = std::sync::mpsc::channel();
            let target_id = endpoint.as_str();
            let (reg, stop) = (&registry, &cancel);
            let task = scope.spawn(move || call(reg, "calc", "add", Some(target_id), "{}".into(),
                None, stop, &mut |text, _| waiting.send(text.to_string()).unwrap()));
            let _cleanup = CancelOnDrop(&cancel);
            assert_eq!(note.recv().unwrap(), "waiting for the person to confirm in Calculator");
            let first = target.down.recv().unwrap();
            let ServiceDown::Confirm { call_id, tool, args } = first.msg else { panic!("a raw receiver must see only a prompt") };
            assert_eq!((tool.as_str(), args.as_str()), ("add", "{}"));
            other.up.send(HostedUp { from: Some(endpoint.clone()), msg: ServiceUp::ConfirmAnswer {
                call_id: call_id.clone(), allow: true,
            }}).unwrap();
            registry.pump_remote();
            // Pump synchronously and inspect the pending phase, so this checks the wrong
            // endpoint even if the waiting thread has not been scheduled since the reply.
            assert!(registry.confirm_pending(&endpoint, &call_id));
            assert!(!registry.confirm_pending(&other_id, &call_id));
            assert!(target.down.try_recv().is_err());
            cancel.store(true, Ordering::Release);
            assert_eq!(task.join().unwrap().outcome, ToolOutcome::Cancelled);
            assert_eq!(target.down.recv().unwrap().msg, ServiceDown::Cancel { call_id: call_id.clone() });
            target.up.send(HostedUp { from: None, msg: ServiceUp::ConfirmAnswer { call_id, allow: true } }).unwrap();
            assert!(registry.pump().is_empty());
            assert!(target.down.try_recv().is_err(), "late Allow cannot dispatch a cancelled call");
        });
    }

    #[test]
    fn a_remote_tool_waits_for_the_port_and_only_allow_delivers_the_original_call() {
        use makepad_platform::{Cx, Event};
        let registry = ServiceRegistry::new();
        let mut manifest = calc();
        for (name, risk) in [("act", Risk::Act), ("request", Risk::Request), ("change", Risk::Destructive)] {
            manifest = manifest.with_tool(ToolDef::new(name, "A test tool.", r#"{"type":"object"}"#, risk));
        }
        let (mut port, link) = AiServicePort::in_process(manifest).unwrap();
        let endpoint = registry.register(link, "", None).unwrap();
        let mut cx = Cx::new(Box::new(|_, _| {}));
        port.handle_event(&mut cx, &Event::Signal);
        let args = r#"{ "first": "one\ntwo", "n": 4 }"#;
        for (i, name) in ["add", "act", "request", "change", "change", "change"].into_iter().enumerate() {
            let cancel = AtomicBool::new(false);
            std::thread::scope(|scope| {
                let task = scope.spawn(|| call(&registry, "calc", name, None, args.into(), Some("caller".into()),
                    &cancel, &mut |_, _| {}));
                let _cleanup = CancelOnDrop(&cancel);
                let mut answered = false;
                let mut delivered = None;
                while !task.is_finished() {
                    for event in port.handle_event(&mut cx, &Event::Signal) {
                        let PortEvent::Call(c) = event else { panic!("a held cancel escaped the port") };
                        assert!(i < 3 || (i == 5 && answered), "only local Allow releases a held call");
                        assert_eq!(c, ServiceCall { call_id: c.call_id.clone(), tool: name.into(),
                            caller: Some("caller".into()), args: args.into(), origin: CallOrigin::Remote });
                        delivered = Some(c.call_id.clone());
                        port.reply(ToolResult::ok(c.call_id, "", ""));
                    }
                    if let Some(id) = cx.confirmation_head().get() {
                        assert!(i >= 3);
                        let (title, details) = cx.confirmation_text(id).unwrap();
                        assert_eq!(title, "A program outside Calculator asks to run change");
                        assert_eq!(details, "first: one\\ntwo\nn: 4");
                        if i == 3 { cancel.store(true, Ordering::Release); }
                        else { answered = cx.answer_confirmation(id, i == 5); }
                    }
                    std::thread::yield_now();
                }
                let result = task.join().unwrap();
                match i {
                    3 => assert_eq!(result.outcome, ToolOutcome::Cancelled),
                    4 => { assert_eq!(result.outcome, ToolOutcome::Refused); assert_eq!(result.text, "the person declined"); }
                    _ => assert_eq!(Some(result.call_id), delivered),
                }
                assert!(port.handle_event(&mut cx, &Event::Signal).is_empty());
                assert_eq!(cx.confirmation_head().get(), None);
            });
        }
        let call = ServiceCall { call_id: "chat1".into(), tool: "change".into(), args: args.into(),
            caller: None, origin: CallOrigin::Conversation };
        assert!(registry.send(&endpoint, ServiceDown::Call(call.clone())));
        assert_eq!(port.handle_event(&mut cx, &Event::Signal), vec![PortEvent::Call(call)]);
    }

    #[test]
    fn the_built_in_services_are_valid_manifests() {
        use makepad_strict_json::Value;

        let lines = makepad_platform::remote::builtin_instances();
        assert_eq!(lines.len(), 4, "ui, edit, logs, inspect");
        let mut tools = 0;
        let mut edit_dispatchers = 0;
        for l in &lines {
            match AppLine::parse(l) {
                Some(AppLine::Instance { endpoint, manifest, .. }) => {
                    assert_eq!(endpoint.as_str(), manifest.id);
                    manifest.validate().unwrap_or_else(|e| panic!("{}: {e}", manifest.id));
                    for tool in &manifest.tools {
                        let name = format!("{}.{}", manifest.id, tool.name);
                        let schema = crate::wire::parse_tool_json(&tool.parameters).expect(&name);
                        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"), "{name}");
                        assert!(matches!(schema.get("properties"), Some(Value::Obj(_))), "{name} lacks properties");
                        for keyword in ["oneOf", "anyOf", "allOf"] {
                            assert!(schema.get(keyword).is_none(), "{name} has top-level {keyword}");
                        }
                        if matches!(name.as_str(), "edit.design" | "edit.tweak_op") {
                            let ops = schema.get("properties").and_then(|p| p.get("op"))
                                .and_then(|op| op.get("enum")).and_then(Value::as_arr).expect(&name);
                            if name == "edit.design" {
                                assert_eq!(ops.len(), makepad_remote::params::DESIGN.len(), "{name}");
                            } else {
                                assert!(ops.iter().all(|op| op.as_str().is_some_and(|s| !s.starts_with("design_"))), "{name}");
                            }
                            edit_dispatchers += 1;
                        }
                    }
                    tools += manifest.tools.len();
                }
                other => panic!("not an instance line: {other:?} from {l}"),
            }
        }
        assert_eq!(edit_dispatchers, 2, "edit.design and edit.tweak_op must both be advertised");
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
        assert_eq!(crate::wire::MAX_ARGS_BYTES, makepad_remote::frame::MAX_PAYLOAD);
        #[cfg(feature = "engine")]
        assert_eq!(crate::wire::MAX_ARGS_BYTES, makepad_ai_hub::chat_wire::MAX_TOOL_JSON_BYTES);
        for bytes in [100 * 1024, 192 * 1024] {
            let args = makepad_strict_json::obj(vec![("source", makepad_strict_json::s("x".repeat(bytes)))]).to_json();
            let request = AppRequest::Call { service: "calc".into(), tool: "add".into(), instance: None,
                args: args.clone(), caller: Some("tok1".into()) };
            let payload = format!("app\n{}", request.to_args());
            let mut buf = Vec::new();
            makepad_remote::frame::write_frame(&mut buf, 1, makepad_remote::frame::kind::OPEN, 0, payload.as_bytes()).unwrap();
            let frame = makepad_remote::frame::FrameReader::new(Box::new(std::io::Cursor::new(buf))).next().unwrap();
            let payload = String::from_utf8(frame.payload).unwrap();
            assert_eq!(AppRequest::parse(payload.strip_prefix("app\n").unwrap()).unwrap(), request);
            let hosted = crate::wire::HostedDown { to: None, msg: crate::wire::ServiceDown::Call(
                crate::wire::ServiceCall { call_id: "c1".into(), tool: "add".into(), args, caller: Some("tok1".into()), origin: CallOrigin::Remote }) };
            assert_eq!(crate::wire::HostedDown::parse(&hosted.to_json()), Some(hosted));
        }
        assert_eq!(AppRequest::parse("list").unwrap(), AppRequest::List);
        assert!(AppRequest::parse("call add").is_err());
        assert!(AppRequest::parse("drop").is_err());
    }
}
