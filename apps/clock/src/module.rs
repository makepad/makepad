//! clock as a module (aicontrol.md §3): the app the window manager seats
//! in a tile in-process, in an isolate of its own — a home-screen
//! resident, exactly like `photos` and `sheets`.
//!
//! `register` puts this crate's widget families (the analog face, the
//! time wheel, the alarm list) and the root type into the isolate the
//! host prepared; `create` mints one `ClockView{}` root there, gives it
//! the host's storage jail for the alarm book, and hands the host its one
//! tool: the same tool the standalone binary answers over its port,
//! answered on the root at call time. The module never touches a file, a
//! socket or a thread — the alarm sound and the tick timer are native Cx
//! resources the view starts itself and stops in `shutdown`.

use crate::view::ClockView;
use makepad_ai_services::wire::{ServiceCall, ServiceManifest};
use makepad_app_module::*;
use makepad_widgets::*;

pub struct ClockModule;

/// The one linked instance of the module description: immutable, no state.
pub static CLOCK_MODULE: ClockModule = ClockModule;

impl AppModule for ClockModule {
    fn id(&self) -> &'static str {
        "clock"
    }

    fn label(&self) -> &'static str {
        "Clock"
    }

    fn register(&self, vm: &mut ScriptVm) {
        crate::face::script_mod(vm);
        crate::wheel::script_mod(vm);
        crate::alarm_list::script_mod(vm);
        crate::digits::script_mod(vm);
        crate::tabs::script_mod(vm);
        crate::ring::script_mod(vm);
        crate::laps::script_mod(vm);
        crate::view::script_mod(vm);
    }

    fn open_schema(&self) -> OpenSchema {
        OpenSchema::new(1)
    }

    fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, handles: InstanceHandles) -> InstanceParts {
        let value = script_eval!(vm, {
            use mod.widgets.*
            ClockView {}
        });
        let root = WidgetRef::script_from_value(vm, value);
        if let Some(mut view) = root.borrow_mut::<ClockView>() {
            // The instance's disk is its storage jail: the alarm book
            // persists there, on every host the same way (the browser's
            // store on the web).
            view.set_storage(handles.storage);
        }
        let shutdown_root = root.clone();
        InstanceParts {
            root: root.clone(),
            executor: Box::new(ClockExecutor { root }),
            shutdown: Box::new(move |vm| {
                if let Some(mut view) = shutdown_root.borrow_mut::<ClockView>() {
                    view.shutdown(vm.cx_mut());
                }
            }),
        }
    }

    fn capabilities(&self) -> &'static [&'static str] {
        &["storage"]
    }
}

/// The instance's one tool, read from the root at call time.
struct ClockExecutor {
    root: WidgetRef,
}

impl ServiceExecutor for ClockExecutor {
    fn manifest(&self) -> ServiceManifest {
        crate::ai::manifest()
    }

    fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
        let result = self
            .root
            .borrow::<ClockView>()
            .map(|view| crate::ai::answer(&view, call))
            .unwrap_or_else(|| makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the clock is gone"));
        ExecOutcome::Done(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::widget_async::{enter_isolate, leave_isolate};

    #[test]
    fn the_module_describes_itself_and_opens_empty() {
        let m = &CLOCK_MODULE;
        assert_eq!(m.id(), "clock");
        assert_eq!(m.label(), "Clock");
        assert!(m.capabilities().contains(&"storage"));
        let schema = m.open_schema();
        assert_eq!(schema.version, 1);
        assert!(schema.empty_open().is_ok(), "no argument is required");
        assert!(schema.validate(r#"{"city":"Amsterdam"}"#, &[]).is_err(), "the schema takes no arguments at all");
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
        let storage = cx.storage("clock.test");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport { size: dvec2(320.0, 200.0) },
            replies,
        };
        let open = CLOCK_MODULE.open_schema().empty_open().unwrap();
        let InstanceParts { root, executor, shutdown } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            CLOCK_MODULE.register(vm);
            let parts = CLOCK_MODULE.create(vm, open, handles);
            assert!(vm.take_errors().is_empty(), "the isolate evaluated the view without errors");
            parts
        });
        assert!(root.borrow::<ClockView>().is_some(), "the root is a ClockView");

        // Full is the default face; the host asks for the tile the same
        // way it asks a standalone window, over `Event::Custom`.
        let entry = enter_isolate(&mut cx, vm_id);
        root.handle_event(&mut cx, &Event::Custom(HostedViewMode::Tile.to_json()), &mut Scope::empty());
        leave_isolate(&mut cx, entry);
        {
            let view = root.borrow::<ClockView>().unwrap();
            assert_eq!(view.face(&cx), HostedViewMode::Tile, "the module root switched faces from the host's message");
            assert!(!view.ai_summary().is_empty(), "the clock has something to say once it has started");
        }

        // The host's order: shutdown in the isolate, the refs, the isolate.
        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        drop(root);
        drop(executor);
        cx.free_splash_vm(vm_id);
    }

    /// The window manager's way into an isolate: the appearance's style
    /// sheet installed, the widgets re-evaluated under it, the WM palette
    /// applied, then the module registered — and on an appearance change
    /// the same again plus a re-apply of the root from its type default.
    /// The title and the digital time must draw in the sheet's text role
    /// every time: dark ink on the light sheet, light ink on the dark one.
    #[test]
    fn the_root_takes_the_hosts_appearance_ink() {
        use crate::digits::TabularLabel;
        use makepad_widgets::desktop_style::{self, DesktopStyle, StyleSheet};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(makepad_widgets::script_mod);
        let vm_id = cx.alloc_splash_vm_with_network(false);
        let storage = cx.storage("clock.test.ink");
        let (replies, _upstream) = ReplySink::pair();
        let handles = InstanceHandles {
            scope: InstanceScope::new(1, 1),
            storage,
            viewport: Viewport { size: dvec2(402.0, 780.0) },
            replies,
        };
        let open = CLOCK_MODULE.open_schema().empty_open().unwrap();
        let sheet = |dark: bool| StyleSheet::load_with_appearance(DesktopStyle::Ios, dark);
        let InstanceParts { mut root, executor, shutdown } = cx.with_script_vm_id_trusted(vm_id, |vm| {
            makepad_widgets::makepad_platform::script::res::script_mod(vm);
            desktop_style::install(vm, sheet(false));
            vm.with_reload(|vm| {
                makepad_widgets::widgets_mod(vm);
                desktop_style::apply_widgets(vm);
            });
            makepad_wm_theme::apply(vm);
            CLOCK_MODULE.register(vm);
            let parts = CLOCK_MODULE.create(vm, open, handles);
            assert!(vm.take_errors().is_empty(), "the isolate evaluated the view without errors");
            parts
        });
        // The two inks the phone showed wrong: the title and the digital time.
        let inks = |cx: &mut Cx, root: &WidgetRef| -> (Vec4f, Vec4f) {
            let entry = enter_isolate(cx, vm_id);
            let view = root.borrow::<ClockView>().unwrap();
            let title = view.label(cx, ids!(title)).borrow().map(|l| l.draw_text.color).unwrap();
            let time = view.widget(cx, ids!(time_full)).borrow::<TabularLabel>().map(|t| t.ink()).unwrap();
            drop(view);
            leave_isolate(cx, entry);
            (title, time)
        };
        let is_light = |c: Vec4f| c.x + c.y + c.z > 2.0;
        let (title, time) = inks(&mut cx, &root);
        assert!(!is_light(title) && !is_light(time), "light sheet: dark ink, got {title:?} {time:?}");

        // An appearance change, the host's way.
        let switch = |cx: &mut Cx, root: &mut WidgetRef, dark: bool| {
            cx.with_script_vm_id_trusted(vm_id, |vm| {
                desktop_style::install(vm, sheet(dark));
                vm.with_reload(|vm| {
                    makepad_widgets::widgets_mod(vm);
                    desktop_style::apply_widgets(vm);
                    makepad_wm_theme::apply(vm);
                    CLOCK_MODULE.register(vm);
                });
                let source = root
                    .widget_type_id()
                    .and_then(|ty| vm.bx.heap.type_default_for_id(ty))
                    .unwrap_or_else(|| root.script_source());
                root.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), source.into());
                assert!(vm.take_errors().is_empty(), "the reapply evaluated without errors");
            });
        };
        switch(&mut cx, &mut root, true);
        let (title, time) = inks(&mut cx, &root);
        assert!(is_light(title) && is_light(time), "dark sheet: light ink, got {title:?} {time:?}");
        switch(&mut cx, &mut root, false);
        let (title, time) = inks(&mut cx, &root);
        assert!(!is_light(title) && !is_light(time), "back to light: dark ink, got {title:?} {time:?}");

        cx.with_script_vm_id_trusted(vm_id, |vm| shutdown(vm));
        drop(root);
        drop(executor);
        cx.free_splash_vm(vm_id);
    }
}
