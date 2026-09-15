//! finance as a module (aicontrol.md §3): the app the phone window manager
//! seats in a tile in-process, in an isolate of its own.
//!
//! `register` puts this crate's palette, chart widget family and root type
//! into the isolate the host prepared; `create` mints one `Finance{}` root
//! there, points it at the makepad home's ledger file, and hands the host
//! its one read-only tool. The module never touches a file itself beyond
//! that: `Finance::start` (run lazily on the root's first draw, the same
//! way the standalone window starts) opens the database at the path
//! `create` set — there is no worker to start or stop, so shutdown is a
//! no-op and the database closes when the root is dropped.

use crate::view::Finance;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct FinanceModule;

/// The one linked instance of the module description: immutable, no state.
pub static FINANCE_MODULE: FinanceModule = FinanceModule;

impl AppModule for FinanceModule {
    fn id(&self) -> &'static str {
        "finance"
    }

    fn label(&self) -> &'static str {
        "Finance"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::theme::install(vm);
        crate::chart::script_mod(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        // Unused today: CSV import still goes through a native file dialog
        // (src/native.rs) on every non-wasm, non-demo build, module
        // included. Once the host issues file handles, an open can carry
        // one straight to a pending import instead.
        OpenSchema::new(1).arg("file", OpenArgKind::FileHandle, false)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, _handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            Finance {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<Finance>() {
            // NOT the storage jail (`handles.storage`, for small settings):
            // the ledger is a plain SQLite file, the same file format the
            // standalone window reads, kept under the shared makepad home
            // so it persists across runs the same way on a phone as it
            // does on this machine. A first run with an empty database
            // seeds the generated demo household, exactly like the
            // desktop's first run.
            view.set_db_path(makepad_widgets::makepad_platform::home::makepad_home().join("finance").join("finance.db"));
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(FinanceExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

/// The instance's one tool, answered against the root at call time.
struct FinanceExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for FinanceExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow_mut::<Finance>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "finance is gone"));
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &FINANCE_MODULE;
        assert_eq!(m.id(), "finance");
        assert_eq!(m.label(), "Finance");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(schema.validate(r#"{"file":"/tmp/statement.csv"}"#, &[]).is_err(), "a path is never an open argument");
    }

    #[test]
    fn the_module_registers_and_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            FINANCE_MODULE.register(vm);
            let value = script_eval!(vm, {
                use mod.widgets.*
                Finance {}
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });
        assert!(root.borrow::<Finance>().is_some(), "the root is a Finance");
        for id in [ids!(body), ids!(sidebar), ids!(content), ids!(tabbar), ids!(screens)] {
            assert!(!root.widget(&cx, id).is_empty(), "missing shared UI id {id:?}");
        }
    }
}
