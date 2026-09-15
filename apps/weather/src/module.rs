//! weather as a module (aicontrol.md §3): the app the window manager
//! seats in a tile in-process, in an isolate of its own — a home-screen
//! resident, exactly like `photos` and `sheets`.
//!
//! `register` puts this crate's widget family (the sky artwork) and the
//! root type into the isolate the host prepared; `create` mints one
//! `WeatherView{}` root there, gives it the host's storage jail for the
//! chosen city, and hands the host its one tool: the same tool the
//! standalone binary answers over its port, answered on the root at call
//! time. The module never opens a socket or spawns a thread itself — the
//! forecast fetch rides the platform's own HTTP request API, the same
//! one a standalone window uses, so it works identically hosted.

use crate::view::WeatherView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct WeatherModule;

/// The one linked instance of the module description: immutable, no state.
pub static WEATHER_MODULE: WeatherModule = WeatherModule;

impl AppModule for WeatherModule {
    fn id(&self) -> &'static str {
        "weather"
    }

    fn label(&self) -> &'static str {
        "Weather"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::sky::script_mod(vm);
        crate::hourly::script_mod(vm);
        crate::daily::script_mod(vm);
        crate::parts::script_mod(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            WeatherView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<WeatherView>() {
            // The instance's disk is its storage jail: the chosen city
            // persists there, on every host the same way (the browser's
            // store on the web).
            view.set_storage(handles.storage);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(WeatherExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<WeatherView>() {
                    view.shutdown(vm.cx_mut());
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage", "net"]
    }
}

/// The instance's one tool, read from the root at call time.
struct WeatherExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for WeatherExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<WeatherView>()
            .map(|view| crate::ai::answer(&view, call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the forecast is gone"));
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::widget_async::{enter_isolate, leave_isolate};

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &WEATHER_MODULE;
        assert_eq!(m.id(), "weather");
        assert_eq!(m.label(), "Weather");
        assert!(m.capabilities().contains(&"storage"));
        assert!(m.capabilities().contains(&"net"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(schema.validate(r#"{"lat":52.37,"lon":4.9}"#, &[]).is_err(), "a location is never inferred or passed in");
    }

    /// The whole contract without a window manager: an instance in an
    /// isolate of its own, its root switching faces the same way a
    /// standalone window does, teardown in the host's order.
    #[test]
    fn the_module_mints_its_root_in_a_fresh_isolate_and_switches_faces() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("weather.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport { size: dvec2(320.0, 200.0) },
            replies,
        };
        let open = WEATHER_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root, executor, shutdown } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            WEATHER_MODULE.register(vm);
            let parts = WEATHER_MODULE.create(vm, open, handles);
            assert!(vm.take_errors().is_empty(), "the isolate evaluated the view without errors");
            parts
        });
        assert!(root.borrow::<WeatherView>().is_some(), "the root is a WeatherView");

        // Full is the default face; the host asks for the tile the same
        // way it asks a standalone window, over `Event::Custom`. The
        // isolate has no network (`alloc_splash_vm_with_network(false)`,
        // matching how the WM allocates every module today), so the
        // fetch itself never completes here — only the face switch and
        // the labelled-city summary are asserted.
        let entry = enter_isolate(&mut cx, vm_id);
        root.handle_event(&mut cx, &Event::Custom(HostedViewMode::Tile.to_json()), &mut Scope::empty());
        leave_isolate(&mut cx, entry);
        {
            let view = root.borrow::<WeatherView>().unwrap();
            assert_eq!(view.face(&cx), HostedViewMode::Tile, "the module root switched faces from the host's message");
            let summary = view.ai_summary();
            assert!(summary.contains("Amsterdam"), "the default city is labelled, not inferred: {summary}");
        }

        // The host's order: shutdown in the isolate, the refs, the isolate.
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        drop(root);
        drop(executor);
        cx.free_splash_vm(vm_id);
    }
}
