//! wm-all — the window manager with its apps linked in.
//!
//! One process: every app is a module (libs/app_module) the desk seats in
//! an isolate of its own and draws into its own texture — no client hub,
//! no child processes, no shared swapchains. The build for a platform
//! that has none of those (iOS), and its desktop twin in the iOS skin,
//! which is how the build is tested outside the simulator. Everything
//! but the build itself is `makepad-wm`.

use makepad_wm::makepad_widgets::*;
use makepad_wm::{App, DesktopStyle, WmBuild};

app_main!(
    App,
    font_set: International,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/NotoColorEmoji.ttf",
        INTER_FONT_ASSET,
        ROBOTO_FLEX_FONT_ASSET,
    ],
    configure: |cx: &mut Cx| {
        cx.set_global(WmBuild {
            modules: vec![
                &makepad_files::FILES_MODULE,
                &makepad_sheets::SHEETS_MODULE,
                &makepad_photos::PHOTOS_MODULE,
                &makepad_app_route::ROUTE_MODULE,
                &makepad_finance::FINANCE_MODULE,
                &makepad_clock::CLOCK_MODULE,
                &makepad_weather::WEATHER_MODULE,
                &makepad_mail::MAIL_MODULE,
                &makepad_notes::NOTES_MODULE,
                &makepad_calendar::CALENDAR_MODULE,
                &makepad_reminders::REMINDERS_MODULE,
                &makepad_calculator::CALCULATOR_MODULE,
            ],
            modules_only: true,
            dynamic_dylibs: false,
            style: DesktopStyle::Ios,
            assistant: Some(makepad_aichat::script_mod),
            title: "wm all-in-one".to_string(),
        });
    }
);

/// A module built only for the tests below: its root panics on demand, in
/// an event or in its draw, the way a real module's did on the phone —
/// so the tests can prove the desk survives that. Never linked into the
/// build; there is no dev path that makes a shipped module panic.
#[cfg(test)]
mod panicking {
    use makepad_wm::makepad_app_module::makepad_ai_services::wire::{ServiceCall, ServiceManifest, ToolResult};
    use makepad_wm::makepad_app_module::*;
    use makepad_wm::makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets_internal.*

        mod.widgets.PanicRoot = #(PanicRoot::register_widget(vm))
    }

    /// Panics on `Signal`; when `panic_in_draw` is set, two turtles into
    /// its draw, so the tile has open turtles to cut back.
    #[derive(Script, ScriptHook, Widget)]
    pub struct PanicRoot {
        #[uid]
        uid: WidgetUid,
        #[source]
        source: ScriptObjectRef,
        #[walk]
        walk: Walk,
        #[layout]
        layout: Layout,
        #[redraw]
        #[rust]
        area: Area,
        #[rust]
        pub panic_in_draw: bool,
        #[rust]
        pub draws: usize,
    }

    impl Widget for PanicRoot {
        fn handle_event(&mut self, _cx: &mut Cx, event: &Event, _scope: &mut Scope) {
            if let Event::Signal = event {
                panic!("the module's root panicked on Signal");
            }
        }

        fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
            cx.begin_turtle(walk, self.layout);
            if self.panic_in_draw {
                cx.begin_turtle(Walk::fill(), Layout::flow_down());
                panic!("the module's root panicked in its draw");
            }
            self.draws += 1;
            cx.end_turtle_with_area(&mut self.area);
            DrawStep::done()
        }
    }

    pub struct PanicModule;
    pub static PANIC_MODULE: PanicModule = PanicModule;

    impl AppModule for PanicModule {
        fn id(&self) -> &'static str {
            "panic_test"
        }

        fn label(&self) -> &'static str {
            "Panic"
        }

        fn register(&self, vm: &mut ScriptVm) {
            script_mod(vm);
        }

        fn open_schema(&self) -> OpenSchema {
            OpenSchema::new(1)
        }

        fn create(&self, vm: &mut ScriptVm, _open: ValidatedOpen, _handles: InstanceHandles) -> InstanceParts {
            let value = script_eval!(vm, {
                use mod.widgets.*
                PanicRoot {}
            });
            let root = WidgetRef::script_from_value(vm, value);
            InstanceParts { root, executor: Box::new(PanicExecutor), shutdown: Box::new(|_vm| {}) }
        }

        fn capabilities(&self) -> &'static [&'static str] {
            &[]
        }
    }

    struct PanicExecutor;

    impl ServiceExecutor for PanicExecutor {
        fn manifest(&self) -> ServiceManifest {
            ServiceManifest::new("panic_test", "Panic", "a test module that panics")
        }

        fn execute(&mut self, _cx: &mut Cx, call: &ServiceCall) -> ExecOutcome {
            ExecOutcome::Done(ToolResult::unavailable(&call.call_id, "a test module"))
        }
    }
}

#[cfg(test)]
mod tests {
    use makepad_wm::makepad_app_module::AppModule;
    use makepad_wm::makepad_widgets::widget_async::current_splash_vm_id;
    use makepad_wm::makepad_widgets::*;
    use makepad_wm::makepad_wm_theme;
    use makepad_wm::module_host::ModuleHost;
    use makepad_wm::module_view::MpModuleView;
    use makepad_wm::MpRunViewAction;
    use super::panicking::{PanicRoot, PANIC_MODULE};

    /// A host with the tile family in the main VM and one instance of the
    /// panicking module per client id given, each seated in a tile.
    fn host_with_panicking_tiles(cx: &mut Cx, clients: &[u64]) -> (ModuleHost, Vec<WidgetRef>) {
        cx.with_vm(makepad_widgets::script_mod);
        cx.with_vm(|vm| {
            // The tokens the tile styles from; the WM evaluates its theme
            // file here.
            vm.eval(script! {
                mod.wm_theme = {
                    background: #1a1b26
                    foreground: #c0caf5
                }
            });
            makepad_wm::module_view::script_mod(vm);
            assert!(vm.take_errors().is_empty(), "the tile family evaluates");
        });
        let mut host = ModuleHost::default();
        let module: &'static dyn AppModule = &PANIC_MODULE;
        let mut tiles = Vec::new();
        for &client in clients {
            let open = module.open_schema().validate("{}", &[]).unwrap();
            host.create(cx, client, module, open, dvec2(402.0, 778.0)).unwrap();
            let instance = host.get(client).unwrap();
            let (vm_id, root) = (instance.vm_id, instance.root.clone());
            let tile = cx.with_vm(|vm| {
                let value = vm.eval(script! {
                    use mod.widgets.*
                    MpModuleView {}
                });
                WidgetRef::script_from_value(vm, value)
            });
            tile.borrow_mut::<MpModuleView>().expect("a module tile").set_root(cx, client, vm_id, root);
            tiles.push(tile);
        }
        (host, tiles)
    }

    fn crash_reported(actions: &[Action]) -> Option<(u64, String)> {
        actions.iter().filter_map(|a| a.as_widget_action()).find_map(|wa| match wa.cast::<MpRunViewAction>() {
            MpRunViewAction::Crashed { client, message } => Some((client, message)),
            _ => None,
        })
    }

    /// One pass, one list, one root turtle, every tile drawn in it: what
    /// the desk does around its tiles. Returns without a panic only when
    /// every stack the tiles touched is balanced again.
    fn draw_tiles(cx: &mut Cx, tiles: &[WidgetRef]) {
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(402.0, 778.0));
        let mut draw_list = DrawList2d::new(cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(402.0, 778.0), Layout::flow_down());
        let before = cx2d.unwind_mark();
        for tile in tiles {
            tile.draw_walk_all(&mut cx2d, &mut Scope::empty(), Walk::fixed(402.0, 300.0));
        }
        let after = cx2d.unwind_mark();
        assert!(before.is_balanced_with(&after), "the tiles left the draw stacks as they found them: {before:?} vs {after:?}");
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    /// The phone's failure: a module's root panics while handling an
    /// event. The tile catches it, the script VM and the isolate scope
    /// are back where they were, the tile reports the crash and shows it
    /// in the instance's place, the host tears the instance down in its
    /// order, and the desk — the tile beside it — keeps drawing.
    #[test]
    fn a_module_whose_root_panics_in_handle_event_is_torn_down_and_the_desk_keeps_drawing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (mut host, tiles) = host_with_panicking_tiles(&mut cx, &[7, 8]);
        let main_heap = cx.with_vm(|vm| vm.bx.heap.heap_key());

        let actions = cx.capture_actions(|cx| tiles[0].handle_event(cx, &Event::Signal, &mut Scope::empty()));
        let (client, message) = crash_reported(&actions).expect("the tile reports the crash to the WM");
        assert_eq!(client, 7);
        assert!(message.contains("panicked on Signal"), "{message}");
        {
            let tile = tiles[0].borrow::<MpModuleView>().unwrap();
            assert!(tile.crashed().is_some_and(|m| m.contains("panicked on Signal")));
            assert!(tile.root().is_none(), "the tile let go of the root");
        }
        // Cx is consistent: the main VM is parked, no isolate is current,
        // and both isolates are still in the table with their heaps.
        assert!(!cx.is_script_vm_held());
        assert_eq!(current_splash_vm_id(&mut cx), MAIN_SPLASH_VM_ID);
        assert_eq!(cx.with_vm(|vm| vm.bx.heap.heap_key()), main_heap);
        let vm_id = host.get(7).unwrap().vm_id;
        cx.with_script_vm_id_trusted(vm_id, |vm| assert!(vm.bx.heap.heap_key() != main_heap));

        // The host's order: the tile has dropped the root, the instance goes.
        assert!(host.teardown(&mut cx, 7));
        assert!(host.get(7).is_none());
        assert!(!cx.is_script_vm_held());

        // The desk keeps drawing: the crashed tile draws its ground and
        // label, the tile beside it draws its root.
        draw_tiles(&mut cx, &tiles);
        let sibling = host.get(8).unwrap().root.clone();
        assert_eq!(sibling.borrow::<PanicRoot>().unwrap().draws, 1, "the healthy instance drew");
        cx.with_vm(|vm| assert_eq!(vm.eval(script! { 20 + 22 }).as_f64(), Some(42.0)));
    }

    /// The other half: the root panics in its draw, two turtles deep. The
    /// tile cuts the draw stacks back to where its own draw stood, so the
    /// desk's pass and list end in balance, and reports the crash.
    #[test]
    fn a_module_whose_root_panics_in_its_draw_leaves_the_frame_balanced() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (mut host, tiles) = host_with_panicking_tiles(&mut cx, &[7, 8]);
        host.get(7).unwrap().root.borrow_mut::<PanicRoot>().unwrap().panic_in_draw = true;

        let actions = cx.capture_actions(|cx| draw_tiles(cx, &tiles));
        let (client, message) = crash_reported(&actions).expect("the tile reports the crash to the WM");
        assert_eq!(client, 7);
        assert!(message.contains("panicked in its draw"), "{message}");
        assert!(tiles[0].borrow::<MpModuleView>().unwrap().root().is_none());
        assert_eq!(host.get(8).unwrap().root.borrow::<PanicRoot>().unwrap().draws, 1, "the tile after the crashed one still drew");
        assert!(!cx.is_script_vm_held());
        assert_eq!(current_splash_vm_id(&mut cx), MAIN_SPLASH_VM_ID);

        assert!(host.teardown(&mut cx, 7));
        // And the next frame draws again, the crashed tile now rootless.
        draw_tiles(&mut cx, &tiles);
        assert_eq!(host.get(8).unwrap().root.borrow::<PanicRoot>().unwrap().draws, 2);
    }

    /// Inside its isolate a module's `self:` names ITS crate's resources,
    /// by (heap, handle): the clock's glyphs come from apps/clock, and the
    /// same handle value in the host's heap is a different file.
    #[test]
    fn module_isolates_resolve_self_resources_to_their_own_crate() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host = ModuleHost::default();
        let module: &'static dyn AppModule = &makepad_clock::CLOCK_MODULE;
        let open = module.open_schema().validate("{}", &[]).unwrap();
        host.create(&mut cx, 1, module, open, dvec2(402.0, 778.0)).unwrap();
        let vm_id = host.get(1).unwrap().vm_id;
        let isolate = cx.with_script_vm_id_trusted(vm_id, |vm| vm.bx.heap.heap_key());
        let main = cx.with_vm(|vm| vm.bx.heap.heap_key());
        assert_ne!(isolate, main);
        let clock_svg = |name: &str| format!("apps/clock/resources/icons/{name}.svg").replace('/', std::path::MAIN_SEPARATOR_STR);
        let (path, handle) = {
            let resources = cx.script_data.resources.resources.borrow();
            let res = resources.iter().find(|r| r.abs_path.ends_with(&clock_svg("alarm"))).expect("the clock's alarm glyph is registered in its isolate");
            let (heap, handle) = res.handles.iter().copied().find(|(heap, _)| *heap == isolate).expect("registered by the isolate's heap");
            assert_eq!(heap, isolate);
            (res.abs_path.clone(), handle)
        };
        assert!(std::path::Path::new(&path).is_file(), "{path} exists natively");
        assert_eq!(cx.get_resource_abs_path(isolate, handle).as_deref(), Some(path.as_str()));
        // Nothing in the isolate resolves to the host's icon folder.
        {
            let resources = cx.script_data.resources.resources.borrow();
            assert!(resources.iter().filter(|r| r.handles.iter().any(|(heap, _)| *heap == isolate)).all(|r| !r.abs_path.contains("apps/wm/resources")));
        }
        cx.load_script_resource(isolate, handle);
        let bytes = cx.get_resource(isolate, handle).expect("the glyph loads by (heap, handle)");
        assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
        // The host's heap never sees the clock's handle as its own.
        assert!(cx.get_resource_abs_path(main, handle).map_or(true, |p| !p.ends_with(&clock_svg("alarm"))));
    }

    /// The clock's large title and time take the dark palette's ink like
    /// every other label when the appearance flips after the module was
    /// created under the light one (the simulator's order).
    #[test]
    fn a_hosted_clocks_large_labels_follow_the_appearance() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host = ModuleHost::default();
        let light = desktop_style::StyleSheet::load_with_appearance(desktop_style::DesktopStyle::Ios, false);
        let dark = desktop_style::StyleSheet::load_with_appearance(desktop_style::DesktopStyle::Ios, true);
        host.apply_style(&mut cx, &light);
        let module: &'static dyn AppModule = &makepad_clock::CLOCK_MODULE;
        let open = module.open_schema().validate("{}", &[]).unwrap();
        host.create(&mut cx, 1, module, open, dvec2(402.0, 778.0)).unwrap();
        let ink = |cx: &mut Cx, host: &ModuleHost, id: &[LiveId]| -> Vec4f {
            let root = host.get(1).unwrap().root.clone();
            let label = root.widget(cx, id);
            let label = label.borrow::<Label>().unwrap_or_else(|| panic!("{id:?} is a Label"));
            label.draw_text.color
        };
        let title_light = ink(&mut cx, &host, ids!(title));
        let date_light = ink(&mut cx, &host, ids!(date_full));
        host.apply_style(&mut cx, &dark);
        let title_dark = ink(&mut cx, &host, ids!(title));
        let date_dark = ink(&mut cx, &host, ids!(date_full));
        assert_ne!(date_light, date_dark, "the small date label flips");
        assert_ne!(title_light, title_dark, "the large title flips too: light {title_light:?} dark {title_dark:?}");
        assert!(title_dark.x > 0.8, "dark ink is light: {title_dark:?}");
    }

    /// A linked module survives a desktop restyle in place: its custom
    /// theme roles follow the new palette and its root keeps its identity.
    #[test]
    fn module_restyle_updates_custom_roles_and_keeps_instance() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(makepad_widgets::script_mod);
        let mut host = ModuleHost::default();
        let module: &'static dyn AppModule = &makepad_sheets::SHEETS_MODULE;
        let open = module.open_schema().validate("{}", &[]).unwrap();
        host.create(&mut cx, 1, module, open, dvec2(900.0, 700.0)).unwrap();
        let uid = host.get(1).unwrap().root.widget_uid();
        host.apply_style(&mut cx, &desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Macos));
        let instance = host.get(1).unwrap();
        assert_eq!(instance.root.widget_uid(), uid);
        cx.with_script_vm_id_trusted(instance.vm_id, |vm| {
            let palette = makepad_wm_theme::current_for_vm(vm).unwrap();
            assert_eq!(palette.get("background"), Some("#ececec"));
            let sheets = vm.module(id!(sheets));
            assert_eq!(vm.bx.heap.value(sheets, id!(bg).into(), NoTrap).as_color(), Some(0xecececff));
            assert!(vm.take_errors().is_empty());
        });
        host.teardown(&mut cx, 1);
    }
}
