//! route as a module (aicontrol.md §3): the app the window manager seats
//! in a tile in-process, in an isolate of its own — the phone's map.
//!
//! `register` puts this crate's widget families into the isolate the host
//! prepared; `create` mints one `RouteView{}` root there, gives it the
//! host's storage jail, and hands the host its tools: the same tools the
//! standalone binary answers over its port, run on the root at call time.
//! The module never touches a file, a socket or a thread itself — the map
//! root is the view's own resolution (maps_root.rs), the loaders and the
//! radar poller run on the platform's pool and workers, and each ends when
//! the view is torn down.

use crate::view::RouteView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct RouteModule;

/// The one linked instance of the module description: immutable, no state.
pub static ROUTE_MODULE: RouteModule = RouteModule;

impl AppModule for RouteModule {
    fn id(&self) -> &'static str {
        "route"
    }

    fn label(&self) -> &'static str {
        "Route"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::side_panel::script_mod(vm);
        crate::chrome::script_mod(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            RouteView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<RouteView>() {
            view.set_storage(handles.storage);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(RouteExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<RouteView>() {
                    view.shutdown(vm.cx_mut());
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage", "location", "net"]
    }
}

/// The instance's tools, run on the root at call time. Every route tool
/// answers synchronously; the map mirrors what it did.
struct RouteExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for RouteExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow_mut::<RouteView>()
            .map(|mut view| crate::ai::answer(cx, &mut view, call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the map is gone"));
        ExecOutcome::Done(result)
    }

    fn chat_open(&mut self, cx: &mut Cx, open: bool) {
        if open {
            if let Some(mut view) = self.root.borrow_mut::<RouteView>() {
                view.close_assistant_panel(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &ROUTE_MODULE;
        assert_eq!(m.id(), "route");
        assert_eq!(m.label(), "Route");
        assert!(m.capabilities().contains(&"storage"));
        assert!(m.capabilities().contains(&"location"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(schema.validate(r#"{"path":"/tmp/maps"}"#, &[]).is_err(), "a path is never an open argument");
    }

    #[test]
    fn the_module_registers_and_mints_its_root_in_a_fresh_isolate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            ROUTE_MODULE.register(vm);
            let value = script_eval!(vm, {
                use mod.widgets.*
                RouteView {}
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });
        assert!(root.borrow::<RouteView>().is_some(), "the root is a RouteView");
        for id in [ids!(map), ids!(layers_panel), ids!(assistant_panel), ids!(prompt_input), ids!(testmap_panel)] {
            assert!(!root.widget(&cx, id).is_empty(), "missing shared UI id {id:?}");
        }
    }
}
