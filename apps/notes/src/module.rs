//! notes as a module: the window manager seats one isolate, one NotesView.

use crate::view::NotesView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct NotesModule;

/// The one linked instance of the module description: immutable, no state.
pub static NOTES_MODULE: NotesModule = NotesModule;

impl AppModule for NotesModule {
    fn id(&self) -> &'static str {
        "notes"
    }

    fn label(&self) -> &'static str {
        "Notes"
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
            NotesView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<NotesView>() {
            view.set_storage(handles.storage);
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(NotesExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

struct NotesExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for NotesExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<NotesView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the notes app is gone"));
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &NOTES_MODULE;
        assert_eq!(m.id(), "notes");
        assert_eq!(m.label(), "Notes");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"folder":"notes"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
    }

    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("notes.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport { size: dvec2(320.0, 200.0) },
            replies,
        };
        let open = NOTES_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root, executor, shutdown } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            NOTES_MODULE.register(vm);
            let parts = NOTES_MODULE.create(vm, open, handles);
            assert!(vm.take_errors().is_empty(), "the isolate evaluated the view without errors");
            parts
        });
        assert!(root.borrow::<NotesView>().is_some(), "the root is a NotesView");

        let storage_b = cx.storage("notes.test.b");
        let (replies_b, _upstream_b) = ReplySink::pair();
        let handles_b = InstanceHandles {
            scope: InstanceScope::new(1, 2),
            storage: storage_b,
            viewport: Viewport { size: dvec2(320.0, 200.0) },
            replies: replies_b,
        };
        let open_b = NOTES_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root: root_b, executor: executor_b, shutdown: shutdown_b } =
            cx.with_script_vm_id_trusted(vm_id, |vm| {
                let parts = NOTES_MODULE.create(vm, open_b, handles_b);
                assert!(vm.take_errors().is_empty(), "the second root evaluated without errors");
                parts
            });

        {
            let mut a = root.borrow_mut::<NotesView>().unwrap();
            a.set_query_text("alpha".to_string());
        }
        {
            let b = root_b.borrow::<NotesView>().unwrap();
            assert!(b.query().is_empty(), "two roots do not share mutable state");
        }
        {
            let a = root.borrow::<NotesView>().unwrap();
            assert_eq!(a.query(), "alpha");
        }

        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown_b(vm));
        drop(root);
        drop(root_b);
        drop(executor);
        drop(executor_b);
        cx.free_splash_vm(vm_id);
    }
}
