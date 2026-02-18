use crate::cx::Cx;
use crate::event::Event;
use crate::ui_runner::UiRunner;

#[cfg(target_env = "ohos")]
pub use napi_ohos;

pub fn should_run_stdin_loop_from_env() -> bool {
    std::env::args().any(|v| v == "--stdin-loop")
        || std::env::var("STUDIO").is_ok_and(|v| !v.trim().is_empty())
}

fn normalize_studio_http_from_studio_var(studio: &str) -> String {
    let studio = studio.trim().trim_end_matches('/');
    if studio.is_empty() {
        return String::new();
    }

    let base = if studio.contains("://") {
        studio.to_string()
    } else {
        format!("http://{studio}")
    };

    if base.contains("/$studio_web_socket") {
        base
    } else {
        format!("{base}/$studio_web_socket")
    }
}

fn with_studio_build_id(studio_http: String) -> String {
    let Ok(build_id) = std::env::var("STUDIO_BUILD_ID") else {
        return studio_http;
    };
    let build_id = build_id.trim();
    if build_id.is_empty() {
        return studio_http;
    }

    let normalized = studio_http.trim_end_matches('/').to_string();
    if normalized.rsplit('/').next().is_some_and(|part| part == build_id) {
        return normalized;
    }
    if normalized.contains("/$studio_web_socket/") {
        return normalized;
    }
    format!("{normalized}/{build_id}")
}

pub fn resolve_studio_http(default: &str) -> String {
    if let Ok(studio) = std::env::var("STUDIO") {
        let studio_http = normalize_studio_http_from_studio_var(&studio);
        if !studio_http.is_empty() {
            return with_studio_build_id(studio_http);
        }
    }

    with_studio_build_id(default.to_string())
}

pub trait AppMain {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event);
    fn ui_runner(&self) -> UiRunner<Self>
    where
        Self: Sized + 'static,
    {
        // This assumes there is only one `AppMain`, and that `0` is reserved for it.
        UiRunner::new(0)
    }
}

#[macro_export]
macro_rules! app_main {
    ( $ app: ident) => {
        #[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
        pub fn app_main() {
            //Cx::init_log();
            if Cx::pre_start() {
                return;
            }

            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = std::rc::Rc::new(std::cell::RefCell::new(Cx::new(Box::new(
                move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| $app::run(vm)));
                    }
                    if let Event::LiveEdit = event {
                        //app.borrow_mut().update_main(cx);
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                },
            ))));
            let studio_http = $crate::resolve_studio_http(std::option_env!("STUDIO").unwrap_or(""));
            cx.borrow_mut().init_websockets(&studio_http);
            if $crate::should_run_stdin_loop_from_env() {
                cx.borrow_mut().in_makepad_studio = true;
            }
            //cx.borrow_mut().init_websockets("");
            cx.borrow_mut().init_cx_os();
            Cx::event_loop(cx);
        }

        /*
        #[cfg(target_os = "android")]
        #[no_mangle]
        pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onNewCx(_: *const std::ffi::c_void, _: *const std::ffi::c_void) -> i64 {
            Cx::android_entry(||{
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move | cx, event | {
                    if let Event::Construct = event {
                        *app.borrow_mut() = Some($app::new_main(cx));
                    }
                    if let Event::LiveEdit = event{
                        app.borrow_mut().update_main(cx);
                    }
                    app.borrow_mut().as_mut().unwrap().handle_event(cx, event);
                })));
                live_design(&mut cx);
                cx.init_cx_os();
                cx
            })
        }*/

        #[cfg(target_os = "android")]
        #[no_mangle]
        pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnCreate(
            _: *const std::ffi::c_void,
            _: *const std::ffi::c_void,
            activity: *const std::ffi::c_void,
        ) {
            Cx::init_log();
            Cx::android_entry(activity, || {
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| $app::run(vm)));
                    }
                    if let Event::LiveEdit = event {
                        //app.borrow_mut().update_main(cx);
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                })));
                let studio_http =
                    $crate::resolve_studio_http(std::option_env!("STUDIO").unwrap_or(""));
                cx.init_websockets(&studio_http);
                cx.init_cx_os();
                cx
            })
        }

        #[cfg(target_env = "ohos")]
        #[no_mangle]
        extern "C" fn ohos_init_app_main(
            exports: $crate::napi_ohos::JsObject,
            env: $crate::napi_ohos::Env,
        ) -> $crate::napi_ohos::Result<()> {
            Cx::ohos_init(exports, env, || {
                let app = std::rc::Rc::new(std::cell::RefCell::new(None));
                let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                    if let Event::Startup = event {
                        *app.borrow_mut() = Some(cx.with_vm(|vm| $app::run(vm)));
                    }
                    if let Event::LiveEdit = event {
                        //app.borrow_mut().update_main(cx);
                    }
                    if let Some(app) = &mut *app.borrow_mut() {
                        <dyn AppMain>::handle_event(app, cx, event);
                    }
                })));
                let studio_http =
                    $crate::resolve_studio_http(std::option_env!("STUDIO").unwrap_or(""));
                cx.init_websockets(&studio_http);
                cx.init_cx_os();
                cx
            });
            Ok(())
        }

        #[cfg(target_arch = "wasm32")]
        pub fn app_main() {}

        #[export_name = "wasm_create_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            let app = std::rc::Rc::new(std::cell::RefCell::new(None));
            let mut cx = Box::new(Cx::new(Box::new(move |cx, event| {
                if let Event::Startup = event {
                    *app.borrow_mut() = Some(cx.with_vm(|vm| $app::run(vm)));
                }
                if let Some(app) = &mut *app.borrow_mut() {
                    <dyn AppMain>::handle_event(app, cx, event);
                }
            })));
            let studio_http = $crate::resolve_studio_http(std::option_env!("STUDIO").unwrap_or(""));
            cx.init_websockets(&studio_http);
            cx.init_cx_os();
            Box::into_raw(cx) as u32
        }

        #[export_name = "wasm_process_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_process_msg(msg_ptr: u32, cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.process_to_wasm(msg_ptr)
        }

        #[export_name = "wasm_return_first_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_return_first_msg(cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.os.from_wasm.take().unwrap().release_ownership()
        }
    };
}

#[cfg(target_env = "ohos")]
#[napi_derive_ohos::module_exports]
fn init(exports: napi_ohos::JsObject, env: napi_ohos::Env) -> napi_ohos::Result<()> {
    #[allow(improper_ctypes)]
    extern "C" {
        fn ohos_init_app_main(
            exports: napi_ohos::JsObject,
            env: napi_ohos::Env,
        ) -> napi_ohos::Result<()>;
    }
    unsafe { ohos_init_app_main(exports, env) }
}
