use crate::*;
use makepad_script::{ScriptMod, ScriptThreadId, ScriptValue};
pub use makepad_script_std::ScriptStd;
use makepad_script_std::{ScriptTaskOnThreadCompletedHook, ScriptTaskPumpHook};

pub use makepad_script_std::{fs, net, run};

pub type CxScriptTaskOnThreadCompletedHook = ScriptTaskOnThreadCompletedHook;
pub type CxScriptTaskPumpHook = ScriptTaskPumpHook;

impl Cx {
    pub fn script_std(&self) -> &ScriptStd {
        &self.script_data.std
    }

    pub fn script_std_mut(&mut self) -> &mut ScriptStd {
        &mut self.script_data.std
    }

    /// Whether the script VM is currently held (`take()`n) by an enclosing
    /// `with_vm`/`eval` on this thread, i.e. calling `with_vm` now would be
    /// re-entrant and panic. Lets a call site that can degrade gracefully
    /// (defer, skip) check first, or use [`Cx::try_with_vm`].
    pub fn is_script_vm_held(&self) -> bool {
        self.script_vm.is_none()
    }

    #[track_caller]
    pub fn with_vm_and_async<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        let _vm_guard = makepad_script_std::VmHolderGuard::enter(
            self.script_vm.is_some(),
            std::panic::Location::caller(),
        );
        makepad_script_std::with_vm_and_async(self, f)
    }

    #[track_caller]
    pub fn with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        let _vm_guard = makepad_script_std::VmHolderGuard::enter(
            self.script_vm.is_some(),
            std::panic::Location::caller(),
        );
        makepad_script_std::with_vm(self, f)
    }

    /// Like [`Cx::with_vm`], but returns `None` instead of panicking when the
    /// VM is already held (swapped off) by an enclosing `with_vm`/`eval`.
    pub fn try_with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> Option<R> {
        makepad_script_std::try_with_vm(self, f)
    }

    #[track_caller]
    pub fn with_vm_thread<R, F: FnOnce(&mut ScriptVm) -> R>(
        &mut self,
        thread_id: ScriptThreadId,
        f: F,
    ) -> R {
        let _vm_guard = makepad_script_std::VmHolderGuard::enter(
            self.script_vm.is_some(),
            std::panic::Location::caller(),
        );
        makepad_script_std::with_vm_thread(self, thread_id, f)
    }

    #[track_caller]
    pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {
        let _vm_guard = makepad_script_std::VmHolderGuard::enter(
            self.script_vm.is_some(),
            std::panic::Location::caller(),
        );
        makepad_script_std::eval(self, script_mod)
    }

    pub fn add_script_task_on_thread_completed_hook(
        &mut self,
        hook: CxScriptTaskOnThreadCompletedHook,
    ) {
        makepad_script_std::add_script_task_on_thread_completed_hook(self.script_std_mut(), hook);
    }

    pub fn add_script_task_pump_hook(&mut self, hook: CxScriptTaskPumpHook) {
        makepad_script_std::add_script_task_pump_hook(self.script_std_mut(), hook);
    }

    pub fn queue_script_thread_resume(&mut self, thread_id: ScriptThreadId) {
        makepad_script_std::queue_script_thread_resume(self.script_std_mut(), thread_id);
    }

    pub fn set_script_task_trace(&mut self, enabled: bool) {
        makepad_script_std::set_script_task_trace(self.script_std_mut(), enabled);
    }

    pub(crate) fn handle_script_tasks(&mut self) {
        makepad_script_std::handle_script_tasks(self);
    }

    pub(crate) fn handle_script_signals(&mut self) {
        makepad_script_std::pump(self);
    }

    pub(crate) fn handle_script_web_socket_event(&mut self, event: NetworkResponse) {
        makepad_script_std::handle_script_web_socket_event(self, event);
    }

    #[allow(unused)]
    pub(crate) fn handle_script_network_events(&mut self, responses: &[NetworkResponse]) {
        for response in responses {
            let request_id = match response {
                NetworkResponse::HttpResponse { request_id, .. }
                | NetworkResponse::HttpStreamChunk { request_id, .. }
                | NetworkResponse::HttpStreamComplete { request_id, .. }
                | NetworkResponse::HttpError { request_id, .. }
                | NetworkResponse::HttpProgress { request_id, .. } => *request_id,
                NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => continue,
            };

            if self.script_data.resources.is_http_resource(request_id) {
                let resource_info = {
                    let path = self
                        .script_data
                        .resources
                        .http_resources
                        .iter()
                        .find(|r| r.request_id == request_id)
                        .map(|r| r.abs_path.as_str());
                    if let Some(path) = path {
                        let resources = self.script_data.resources.resources.borrow();
                        if let Some(res) = resources.iter().find(|r| r.abs_path == path) {
                            format!(
                                "abs_path={} web_url={:?} dependency_path={:?}",
                                res.abs_path, res.web_url, res.dependency_path
                            )
                        } else {
                            format!("path={:?} (resource entry not found)", path)
                        }
                    } else {
                        "unknown resource".to_string()
                    }
                };
                match response {
                    NetworkResponse::HttpResponse { response: res, .. } => {
                        if let Some(body) = res.get_body() {
                            if (200..300).contains(&res.status_code) {
                                self.script_data
                                    .resources
                                    .handle_http_response(request_id, body.to_vec());
                            } else {
                                crate::log!(
                                    "Script resource HTTP load failed: status={} {}",
                                    res.status_code,
                                    resource_info
                                );
                                self.script_data.resources.handle_http_error(
                                    request_id,
                                    format!("HTTP error: status {}", res.status_code),
                                );
                            }
                        } else {
                            crate::log!(
                                "Script resource HTTP load failed: empty response body {}",
                                resource_info
                            );
                            self.script_data.resources.handle_http_error(
                                request_id,
                                "HTTP error: empty response body".to_string(),
                            );
                        }
                        self.redraw_all();
                    }
                    NetworkResponse::HttpError { error: err, .. } => {
                        crate::log!(
                            "Script resource HTTP request error: message={} {}",
                            err.message,
                            resource_info
                        );
                        self.script_data.resources.handle_http_error(
                            request_id,
                            format!("HTTP request error: {}", err.message),
                        );
                    }
                    _ => {}
                }
            }
        }

        makepad_script_std::handle_script_network_events(self, responses);
    }

    /// Run the script network handlers against whichever VM is currently *installed*
    /// on this `Cx` (`Cx::script_vm` + `Cx::script_data.std`).
    ///
    /// Splash isolates call this after swapping themselves onto `Cx`. They must never
    /// run a VM that is passed alongside `Cx` as a separate `&mut`: `ScriptVm::with_cx`
    /// parks the executing `bx` into `cx.script_vm` and takes it back out, so a VM
    /// executing while a *different* VM sits in that slot would overwrite (and drop) it.
    ///
    /// Unlike [`Cx::handle_script_network_events`], this does not resolve `http_resource`
    /// loads — those live on `Cx::script_data.resources` and are handled once, for the
    /// app VM, before the event is dispatched to the widget tree.
    pub fn handle_script_network_events_for_current_vm(&mut self, responses: &[NetworkResponse]) {
        makepad_script_std::handle_script_network_events(self, responses);
    }
}

#[cfg(test)]
mod unwind_tests {
    use crate::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn heap_key(cx: &mut Cx) -> usize {
        cx.with_vm(|vm| vm.bx.heap.heap_key())
    }

    /// A closure handed to `with_vm` panics inside a `catch_unwind` (the
    /// platforms' event catchers): the VM is parked back on `Cx` — the
    /// same heap, not a fresh one — and the next `with_vm` works.
    #[test]
    fn a_panic_inside_with_vm_leaves_the_vm_parked_on_cx() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let key = heap_key(&mut cx);
        let caught = catch_unwind(AssertUnwindSafe(|| cx.with_vm(|_vm| panic!("a native panicked under with_vm"))));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held(), "the VM is back on Cx after the unwind");
        assert_eq!(heap_key(&mut cx), key, "the same heap came back");

        // The same through `eval`, `try_with_vm` and a thread entry.
        let caught = catch_unwind(AssertUnwindSafe(|| cx.try_with_vm(|_vm| panic!("under try_with_vm"))));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held());
        let caught = catch_unwind(AssertUnwindSafe(|| cx.with_vm_and_async(|_vm| panic!("under with_vm_and_async"))));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held());
        assert_eq!(heap_key(&mut cx), key);
        // A re-entrant call is still diagnosed, so the holder bookkeeping
        // came back with the VM too.
        let caught = catch_unwind(AssertUnwindSafe(|| cx.with_vm(|vm| vm.cx_mut().with_vm(|_| ()))));
        assert!(caught.is_err(), "a raw cx_mut re-entry is still refused");
        assert!(!cx.is_script_vm_held());
        assert_eq!(heap_key(&mut cx), key);
    }

    /// The panic starts under `with_cx_mut`, where the VM sits on `Cx` and
    /// the `ScriptVm` holds a placeholder: the real VM is what survives.
    #[test]
    fn a_panic_under_with_cx_mut_keeps_the_real_vm() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let key = heap_key(&mut cx);
        let caught = catch_unwind(AssertUnwindSafe(|| {
            cx.with_vm(|vm| vm.with_cx_mut(|_cx| panic!("a native panicked under with_cx_mut")))
        }));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held());
        assert_eq!(heap_key(&mut cx), key, "the parked VM, not the placeholder, is on Cx");
        // Nested: the inner `with_vm` under `with_cx_mut` is where it starts.
        let caught = catch_unwind(AssertUnwindSafe(|| {
            cx.with_vm(|vm| vm.with_cx_mut(|cx| cx.with_vm(|_vm| panic!("two levels down"))))
        }));
        assert!(caught.is_err());
        assert!(!cx.is_script_vm_held());
        assert_eq!(heap_key(&mut cx), key);
        // And the VM still runs script afterwards.
        cx.with_vm(|vm| {
            let value = vm.eval(script! { 1 + 2 });
            assert_eq!(value.as_f64(), Some(3.0));
        });
    }
}
