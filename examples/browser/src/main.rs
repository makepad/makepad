pub use makepad_widgets;

use makepad_cef::BootstrapResult;
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const INITIAL_URL: &str = "https://makepad.nl";
const NATIVE_BROWSER_COUNT: usize = 20;

fn startup_trace(message: &str) {
    use std::io::Write;

    let path = std::env::temp_dir().join("makepad-example-browser-startup.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.NativeBrowserListBase = #(NativeBrowserList::register_widget(vm))
    mod.widgets.NativeBrowserList = set_type_default() do mod.widgets.NativeBrowserListBase{
        width: Fill
        height: Fill

        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: true
            scroll_bar: ScrollBar{}

            BrowserCard := RoundedView{
                width: Fill
                height: 420
                margin: Inset{top: 8 bottom: 8 left: 0 right: 0}
                padding: Inset{top: 14 bottom: 14 left: 14 right: 14}
                flow: Down
                spacing: 10
                draw_bg.color: #x101521
                draw_bg.border_radius: 12.0

                header := View{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 4

                    title := Label{
                        text: "Browser"
                        draw_text.color: #xfff
                        draw_text.text_style: theme.font_bold{font_size: 12}
                    }

                    subtitle := Label{
                        text: ""
                        draw_text.color: #x9fb0d7
                        draw_text.text_style: theme.font_regular{font_size: 10}
                    }
                }

                browser := Browser{
                    width: Fill
                    height: Fill
                    backend: BrowserBackend.Native
                    url: "https://makepad.nl"
                }
            }
        }
    }

    let AppDock = Dock{
        width: Fill
        height: Fill

        root := DockTabs{
            tabs: [@cef_tab @native_tab]
            selected: 0
            closable: false
        }

        cef_tab := DockTab{
            name: "CEF"
            template: @PermanentTab
            kind: @TabCEF
        }

        native_tab := DockTab{
            name: "Native PortalList"
            template: @PermanentTab
            kind: @TabNative
        }

        TabCEF := View{
            width: Fill
            height: Fill

            cef_browser := Browser{
                width: Fill
                height: Fill
                backend: BrowserBackend.CEF
                url: "https://www.google.nl"
            }
        }

        TabNative := View{
            width: Fill
            height: Fill
            padding: Inset{top: 12 bottom: 12 left: 12 right: 12}
            flow: Down
            spacing: 10

            RoundedView{
                width: Fill
                height: Fit
                flow: Down
                spacing: 4
                padding: Inset{top: 12 bottom: 12 left: 14 right: 14}
                draw_bg +: {
                    color: #x17191d
                }

                Label{
                    text: "Native browser PortalList"
                    draw_text.color: #xfff
                    draw_text.text_style: theme.font_bold{font_size: 12}
                }

                Label{
                    text: "This tab keeps 20 persistent system browsers attached to their Browser widgets while the Dock tab is active."
                    draw_text.color: #x94a7d4
                    draw_text.text_style.font_size: 10
                }
            }

            native_browser_list := mod.widgets.NativeBrowserList{}
        }
    }

    mod.gc.set_static(AppDock)
    mod.gc.run()

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 960)
                body +: {
                    dock := AppDock{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

#[derive(Script, ScriptHook, Widget)]
pub struct NativeBrowserList {
    #[deref]
    view: View,
    #[rust]
    active: bool,
    #[rust]
    bindings: HashMap<WidgetUid, usize>,
    #[rust]
    browsers: HashMap<WidgetUid, BrowserRef>,
}

impl NativeBrowserList {
    fn set_active_internal(&mut self, cx: &mut Cx, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        for browser in self.browsers.values() {
            browser.set_visible(cx, active);
        }
        self.redraw(cx);
    }

    fn bind_item(&mut self, cx: &mut Cx, item: &WidgetRef, item_id: usize) {
        let widget_uid = item.widget_uid();
        let is_new_browser = !self.browsers.contains_key(&widget_uid);
        let browser = item.browser(cx, ids!(browser));
        browser.set_visible(cx, self.active);

        if self.bindings.get(&widget_uid) != Some(&item_id) {
            self.bindings.insert(widget_uid, item_id);
            item.label(cx, ids!(title))
                .set_text(cx, &format!("Native Browser {}", item_id + 1));
            item.label(cx, ids!(subtitle)).set_text(
                cx,
                &format!("PortalList row {} attached to {}", item_id + 1, INITIAL_URL),
            );
        }

        if is_new_browser {
            browser.set_url(cx, INITIAL_URL);
        }

        self.browsers.insert(widget_uid, browser);
    }
}

impl Widget for NativeBrowserList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, NATIVE_BROWSER_COUNT);
                while let Some(item_id) = list.next_visible_item(cx) {
                    let item_widget = list.item(cx, item_id, id!(BrowserCard));
                    self.bind_item(cx, &item_widget, item_id);
                    item_widget.draw_all_unscoped(cx);
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl App {
    fn set_native_tab_active(&mut self, cx: &mut Cx, active: bool) {
        self.ui
            .dock(cx, ids!(dock))
            .item(id!(native_tab))
            .widget(cx, ids!(native_browser_list))
            .borrow_mut::<NativeBrowserList>()
            .map(|mut list| list.set_active_internal(cx, active));
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            if let DockAction::TabWasPressed(tab_id) = action.cast() {
                self.set_native_tab_active(cx, tab_id == id!(native_tab));
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.set_native_tab_active(cx, false);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

fn main() {
    app_main();
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub fn app_main() {
    startup_trace(&format!(
        "start exe={} bundled_env={:?} args={:?}",
        std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "<unresolved>".to_string()),
        std::env::var_os("MAKEPAD_CEF_APP_BUNDLE_EXEC"),
        std::env::args().collect::<Vec<_>>()
    ));

    if let Err(err) = makepad_cef::reexec_into_app_bundle_if_needed() {
        startup_trace(&format!("bundle re-exec failed: {err}"));
        panic!("CEF bundle re-exec failed: {err}");
    }
    startup_trace("bundle re-exec ok");

    match makepad_cef::bootstrap() {
        Ok(BootstrapResult::Continue) => {}
        Ok(BootstrapResult::Exit(code)) => {
            startup_trace(&format!("bootstrap exit {code}"));
            std::process::exit(code)
        }
        Err(err) => {
            startup_trace(&format!("bootstrap error: {err}"));
            panic!("CEF bootstrap failed: {err}")
        }
    }
    startup_trace("bootstrap continue");

    Cx::init_log();
    if Cx::pre_start() {
        startup_trace("Cx::pre_start returned true");
        return;
    }
    startup_trace("Cx::pre_start returned false");

    if let Err(err) = makepad_cef::initialize() {
        startup_trace(&format!("cef initialize error: {err}"));
        panic!("CEF initialize failed: {err}");
    }
    startup_trace("cef initialize ok");

    let app = Rc::new(RefCell::new(None));
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if let Event::Startup = event {
            *app.borrow_mut() = Some(cx.with_vm(|vm| {
                let value = <App as AppMain>::script_mod(vm);
                let mut app = <App as ScriptNew>::script_from_value(vm, value);
                <App as AppMain>::after_new_from_script(vm, &mut app);
                app
            }));
            cx.start_hot_reload_file_observer_if_requested();
        }
        if let Event::LiveEdit = event {
            let mut app_ref = app.borrow_mut();
            if let Some(app) = app_ref.as_mut() {
                cx.with_vm(|vm| {
                    let value = vm.with_reload(|vm| <App as AppMain>::script_mod(vm));
                    <App as ScriptApply>::script_apply(
                        app,
                        vm,
                        &Apply::Reload,
                        &mut Scope::empty(),
                        value,
                    );
                });
            }
        }
        if let Some(app) = &mut *app.borrow_mut() {
            <dyn AppMain>::handle_event(app, cx, event);
        }
    }))));

    let studio_http = resolve_studio_http();
    cx.borrow_mut().init_websockets(&studio_http);
    if should_run_stdin_loop_from_env() {
        cx.borrow_mut().in_makepad_studio = true;
    }
    cx.borrow_mut().init_cx_os();
    Cx::event_loop(cx.clone());
    drop(cx);
    makepad_cef::shutdown();
}

#[cfg(any(target_arch = "wasm32", target_os = "android", target_env = "ohos"))]
pub fn app_main() {
    panic!("makepad-example-browser is desktop-only");
}
