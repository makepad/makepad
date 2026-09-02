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
                    let handle = self
                        .script_data
                        .resources
                        .http_resources
                        .iter()
                        .find(|r| r.request_id == request_id)
                        .map(|r| r.handle);
                    if let Some(handle) = handle {
                        let resources = self.script_data.resources.resources.borrow();
                        if let Some(res) = resources.iter().find(|r| r.has_handle(handle)) {
                            format!(
                                "abs_path={} web_url={:?} dependency_path={:?}",
                                res.abs_path, res.web_url, res.dependency_path
                            )
                        } else {
                            format!("handle={:?} (resource entry not found)", handle)
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
