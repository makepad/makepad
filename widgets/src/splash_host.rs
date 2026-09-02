//! Host-service bridge for Splash isolates.
//!
//! Mini-apps are sandboxed: no filesystem beyond their jail, no processes,
//! and no network unless granted. Real apps still need the things a phone
//! offers its apps — location, clipboard, notifications, pickers, messaging.
//! This module is the ONE doorway for all of that: an isolate's script asks,
//! the embedding host (the launcher) decides and answers. No policy lives
//! here; an undrained or unrecognized request simply never resolves, and a
//! host that says no answers `{ok: false}`.
//!
//! Registered in isolates as `mod.host` (bound as `host` by the Splash
//! prefix, like `fs`):
//!
//! ```text
//! let id = host.request("location.get", {}, fn(r) {
//!     if r.is_ok { use_it(r.data) } else { show(r.error) }
//! })
//! host.capabilities()   // host-pushed grant list, for adapting UI
//! host.has("location")  // membership test on the same list
//! ```
//!
//! The success field is `is_ok`, NOT `ok`: `ok` is a script keyword (the
//! ok-test operator), so `r.ok` doesn't parse as a field access.
//!
//! Mechanics mirror the storage jail (splash_storage.rs): all state lives
//! host-side in thread-locals keyed by the isolate's heap, where script code
//! can neither read nor forge it. A request serializes its args to JSON and
//! queues `{app_tag, heap_key, req_id, service, args_json}`; the app_tag is
//! host-assigned (`Splash::set_host_tag`), so the host can trust request
//! identity. The callback is held as a rooted `ScriptFnRef` until the host
//! answers via [`splash_host_respond`], which re-enters the owning isolate
//! under the standard entry budget and calls it with `{ok, data, error}`.
//! Dead isolates drop their queued requests and callbacks in the isolate GC.

use std::collections::HashMap;

use crate::makepad_draw::makepad_platform::makepad_script;
use crate::makepad_draw::makepad_platform::SignalToUI;
use crate::widget_async::{CxSplashVmExt, WIDGET_SCRIPT_INSTRUCTION_LIMIT};
use crate::makepad_draw::makepad_platform::Cx;
use makepad_script::*;

/// One script-originated service request, as drained by the host.
#[derive(Clone, Debug)]
pub struct SplashHostRequest {
    /// Host-assigned identity of the requesting isolate (empty for isolates
    /// the host never tagged, e.g. previews and validation runs).
    pub app_tag: String,
    /// The requesting isolate's heap key; pass back to [`splash_host_respond`].
    pub heap_key: usize,
    /// Per-process unique id; pass back to [`splash_host_respond`].
    pub req_id: u64,
    /// Service name as the script gave it, e.g. `"location.get"`.
    pub service: String,
    /// The request's args value serialized to JSON (`"null"` when absent).
    pub args_json: String,
    /// Whether this isolate's surface may raise user prompts (host-assigned,
    /// like the tag). Background surfaces — home-screen widget tiles — set
    /// this false so a permission-needing request FAILS instead of popping a
    /// consent dialog nobody asked for.
    pub may_prompt: bool,
}

#[derive(Default)]
struct HostBridge {
    /// heap key -> host-assigned app tag.
    tags: HashMap<usize, String>,
    /// heap keys whose surface must NOT raise prompts (absent = may prompt).
    no_prompt: std::collections::HashSet<usize>,
    /// heap key -> granted-capability names, for `host.capabilities()`.
    /// Informational only; enforcement happens host-side per request.
    caps: HashMap<usize, Vec<String>>,
    /// Requests awaiting the host's next drain.
    queue: Vec<SplashHostRequest>,
    /// (heap key, req id) -> the script callback awaiting an answer.
    callbacks: HashMap<(usize, u64), ScriptFnRef>,
    next_req_id: u64,
}

std::thread_local! {
    /// UI-thread-owned like the isolate registry itself.
    static BRIDGE: std::cell::RefCell<HostBridge> = Default::default();
}

/// Assigns (or clears) the host-trusted identity tag for an isolate.
pub(crate) fn set_tag_for_heap(heap_key: usize, tag: Option<String>) {
    BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        match tag {
            Some(tag) => {
                b.tags.insert(heap_key, tag);
            }
            None => {
                b.tags.remove(&heap_key);
            }
        }
    });
}

/// Replaces the granted-capability list `host.capabilities()` reports.
pub(crate) fn set_caps_for_heap(heap_key: usize, caps: Vec<String>) {
    BRIDGE.with(|b| {
        b.borrow_mut().caps.insert(heap_key, caps);
    });
}

/// Marks whether this isolate's surface may raise user prompts.
pub(crate) fn set_prompts_for_heap(heap_key: usize, may_prompt: bool) {
    BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        if may_prompt {
            b.no_prompt.remove(&heap_key);
        } else {
            b.no_prompt.insert(heap_key);
        }
    });
}

/// Drops all bridge state for reclaimed isolates (called from the isolate GC).
pub(crate) fn gc_bridge(dead_heaps: &[usize]) {
    BRIDGE.with(|b| {
        let mut b = b.borrow_mut();
        for heap in dead_heaps {
            b.tags.remove(heap);
            b.caps.remove(heap);
            b.no_prompt.remove(heap);
        }
        b.queue.retain(|r| !dead_heaps.contains(&r.heap_key));
        b.callbacks.retain(|(heap, _), _| !dead_heaps.contains(heap));
    });
}

/// Drains every request queued since the last call. The host owns answering
/// each one (or consciously dropping it); un-drained requests just wait.
pub fn take_splash_host_requests() -> Vec<SplashHostRequest> {
    BRIDGE.with(|b| std::mem::take(&mut b.borrow_mut().queue))
}

/// Answers one request: `Ok(json)` becomes `{ok: true, data: <parsed>}`,
/// `Err(msg)` becomes `{ok: false, error: msg}`. Returns what happened, so a
/// host can log undeliverable answers instead of wondering why an app hangs.
pub fn splash_host_respond(
    cx: &mut Cx,
    heap_key: usize,
    req_id: u64,
    result: Result<&str, &str>,
) -> SplashRespondOutcome {
    let callback = BRIDGE.with(|b| b.borrow_mut().callbacks.remove(&(heap_key, req_id)));
    let Some(callback) = callback else {
        return SplashRespondOutcome::NoCallback;
    };
    let Some(vm_id) = crate::widget_async::vm_for_heap(cx, heap_key) else {
        return SplashRespondOutcome::IsolateGone;
    };
    let mut delivered = true;
    let contained = crate::widget_async::contain_isolate_panic("host request callback", || {
    cx.with_script_vm_id(vm_id, |vm| {
        // Prove we landed in the heap this request came from before minting
        // anything in it. `vm_id` is looked up through a map; if that map is
        // ever wrong, building the answer object here would plant one heap's
        // values in another and the fault would surface later, in the GC, with
        // nothing left to point at the cause. One compare turns that into an
        // undeliverable answer, which callers already handle.
        if vm.bx.heap.heap_key() != heap_key {
            delivered = false;
            return;
        }
        let (ok, data, error) = match result {
            Ok(json) => {
                let mut parser = makepad_script::json::JsonParserThread::default();
                let data = parser.read_json(json, &mut vm.bx.heap);
                (true, data, NIL)
            }
            Err(msg) => (false, NIL, vm.new_string_with(|_vm, s| s.push_str(msg))),
        };
        let obj = vm.bx.heap.new_object();
        let trap = vm.bx.threads.cur().trap.pass();
        // `is_ok`, never `ok`: `ok` is a script keyword and unusable as a field.
        vm.bx.heap.set_value(obj, id!(is_ok).into(), ok.into(), trap);
        vm.bx.heap.set_value(obj, id!(data).into(), data, trap);
        vm.bx.heap.set_value(obj, id!(error).into(), error, trap);
        vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
            vm.call(callback.as_object().into(), &[obj.into()]);
        });
        // A callback that errors (or pauses and errors later on its own
        // thread) leaves its errors queued on the trap; nothing else drains
        // this entry path, so they used to vanish and a broken callback
        // looked like a delivered-but-inert answer.
        for err in vm.take_errors() {
            crate::makepad_draw::error!("splash host callback error: {err}");
        }
    });
    });
    if !contained {
        // The callback panicked; the panic was contained and that isolate is
        // suspect, but the host app carries on.
        return SplashRespondOutcome::Delivered;
    }
    if !delivered {
        return SplashRespondOutcome::IsolateGone;
    }
    SplashRespondOutcome::Delivered
}

/// What [`splash_host_respond`] managed to do with an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashRespondOutcome {
    /// The script callback ran (or at least was invoked; script errors are
    /// the script's own and land in the log).
    Delivered,
    /// The request carried no callback (fire-and-forget) — or was already
    /// answered once.
    NoCallback,
    /// The isolate died between asking and answering.
    IsolateGone,
}

/// Registers the `host` bridge module into an isolate VM.
pub fn script_mod(vm: &mut ScriptVm) {
    let host = vm.new_module(id!(host));

    vm.add_method(
        host,
        id_lut!(request),
        script_args_def!(service = NIL, args = NIL, on_result = NIL),
        |vm, args| {
            let service_val = script_value!(vm, args.service);
            let request_args = script_value!(vm, args.args);
            let on_result = script_value!(vm, args.on_result);

            let mut service = None;
            vm.string_with(service_val, |_vm, s| service = Some(s.to_string()));
            let Some(service) = service else {
                return script_err_invalid_args!(vm.trap(), "service must be a string");
            };

            let mut args_json = String::new();
            vm.bx.heap.to_json_inner(request_args, &mut args_json);

            let callback = if let Some(obj) = on_result.as_object() {
                Some(vm.bx.heap.new_fn_ref(obj))
            } else if on_result.is_nil() {
                None
            } else {
                return script_err_invalid_args!(vm.trap(), "on_result must be a fn or nil");
            };

            let heap_key = vm.bx.heap.heap_key();
            let req_id = BRIDGE.with(|b| {
                let mut b = b.borrow_mut();
                b.next_req_id += 1;
                let req_id = b.next_req_id;
                if let Some(callback) = callback {
                    b.callbacks.insert((heap_key, req_id), callback);
                }
                let app_tag = b.tags.get(&heap_key).cloned().unwrap_or_default();
                let may_prompt = !b.no_prompt.contains(&heap_key);
                b.queue.push(SplashHostRequest {
                    app_tag,
                    heap_key,
                    req_id,
                    service,
                    args_json,
                    may_prompt,
                });
                req_id
            });
            // The request was queued mid-script, after this pass's host code
            // already ran; a UI signal gets it drained promptly instead of on
            // the next input event.
            SignalToUI::set_ui_signal();
            (req_id as f64).into()
        },
    );

    vm.add_method(host, id_lut!(capabilities), script_args_def!(), |vm, _args| {
        let heap_key = vm.bx.heap.heap_key();
        let caps = BRIDGE.with(|b| b.borrow().caps.get(&heap_key).cloned().unwrap_or_default());
        let array = vm.bx.heap.new_array();
        vm.bx.heap.array_mut_mut_self_with(array, |heap, storage| {
            *storage = ScriptArrayStorage::ScriptValue(Default::default());
            if let ScriptArrayStorage::ScriptValue(vec) = storage {
                for cap in &caps {
                    vec.push_back(heap.new_string_from_str(cap));
                }
            }
        });
        array.into()
    });

    vm.add_method(host, id_lut!(has), script_args_def!(cap = NIL), |vm, args| {
        let cap_val = script_value!(vm, args.cap);
        let mut cap = None;
        vm.string_with(cap_val, |_vm, s| cap = Some(s.to_string()));
        let Some(cap) = cap else {
            return script_err_invalid_args!(vm.trap(), "capability must be a string");
        };
        let heap_key = vm.bx.heap.heap_key();
        BRIDGE
            .with(|b| {
                b.borrow()
                    .caps
                    .get(&heap_key)
                    .map(|caps| caps.iter().any(|c| c == &cap))
                    .unwrap_or(false)
            })
            .into()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_raw(heap_key: usize, req_id: u64, service: &str) {
        BRIDGE.with(|b| {
            b.borrow_mut().queue.push(SplashHostRequest {
                app_tag: String::new(),
                heap_key,
                req_id,
                service: service.to_string(),
                args_json: "null".to_string(),
                may_prompt: true,
            })
        });
    }

    #[test]
    fn take_drains_and_gc_drops_dead_state() {
        queue_raw(11, 1, "a");
        queue_raw(22, 2, "b");
        set_tag_for_heap(11, Some("app_a".into()));
        set_caps_for_heap(11, vec!["network".into()]);

        gc_bridge(&[11]);
        let reqs = take_splash_host_requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].heap_key, 22);
        assert!(take_splash_host_requests().is_empty());
        BRIDGE.with(|b| {
            let b = b.borrow();
            assert!(!b.tags.contains_key(&11));
            assert!(!b.caps.contains_key(&11));
        });
    }

    #[test]
    fn tags_and_caps_are_per_heap() {
        set_tag_for_heap(1, Some("one".into()));
        set_tag_for_heap(2, Some("two".into()));
        set_caps_for_heap(1, vec!["location".into()]);
        BRIDGE.with(|b| {
            let b = b.borrow();
            assert_eq!(b.tags.get(&1).unwrap(), "one");
            assert_eq!(b.tags.get(&2).unwrap(), "two");
            assert!(b.caps.get(&2).is_none());
        });
        set_tag_for_heap(1, None);
        BRIDGE.with(|b| assert!(!b.borrow().tags.contains_key(&1)));
    }
}
