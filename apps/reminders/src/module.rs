//! reminders as a module the window manager seats in-process.

use crate::view::RemindersView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct RemindersModule;

pub static REMINDERS_MODULE: RemindersModule = RemindersModule;

impl AppModule for RemindersModule {
    fn id(&self) -> &'static str {
        "reminders"
    }

    fn label(&self) -> &'static str {
        "Reminders"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1)
    }

    fn create(
        &self,
        vm: &mut ScriptVm,
        _open: ValidatedOpen,
        handles: InstanceHandles,
    ) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            RemindersView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<RemindersView>() {
            view.set_storage(handles.storage);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(RemindersExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<RemindersView>() {
                    view.shutdown(vm.cx_mut());
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

struct RemindersExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for RemindersExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<RemindersView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(
                    &call.call_id,
                    "reminders are gone",
                )
            });
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &REMINDERS_MODULE;
        assert_eq!(m.id(), "reminders");
        assert_eq!(m.label(), "Reminders");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"file":"/tmp/x.json"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
        let tools = crate::ai::manifest();
        assert_eq!(tools.id, "reminders");
        assert!(tools.validate().is_ok());
        assert_eq!(tools.tools.len(), 2);
    }

    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("reminders.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport {
                size: dvec2(320.0, 200.0),
            },
            replies,
        };
        let open = REMINDERS_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts {
            root,
            executor,
            shutdown,
        } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            REMINDERS_MODULE.register(vm);
            let parts = REMINDERS_MODULE.create(vm, open, handles);
            assert!(
                vm.take_errors().is_empty(),
                "the isolate evaluated the view without errors"
            );
            parts
        });
        assert!(
            root.borrow::<RemindersView>().is_some(),
            "the root is a RemindersView"
        );
        root.handle_event(&mut cx, &Event::Startup, &mut Scope::empty());
        assert!(root.borrow::<RemindersView>().unwrap().timer_is_active());
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        assert!(!root.borrow::<RemindersView>().unwrap().timer_is_active());
        drop(root);
        drop(executor);
        cx.free_splash_vm(vm_id);
    }
}
