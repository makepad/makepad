//! sheets as a module (aicontrol.md §3): the app the window manager seats
//! in a tile in-process, in an isolate of its own.
//!
//! `register` puts this crate's theme tokens (`mod.sheets`) and widget
//! family into the isolate the host prepared; `create` mints one
//! `MpSheets{}` root there and hands the host its tools: the same
//! `sheets.summary` the standalone binary answers over its port, read
//! from the root at call time. The module never touches a file, a socket
//! or a thread — a workbook arrives, when it does, as a handle the host
//! issued (the `file` argument of the open schema, unused until the host
//! has a picker to issue one).

use crate::view::MpSheets;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct SheetsModule;

/// The one linked instance of the module description: immutable, no state.
pub static SHEETS_MODULE: SheetsModule = SheetsModule;

impl AppModule for SheetsModule {
    fn id(&self) -> &'static str {
        "sheets"
    }

    fn label(&self) -> &'static str {
        "Sheets"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::theme::install(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1).arg("file", OpenArgKind::FileHandle, false)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            MpSheets {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        // The instance's disk is its storage jail: Open and Save keep CSV
        // there, on every host the same way (the browser's store on the web).
        if let Some(mut sheets) = root.borrow_mut::<MpSheets>() {
            sheets.set_storage(handles.storage);
        }
        InstanceParts {
            root: root.clone(),
            executor: Box::new(SheetsExecutor { root }),
            shutdown: Box::new(|_vm| {}),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

/// The instance's tools: `summary`, read from the root at call time.
struct SheetsExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for SheetsExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let summary = self
            .root
            .borrow::<MpSheets>()
            .map(|sheets| sheets.ai_summary(cx))
            .unwrap_or_else(|| "no sheet is open".to_string());
        ExecOutcome::Done(crate::ai::answer(call, || summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &SHEETS_MODULE;
        assert_eq!(m.id(), "sheets");
        assert_eq!(m.label(), "Sheets");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(schema.validate(r#"{"file":"/tmp/x.csv"}"#, &[]).is_err(), "a path is never an open argument");
    }
}
