//! files as a module (aicontrol.md §3): the app the window manager seats
//! in a tile in-process, in an isolate of its own.
//!
//! `register` puts this crate's widget families and the root type into the
//! isolate the host prepared; `create` mints one `FilesView{}` root there
//! and hands the host its tools: the same tools the standalone binary
//! answers over its port, answered on the root at call time.

use crate::view::FilesView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct FilesModule;

/// The one linked instance of the module description: immutable, no state.
pub static FILES_MODULE: FilesModule = FilesModule;

#[cfg(feature = "dynamic-module")]
makepad_app_module::export_app_module!(FILES_MODULE);

impl AppModule for FilesModule {
    fn id(&self) -> &'static str {
        "files"
    }

    fn label(&self) -> &'static str {
        "Files"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            FilesView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<FilesView>() {
            view.set_reply_sink(handles.replies);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(FilesExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<FilesView>() {
                    view.shutdown(vm.cx_mut());
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &[]
    }
}

/// The instance's tools, answered against the root at call time.
struct FilesExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for FilesExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::chat_tools::service_manifest()
    }

    fn execute(&mut self, cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        match self.root.borrow_mut::<FilesView>() {
            Some(mut view) => view.ai_execute(cx, call),
            None => ExecOutcome::Done(makepad_ai_services::wire::ToolResult::unavailable(
                &call.call_id,
                "the file browser is gone",
            )),
        }
    }

    fn cancel(&mut self, _cx: &mut Cx, call_id: &str) {
        if let Some(mut view) = self.root.borrow_mut::<FilesView>() {
            view.ai_cancel(call_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &FILES_MODULE;
        assert_eq!(m.id(), "files");
        assert_eq!(m.label(), "Files");
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"path":"/tmp/x"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
    }
}
