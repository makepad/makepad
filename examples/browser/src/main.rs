pub use makepad_widgets;

use makepad_cef::BootstrapResult;
use makepad_widgets::*;
use std::cell::RefCell;
use std::rc::Rc;

const INITIAL_URL: &str = "https://www.google.com";

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

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 840)
                body +: {
                    flow: Down

                    RoundedView{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        padding: 10
                        draw_bg+: {
                            color: #x17191d
                        }

                        url_input := TextInput{
                            width: Fill
                            height: Fit
                            empty_text: "Enter a URL"
                        }

                        go_button := Button{
                            width: 90
                            text: "Go"
                        }
                    }

                    browser := Browser{
                        width: Fill
                        height: Fill
                        url: "https://www.google.com"
                    }
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

impl App {
    fn navigate_to_input(&mut self, cx: &mut Cx, text: &str) {
        let mut url = text.trim().to_string();
        if url.is_empty() {
            url = "about:blank".to_string();
        } else if !url.contains("://") {
            url = format!("https://{url}");
        }
        self.ui.text_input(cx, ids!(url_input)).set_text(cx, &url);
        self.ui.browser(cx, ids!(browser)).set_url(cx, &url);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(go_button)).clicked(actions) {
            let text = self.ui.text_input(cx, ids!(url_input)).text();
            self.navigate_to_input(cx, &text);
        }

        if let Some((text, _)) = self.ui.text_input(cx, ids!(url_input)).returned(actions) {
            self.navigate_to_input(cx, &text);
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
            self.ui
                .text_input(cx, ids!(url_input))
                .set_text(cx, INITIAL_URL);
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
