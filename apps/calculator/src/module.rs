//! calculator as a module: the app the window manager seats in a tile
//! in-process, in an isolate of its own.

use crate::view::CalculatorView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct CalculatorModule;

/// The one linked instance of the module description: immutable, no state.
pub static CALCULATOR_MODULE: CalculatorModule = CalculatorModule;

impl AppModule for CalculatorModule {
    fn id(&self) -> &'static str {
        "calculator"
    }

    fn label(&self) -> &'static str {
        "Calculator"
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
            CalculatorView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<CalculatorView>() {
            view.set_storage(handles.storage);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(CalculatorExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<CalculatorView>() {
                    vm.with_cx_mut(|cx| view.shutdown(cx));
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

struct CalculatorExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for CalculatorExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<CalculatorView>()
            .map(|view| crate::ai::answer(view.doc(), call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the calculator is gone")
            });
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::{ServiceCall, ToolOutcome};

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &CALCULATOR_MODULE;
        assert_eq!(m.id(), "calculator");
        assert_eq!(m.label(), "Calculator");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"city":"Amsterdam"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
    }

    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("calculator.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport {
                size: dvec2(320.0, 200.0),
            },
            replies,
        };
        let open = CALCULATOR_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts {
            root,
            executor,
            shutdown,
        } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            CALCULATOR_MODULE.register(vm);
            let parts = CALCULATOR_MODULE.create(vm, open, handles);
            assert!(
                vm.take_errors().is_empty(),
                "the isolate evaluated the view without errors"
            );
            parts
        });
        assert!(
            root.borrow::<CalculatorView>().is_some(),
            "the root is a CalculatorView"
        );
        {
            let view = root.borrow::<CalculatorView>().unwrap();
            assert!(!view.ai_summary().is_empty());
            assert_eq!(view.doc().session.source, "0");
        }

        let storage2 = cx.storage("calculator.test.b");
        let (replies2, _upstream2) = ReplySink::pair();
        let handles2 = InstanceHandles {
            scope: InstanceScope::new(1, 2),
            storage: storage2,
            viewport: Viewport {
                size: dvec2(320.0, 200.0),
            },
            replies: replies2,
        };
        let open2 = CALCULATOR_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts {
            root: root2,
            executor: executor2,
            shutdown: shutdown2,
        } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            CALCULATOR_MODULE.create(vm, open2, handles2)
        });

        let call = ServiceCall {
            call_id: "t1".into(),
            tool: "eval".into(),
            args: r#"{"expression":"2+3*4"}"#.into(),
        };
        let mut exec = executor;
        let ExecOutcome::Done(result) = exec.execute(&mut cx, &call) else {
            panic!("executor should finish");
        };
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert!(result.data.contains("14"), "{}", result.data);
        {
            let view = root.borrow::<CalculatorView>().unwrap();
            assert_eq!(view.doc().session.source, "0", "eval must not mutate the document");
        }
        {
            let mut a = root.borrow_mut::<CalculatorView>().unwrap();
            a.apply_command_for_test(crate::model::Command::Digit(7));
            assert_eq!(a.doc().session.source, "7");
        }
        {
            let a = root.borrow::<CalculatorView>().unwrap();
            let b = root2.borrow::<CalculatorView>().unwrap();
            assert_eq!(a.doc().session.source, "7");
            assert_eq!(b.doc().session.source, "0");
        }

        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown2(vm));
        drop(root);
        drop(root2);
        drop(exec);
        drop(executor2);
        cx.free_splash_vm(vm_id);
    }
}
