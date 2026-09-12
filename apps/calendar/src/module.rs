//! calendar as a module: the window manager seats one instance per isolate.

use crate::view::CalendarView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct CalendarModule;

pub static CALENDAR_MODULE: CalendarModule = CalendarModule;

impl AppModule for CalendarModule {
    fn id(&self) -> &'static str {
        "calendar"
    }

    fn label(&self) -> &'static str {
        "Calendar"
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
            CalendarView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<CalendarView>() {
            view.set_storage(handles.storage);
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(CalendarExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

struct CalendarExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for CalendarExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<CalendarView>()
            .map(|view| crate::ai::answer(&view, call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the calendar is gone")
            });
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClockSnapshot;
    use makepad_civil_time::from_ymd;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &CALENDAR_MODULE;
        assert_eq!(m.id(), "calendar");
        assert_eq!(m.label(), "Calendar");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(
            schema.validate(r#"{"date":"2026-09-09"}"#, &[]).is_err(),
            "the schema takes no arguments at all"
        );
    }

    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("calendar.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport {
                size: dvec2(320.0, 200.0),
            },
            replies,
        };
        let open = CALENDAR_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts {
            root,
            executor,
            shutdown,
        } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            CALENDAR_MODULE.register(vm);
            let parts = CALENDAR_MODULE.create(vm, open, handles);
            assert!(
                vm.take_errors().is_empty(),
                "the isolate evaluated the view without errors"
            );
            parts
        });
        assert!(
            root.borrow::<CalendarView>().is_some(),
            "the root is a CalendarView"
        );

        let storage2 = cx.storage("calendar.test.b");
        let (replies2, _upstream2) = ReplySink::pair();
        let handles2 = InstanceHandles {
            scope: InstanceScope::new(1, 2),
            storage: storage2,
            viewport: Viewport {
                size: dvec2(320.0, 200.0),
            },
            replies: replies2,
        };
        let open2 = CALENDAR_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts {
            root: root2,
            executor: executor2,
            shutdown: shutdown2,
        } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            let parts = CALENDAR_MODULE.create(vm, open2, handles2);
            assert!(vm.take_errors().is_empty());
            parts
        });
        {
            let mut a = root.borrow_mut::<CalendarView>().unwrap();
            let mut b = root2.borrow_mut::<CalendarView>().unwrap();
            a.seed_for_test(from_ymd(2026, 9, 9));
            b.seed_for_test(from_ymd(2024, 2, 29));
            assert_ne!(
                a.document().unwrap().seed_anchor,
                b.document().unwrap().seed_anchor,
                "two roots have independent state"
            );
            let _ = ClockSnapshot {
                today: from_ymd(2026, 9, 9),
                minute: 0,
            };
        }

        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown2(vm));
        drop(root);
        drop(root2);
        drop(executor);
        drop(executor2);
        cx.free_splash_vm(vm_id);
    }
}
