//! Mail as a module: the window manager seats one isolate, one `MailView`.

use crate::view::MailView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct MailModule;

pub static MAIL_MODULE: MailModule = MailModule;

impl AppModule for MailModule {
    fn id(&self) -> &'static str {
        "mail"
    }

    fn label(&self) -> &'static str {
        "Mail"
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
            MailView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<MailView>() {
            view.set_storage(handles.storage);
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(MailExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

struct MailExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for MailExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<MailView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "mail is gone"));
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &MAIL_MODULE;
        assert_eq!(m.id(), "mail");
        assert_eq!(m.label(), "Mail");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"file":"/tmp/x"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
    }

    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage_a = cx.storage("mail.test.a");
        let storage_b = cx.storage("mail.test.b");
        let (replies_a, _up_a) = ReplySink::pair();
        let (replies_b, _up_b) = ReplySink::pair();
        let open = MAIL_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root: root_a, executor: exec_a, shutdown: shut_a } =
            cx.with_script_vm_id_trusted(vm_id, |vm| {
                MAIL_MODULE.register(vm);
                let parts = MAIL_MODULE.create(
                    vm,
                    open,
                    InstanceHandles {
                        scope: InstanceScope::new(1, 1),
                        storage: storage_a,
                        viewport: Viewport { size: dvec2(1240.0, 800.0) },
                        replies: replies_a,
                    },
                );
                assert!(vm.take_errors().is_empty(), "the isolate evaluated MailView without errors");
                parts
            });
        let open_b = MAIL_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root: root_b, executor: exec_b, shutdown: shut_b } =
            cx.with_script_vm_id_trusted(vm_id, |vm| {
                MAIL_MODULE.create(
                    vm,
                    open_b,
                    InstanceHandles {
                        scope: InstanceScope::new(1, 2),
                        storage: storage_b,
                        viewport: Viewport { size: dvec2(402.0, 780.0) },
                        replies: replies_b,
                    },
                )
            });
        assert!(root_a.borrow::<MailView>().is_some(), "root A is a MailView");
        assert!(root_b.borrow::<MailView>().is_some(), "root B is a MailView");
        {
            let mut a = root_a.borrow_mut::<MailView>().unwrap();
            a.set_query_for_test("harbor");
        }
        assert_eq!(root_a.borrow::<MailView>().unwrap().query(), "harbor");
        assert_eq!(
            root_b.borrow::<MailView>().unwrap().query(),
            "",
            "independent roots do not share data"
        );
        cx.with_script_vm_id_trusted(vm_id, |vm| {
            shut_a(vm);
            shut_b(vm);
        });
        drop(root_a);
        drop(root_b);
        drop(exec_a);
        drop(exec_b);
        cx.free_splash_vm(vm_id);
    }
}
