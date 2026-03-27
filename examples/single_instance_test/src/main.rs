pub use makepad_widgets;

use makepad_widgets::*;
use std::path::PathBuf;

const APP_ID: &str = "dev.makepad.example.singletest";

fn app_name() -> &'static str {
    APP_ID.rsplit('.').next().unwrap_or(APP_ID)
}

fn socket_path() -> PathBuf {
    crate::makepad_widgets::single_instance::app_socket_path().unwrap_or_else(|| {
        #[cfg(target_os = "macos")]
        {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join(app_name())
                    .join("app.sock");
            }
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
                if !xdg.is_empty() {
                    return PathBuf::from(xdg)
                        .join(app_name())
                        .join("app.sock");
                }
            }
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home)
                    .join(".local")
                    .join("state")
                    .join(app_name())
                    .join("app.sock");
            }
        }

        #[cfg(windows)]
        {
            return std::env::temp_dir().join("dev.makepad.example.singletest.port");
        }

        std::env::temp_dir().join("dev.makepad.example.singletest.sock")
    })
}

fn state_path() -> PathBuf {
    PathBuf::from(format!("{}.state", socket_path().display()))
}

fn write_state_file(received: &[String]) {
    let socket = socket_path();
    let mut state = format!(
        "APP_ID={APP_ID}\nAPP_NAME={}\nSOCKET_PATH={}\nSTATE_PATH={}\n",
        app_name(),
        socket.display(),
        state_path().display(),
    );
    for item in received {
        state.push_str("APP_OPEN=");
        state.push_str(item);
        state.push('\n');
    }
    if let Some(parent) = state_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(state_path(), state);
}

fn read_state_file() -> String {
    std::fs::read_to_string(state_path()).unwrap_or_else(|_| "State file not written yet.".to_string())
}

fn remove_state_file() {
    let _ = std::fs::remove_file(state_path());
}

fn app_bootstrap() {
    Cx::init_log();
    let items: Vec<String> = std::env::args().skip(1).collect();
    let item_refs: Vec<&str> = items.iter().map(|item| item.as_str()).collect();
    if let crate::makepad_widgets::single_instance::SingleInstanceResult::Secondary =
        Cx::enable_single_instance(APP_ID, &item_refs)
    {
        println!("Forwarded open request to primary instance.");
        println!("--- primary state ---");
        print!("{}", read_state_file());
        return;
    }

    if Cx::pre_start() {
        return;
    }

    app_main();
}

#[cfg(not(any(target_os = "android", target_env = "ohos")))]
fn main() {
    app_bootstrap();
}

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "singletest"
                window.inner_size: vec2(920, 760)
                body +: {
                    app := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        header := RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 6
                            padding: 16
                            draw_bg.color: #x1b1f27
                            draw_bg.border_radius: 10.0

                            title := Label{
                                text: "singletest"
                                draw_text.text_style.font_size: 22
                            }
                            subtitle := Label{
                                text: "Open singletest://example-url from the demo page. AppOpen items appear below and are written to the state file."
                                draw_text.wrap: Words
                            }
                            socket_path := Label{
                                text: "Socket: pending"
                                draw_text.wrap: Words
                            }
                            state_path := Label{
                                text: "State: pending"
                                draw_text.wrap: Words
                            }
                            summary := Label{
                                text: "Received items: 0"
                            }
                        }

                        state_panel := RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            padding: 16
                            draw_bg.color: #x161a21
                            draw_bg.border_radius: 10.0

                            Label{
                                text: "State file"
                                draw_text.text_style.font_size: 16
                            }
                            state_contents := Label{
                                width: Fill
                                text: "Waiting for primary state..."
                                draw_text.wrap: Line
                            }
                        }

                        log_panel := RoundedView{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 8
                            padding: 16
                            draw_bg.color: #x10141a
                            draw_bg.border_radius: 10.0

                            Label{
                                text: "AppOpen log"
                                draw_text.text_style.font_size: 16
                            }
                            log_view := ScrollYView{
                                width: Fill
                                height: Fill
                                padding: 12
                                flow: Down
                                spacing: 8
                                received_log := Label{
                                    width: Fill
                                    text: "Waiting for Event::AppOpen items..."
                                    draw_text.wrap: Line
                                }
                            }
                        }
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
    #[rust]
    received: Vec<String>,
}

impl App {
    fn update_ui(&mut self, cx: &mut Cx) {
        let socket_text = format!("Socket: {}", socket_path().display());
        self.ui.label(cx, ids!(socket_path)).set_text(cx, &socket_text);
        self.ui
            .label(cx, ids!(state_path))
            .set_text(cx, &format!("State: {}", state_path().display()));
        self.ui
            .label(cx, ids!(summary))
            .set_text(cx, &format!("Received items: {}", self.received.len()));
        self.ui
            .label(cx, ids!(state_contents))
            .set_text(cx, &read_state_file());

        let log_text = if self.received.is_empty() {
            "Waiting for Event::AppOpen items...".to_string()
        } else {
            self.received.join("\n")
        };
        self.ui
            .label(cx, ids!(received_log))
            .set_text(cx, &log_text);
    }

    fn push_items(&mut self, cx: &mut Cx, items: &[String]) {
        if items.is_empty() {
            return;
        }
        for item in items {
            println!("APP_OPEN={}", item);
            log!("AppOpen {}", item);
            self.received.push(item.clone());
        }
        write_state_file(&self.received);
        self.update_ui(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        write_state_file(&self.received);
        self.update_ui(cx);
    }

    fn handle_shutdown(&mut self, _cx: &mut Cx) {
        remove_state_file();
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::AppOpen(items) = event {
            self.push_items(cx, items);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnCreate(
    _: *const std::ffi::c_void,
    _: *const std::ffi::c_void,
    activity: *const std::ffi::c_void,
) {
    app_bootstrap();
}

#[cfg(target_arch = "wasm32")]
pub fn app_main() {}

#[cfg(target_arch = "wasm32")]
#[export_name = "wasm_create_app"]
pub extern "C" fn create_wasm_app() -> u32 {
    Cx::init_log();
    let app = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
        if let Event::Startup = event {
            *app.borrow_mut() = Some(cx.with_vm(|vm| {
                let value = <App as AppMain>::script_mod(vm);
                <App as ScriptNew>::script_from_value(vm, value)
            }));
        }
        if let Some(app) = &mut *app.borrow_mut() {
            <dyn AppMain>::handle_event(app, cx, event);
        }
    })));
    cx.init_cx_os();
    Box::into_raw(cx) as u32
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub fn app_main() {
    let app = std::rc::Rc::new(std::cell::RefCell::new(None));
    let cx = std::rc::Rc::new(std::cell::RefCell::new(Cx::new(Box::new(
        move |cx, event| {
            if let Event::Startup = event {
                *app.borrow_mut() = Some(cx.with_vm(|vm| {
                    let value = <App as AppMain>::script_mod(vm);
                    <App as ScriptNew>::script_from_value(vm, value)
                }));
            }
            if let Some(app) = &mut *app.borrow_mut() {
                <dyn AppMain>::handle_event(app, cx, event);
            }
        },
    ))));
    cx.borrow_mut().init_cx_os();
    Cx::event_loop(cx);
}
