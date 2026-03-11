use crate::*;
use makepad_script::{ScriptMod, ScriptThreadId, ScriptValue};
use makepad_script_std::{ScriptStd, ScriptTaskOnThreadCompletedHook, ScriptTaskPumpHook};

pub use makepad_script_std::{fs, run};

pub type CxScriptTaskOnThreadCompletedHook = ScriptTaskOnThreadCompletedHook;
pub type CxScriptTaskPumpHook = ScriptTaskPumpHook;

impl Cx {
    pub fn script_std(&self) -> &ScriptStd {
        &self.script_data.std
    }

    pub fn script_std_mut(&mut self) -> &mut ScriptStd {
        &mut self.script_data.std
    }

    fn with_script_std_vm<R>(
        &mut self,
        f: impl FnOnce(&mut Cx, &mut ScriptStd, &mut Option<Box<ScriptVmBase>>) -> R,
    ) -> R {
        let host = self as *mut Cx;
        let std = &mut self.script_data.std as *mut ScriptStd;
        let script_vm = &mut self.script_vm as *mut Option<Box<ScriptVmBase>>;
        unsafe { f(&mut *host, &mut *std, &mut *script_vm) }
    }

    pub fn with_vm_and_async<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::with_vm_and_async(host, std, script_vm, f)
        })
    }

    pub fn with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::with_vm(host, std, script_vm, f)
        })
    }

    pub fn with_vm_thread<R, F: FnOnce(&mut ScriptVm) -> R>(
        &mut self,
        thread_id: ScriptThreadId,
        f: F,
    ) -> R {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::with_vm_thread(host, std, script_vm, thread_id, f)
        })
    }

    pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::eval(host, std, script_vm, script_mod)
        })
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
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::handle_script_tasks(host, std, script_vm)
        });
    }

    pub(crate) fn handle_script_signals(&mut self) {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::pump(host, std, script_vm)
        });
    }

    pub(crate) fn handle_script_web_socket_event(&mut self, event: NetworkResponse) {
        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::handle_script_web_socket_event(host, std, script_vm, event)
        });
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
                        if let Some(res) = resources.iter().find(|r| r.handle == handle) {
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
                                    .handle_http_response(request_id, body.clone());
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

        self.with_script_std_vm(|host, std, script_vm| {
            makepad_script_std::handle_script_network_events(host, std, script_vm, responses)
        });
    }
}
